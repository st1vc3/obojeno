//! `obojeno daemon`: the long-running process that owns theming.
//!
//! Clients talk to it over a Unix socket (see `obojeno_core::ipc`). Socket
//! threads only parse and queue; all work runs one job at a time on the main
//! thread, which AppKit (transitions) requires anyway.

use std::collections::{HashSet, VecDeque};
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
use obojeno_core::gallery::{Gallery, Index, IndexEntry};
use obojeno_core::ipc::{self, Reply, Request};
use obojeno_core::{Mode, Palette};

use crate::apply::{self, Applied};
use crate::paths::Paths;
use crate::state::{Source, State};
use crate::{actions, gallery, macos, watch};

/// Work for the main thread.
pub enum Job {
    /// A client request; the reply goes back to its connection.
    Request(Request, Sender<Reply>),
    /// Something changed on the system (see `watch`).
    Change(Change),
    /// A gallery image finished indexing.
    Indexed(PathBuf, Result<IndexEntry, String>),
    /// Index the gallery ahead of time, so the menu bar shows it at once.
    WarmGallery,
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
    gallery: Mutex<GalleryWork>,
    /// The config file as last applied, to ignore watcher events for edits
    /// the daemon made itself.
    config_text: Mutex<String>,
}

/// The gallery index and the images being worked on.
#[derive(Default)]
struct GalleryWork {
    index: Index,
    indexing: HashSet<PathBuf>,
    /// Images that could not be indexed; not retried until the daemon restarts.
    failed: HashSet<PathBuf>,
    /// Results since subscribers last got the gallery.
    unannounced: usize,
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
            gallery: Mutex::new(GalleryWork::default()),
            config_text: Mutex::new(String::new()),
        })
        .unwrap_or_else(|_| unreachable!("run is called once"));

    std::thread::Builder::new()
        .name("obojeno-socket".into())
        .spawn(move || accept(&listener))?;

    daemon().gallery.lock().expect("gallery lock").index =
        Index::load(&gallery::index_file(&daemon().paths));
    let _watchers = watch::start(&daemon().paths.config_file)?;
    let app = NSApplication::sharedApplication(mtm);
    app.setActivationPolicy(NSApplicationActivationPolicy::Accessory);
    // Theme once at start, so the desktop is right after login.
    submit(Job::Change(Change::Wallpaper));
    submit(Job::WarmGallery);
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
            Job::Indexed(path, result) => indexed(path, result),
            Job::WarmGallery => {
                if let Err(e) = gallery_now() {
                    tracing::warn!("gallery: {e:#}");
                }
            }
        }
    }
    DRAINING.set(false);
}

fn handle(request: Request) -> Reply {
    match request {
        Request::State => return current_state().map_or_else(error, Reply::State),
        Request::Gallery => return gallery_now().map_or_else(error, Reply::Gallery),
        _ => {}
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
            let text = std::fs::read_to_string(&daemon().paths.config_file).unwrap_or_default();
            if *daemon().config_text.lock().expect("config lock") == text {
                return Ok(());
            }
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
    *daemon().config_text.lock().expect("config lock") =
        std::fs::read_to_string(&daemon().paths.config_file).unwrap_or_default();
    broadcast(&Reply::State(state));
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
        scheme: config.scheme,
        contrast: config.contrast,
        wallpaper_dirs: config.wallpaper_dirs.clone(),
    })
}

/// The gallery as far as it is indexed; starts indexing the rest.
fn gallery_now() -> anyhow::Result<Gallery> {
    let paths = &daemon().paths;
    let config = Config::load(&paths.config_file)?;
    let state = State::load(&paths.state_file());
    let (current, mode) =
        current_state().map_or_else(|_| (None, macos::system_mode()), |s| (s.wallpaper, s.mode));
    let mut work = daemon().gallery.lock().expect("gallery lock");
    let (mut gallery, missing) = gallery::build(
        paths,
        &config,
        &state,
        &mut work.index,
        current.as_deref(),
        mode,
    );
    let todo: Vec<PathBuf> = missing
        .into_iter()
        .filter(|p| !work.failed.contains(p) && !work.indexing.contains(p))
        .collect();
    work.indexing.extend(todo.iter().cloned());
    gallery.pending = work.indexing.len();
    work.index.save(&gallery::index_file(paths))?;
    drop(work);
    if !todo.is_empty() {
        tracing::info!("indexing {} gallery images", todo.len());
        start_indexing(todo);
    }
    Ok(gallery)
}

/// Indexes images on a few background threads, reporting each result to the
/// main thread.
fn start_indexing(todo: Vec<PathBuf>) {
    let queue = Mutex::new(todo);
    let spawned = std::thread::Builder::new()
        .name("obojeno-index".into())
        .spawn(move || {
            std::thread::scope(|scope| {
                for _ in 0..gallery::parallelism() {
                    scope.spawn(|| {
                        loop {
                            let next = queue.lock().expect("queue lock").pop();
                            let Some(path) = next else { break };
                            let result = gallery::index_image(&daemon().paths, &path);
                            submit(Job::Indexed(path, result));
                        }
                    });
                }
            });
        });
    if let Err(e) = spawned {
        tracing::warn!("cannot start indexing: {e}");
    }
}

fn indexed(path: PathBuf, result: Result<IndexEntry, String>) {
    let paths = &daemon().paths;
    let mut work = daemon().gallery.lock().expect("gallery lock");
    work.indexing.remove(&path);
    match result {
        Ok(entry) => {
            // Clicking it in the gallery then themes without decoding it again.
            let mut state = State::load(&paths.state_file());
            state.cache_color(&entry.key, entry.source);
            if let Err(e) = state.save(&paths.state_file()) {
                tracing::warn!("{e:#}");
            }
            work.index.entries.insert(path, entry);
        }
        Err(e) => {
            tracing::warn!("cannot index {}: {e}", path.display());
            work.failed.insert(path);
        }
    }
    work.unannounced += 1;
    // Batch updates while many images finish at once.
    let announce = work.indexing.is_empty() || work.unannounced >= gallery::parallelism();
    if announce {
        work.unannounced = 0;
    }
    drop(work);
    if announce {
        match gallery_now() {
            Ok(g) => broadcast(&Reply::Gallery(g)),
            Err(e) => tracing::warn!("gallery: {e:#}"),
        }
    }
}

fn broadcast(reply: &Reply) {
    daemon()
        .subscribers
        .lock()
        .expect("subscribers lock")
        .retain(|tx| tx.send(reply.clone()).is_ok());
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
