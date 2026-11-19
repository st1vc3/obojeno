//! `obojeno daemon`: the long-running process that owns theming.
//!
//! Clients talk to it over a Unix socket (see `obojeno_core::ipc`). Socket
//! threads only parse and queue; all work runs one job at a time on the main
//! thread, which AppKit (transitions) requires anyway.

use std::collections::VecDeque;
use std::io::{BufRead as _, BufReader, Write as _};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Sender};
use std::sync::{Mutex, OnceLock};

use anyhow::{Context as _, bail};
use dispatch2::DispatchQueue;
use objc2::MainThreadMarker;
use objc2_app_kit::{NSApplication, NSApplicationActivationPolicy};
use obojeno_core::config::Config;
use obojeno_core::ipc::{self, Reply, Request};
use obojeno_core::{Mode, Palette};

use crate::apply::{self, Applied};
use crate::paths::Paths;
use crate::state::{Source, State};
use crate::{actions, macos, watch};

/// Work for the main thread.
pub enum Job {
    /// A client request; the reply goes back to its connection.
    Request(Request, Sender<Reply>),
    /// Something changed on the system (see `watch`).
    Change(Change),
}

/// Something that may have changed; the daemon checks and re-themes if so.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Change {
    /// System Settings, a Space switch or a screen change.
    Wallpaper,
    Appearance,
    Config,
}

struct Daemon {
    paths: Paths,
    jobs: Mutex<VecDeque<Job>>,
    subscribers: Mutex<Vec<Sender<Reply>>>,
    /// What is on screen; `None` until the first apply.
    current: Mutex<Option<ipc::State>>,
}

static DAEMON: OnceLock<Daemon> = OnceLock::new();

fn daemon() -> &'static Daemon {
    DAEMON.get().expect("daemon started")
}

pub fn run(paths: Paths) -> anyhow::Result<()> {
    let mtm = MainThreadMarker::new().context("the daemon must run on the main thread")?;
    let socket = ipc::socket_path(&paths.state);
    let listener = bind(&socket)?;
    tracing::info!("listening on {}", socket.display());

    DAEMON
        .set(Daemon {
            paths,
            jobs: Mutex::new(VecDeque::new()),
            subscribers: Mutex::new(Vec::new()),
            current: Mutex::new(None),
        })
        .unwrap_or_else(|_| unreachable!("run is called once"));

    std::thread::Builder::new()
        .name("obojeno-socket".into())
        .spawn(move || accept(&listener))?;

    let _watchers = watch::start(&daemon().paths.config_file)?;
    let app = NSApplication::sharedApplication(mtm);
    app.setActivationPolicy(NSApplicationActivationPolicy::Accessory);
    // Theme once at start, so the desktop is right after login.
    submit(Job::Change(Change::Wallpaper));
    app.run();
    Ok(())
}

/// Binds the socket, replacing a stale one but refusing to run twice.
fn bind(socket: &Path) -> anyhow::Result<UnixListener> {
    if let Some(dir) = socket.parent() {
        std::fs::create_dir_all(dir).with_context(|| format!("creating {}", dir.display()))?;
    }
    if UnixStream::connect(socket).is_ok() {
        bail!(
            "another obojeno daemon is already running ({})",
            socket.display()
        );
    }
    match std::fs::remove_file(socket) {
        Ok(()) => {}
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => return Err(e).with_context(|| format!("removing stale {}", socket.display())),
    }
    UnixListener::bind(socket).with_context(|| format!("binding {}", socket.display()))
}

/// Queues work for the main thread from any thread.
pub fn submit(job: Job) {
    daemon().jobs.lock().expect("jobs lock").push_back(job);
    DispatchQueue::main().exec_async(drain);
}

thread_local! {
    static DRAINING: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

/// Runs queued jobs in order. Transitions pump the run loop, which can call
/// this again; the nested call returns and the outer loop picks the job up.
fn drain() {
    if DRAINING.replace(true) {
        return;
    }
    loop {
        let job = daemon().jobs.lock().expect("jobs lock").pop_front();
        let Some(job) = job else { break };
        match job {
            Job::Request(request, reply) => {
                let response = handle(request);
                let _ = reply.send(response);
            }
            Job::Change(change) => {
                if let Err(e) = handle_change(change) {
                    tracing::warn!("{change:?} change: {e:#}");
                }
            }
        }
    }
    DRAINING.set(false);
}

fn handle(request: Request) -> Reply {
    if request == Request::State {
        return current_state().map_or_else(error, Reply::State);
    }
    let paths = &daemon().paths;
    let result = actions::execute(paths, &request).and_then(|applied| {
        publish(&applied, &Config::load(&paths.config_file)?).map(|()| applied)
    });
    result.map_or_else(error, |applied| Reply::Report(applied.report))
}

#[allow(clippy::needless_pass_by_value)] // Used as a `map_or_else` callback.
fn error(e: anyhow::Error) -> Reply {
    Reply::Error {
        message: format!("{e:#}"),
    }
}

fn handle_change(change: Change) -> anyhow::Result<()> {
    match change {
        Change::Wallpaper => {
            let path = macos::current_wallpaper()?;
            let current = daemon().current.lock().expect("state lock").clone();
            if current.is_some_and(|s| s.wallpaper.as_deref() == Some(path.as_path())) {
                return Ok(());
            }
            tracing::info!("wallpaper is now {}", path.display());
            run_apply(None, Some(&path)).map(drop)
        }
        Change::Appearance => {
            let mode = macos::system_mode();
            let current = daemon().current.lock().expect("state lock").clone();
            if current.is_some_and(|s| s.mode == mode) {
                return Ok(());
            }
            tracing::info!("appearance is now {mode}");
            run_apply(Some(mode), None).map(drop)
        }
        Change::Config => {
            tracing::info!("config changed");
            run_apply(None, None).map(drop)
        }
    }
}

fn run_apply(mode: Option<Mode>, wallpaper: Option<&Path>) -> anyhow::Result<Applied> {
    let paths = &daemon().paths;
    let applied = apply::apply(paths, mode, wallpaper)?;
    publish(&applied, &Config::load(&paths.config_file)?)?;
    Ok(applied)
}

/// Records what is on screen and tells subscribers.
fn publish(applied: &Applied, config: &Config) -> anyhow::Result<()> {
    let state = state_from(
        applied.report.wallpaper.clone(),
        applied.report.mode,
        &applied.palette,
        config,
    )?;
    *daemon().current.lock().expect("state lock") = Some(state.clone());
    daemon()
        .subscribers
        .lock()
        .expect("subscribers lock")
        .retain(|tx| tx.send(Reply::State(state.clone())).is_ok());
    Ok(())
}

fn state_from(
    wallpaper: Option<PathBuf>,
    mode: Mode,
    palette: &Palette,
    config: &Config,
) -> anyhow::Result<ipc::State> {
    let fixed_color = match State::load(&daemon().paths.state_file()).source {
        Source::Wallpaper => None,
        Source::Color { color } => Some(color),
    };
    Ok(ipc::State {
        wallpaper,
        fixed_color,
        mode,
        palette: palette.clone(),
        transition: config.transition.style,
        duration: config.transition.duration,
    })
}

fn current_state() -> anyhow::Result<ipc::State> {
    daemon()
        .current
        .lock()
        .expect("state lock")
        .clone()
        .context("not themed yet")
}

fn accept(listener: &UnixListener) {
    for stream in listener.incoming() {
        match stream {
            Ok(stream) => {
                let spawned = std::thread::Builder::new()
                    .name("obojeno-client".into())
                    .spawn(move || {
                        if let Err(e) = serve(stream) {
                            tracing::debug!("client: {e:#}");
                        }
                    });
                if let Err(e) = spawned {
                    tracing::warn!("cannot serve client: {e}");
                }
            }
            Err(e) => tracing::warn!("accepting a client failed: {e}"),
        }
    }
}

fn serve(stream: UnixStream) -> anyhow::Result<()> {
    let mut line = String::new();
    BufReader::new(&stream).read_line(&mut line)?;
    let mut writer = &stream;
    let request: Request = match serde_json::from_str(&line) {
        Ok(request) => request,
        Err(e) => {
            return send(
                &mut writer,
                &Reply::Error {
                    message: format!("bad request: {e}"),
                },
            );
        }
    };
    let (tx, rx) = mpsc::channel();
    if request == Request::Subscribe {
        // Register first so no change between reading the state and
        // subscribing gets lost.
        daemon()
            .subscribers
            .lock()
            .expect("subscribers lock")
            .push(tx);
        if let Ok(state) = current_state() {
            send(&mut writer, &Reply::State(state))?;
        }
        for reply in rx {
            send(&mut writer, &reply)?;
        }
        return Ok(());
    }
    submit(Job::Request(request, tx));
    let reply = rx.recv().context("the daemon dropped the request")?;
    send(&mut writer, &reply)
}

fn send(writer: &mut impl std::io::Write, reply: &Reply) -> anyhow::Result<()> {
    let mut line = serde_json::to_vec(reply)?;
    line.push(b'\n');
    writer.write_all(&line)?;
    writer.flush()?;
    Ok(())
}

/// Sends one request to a running daemon. `None` when no daemon runs.
pub fn request(paths: &Paths, request: &Request) -> anyhow::Result<Option<Reply>> {
    let socket = ipc::socket_path(&paths.state);
    let mut stream = match UnixStream::connect(&socket) {
        Ok(stream) => stream,
        Err(e)
            if matches!(
                e.kind(),
                std::io::ErrorKind::NotFound | std::io::ErrorKind::ConnectionRefused
            ) =>
        {
            return Ok(None);
        }
        Err(e) => return Err(e).with_context(|| format!("connecting to {}", socket.display())),
    };
    let mut line = serde_json::to_vec(request)?;
    line.push(b'\n');
    stream.write_all(&line)?;
    let mut reply = String::new();
    BufReader::new(&stream).read_line(&mut reply)?;
    Ok(Some(
        serde_json::from_str(&reply).context("bad reply from the daemon")?,
    ))
}
