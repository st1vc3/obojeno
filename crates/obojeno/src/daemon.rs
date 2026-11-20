//! `obojeno daemon`: the long-running process that owns theming.
//!
//! Clients talk to it over a Unix socket (see `obojeno_core::ipc`). Socket
//! threads only parse and queue; all work runs one job at a time on the main
//! thread, which AppKit (transitions) requires anyway.

use std::collections::{HashSet, VecDeque};
use std::io::{BufRead as _, BufReader};
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
use crate::{
    actions, desktop, gallery, imageio, lid, lid_view, macos, screen_images, setup, watch,
};

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
    /// The wallpaper was changed (System Settings, another tool).
    Wallpaper,
    /// A Space switch or a screen change: the wallpaper on screen may be
    /// another one.
    Space,
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
    // Not an error: launchd restarts a daemon that fails, and the one
    // already running is doing the job.
    if UnixStream::connect(&socket).is_ok() {
        eprintln!(
            "obojeno: a daemon is already running ({})",
            socket.display()
        );
        return Ok(());
    }
    let listener = bind(&socket)?;
    imageio::decode_in_helpers();
    tracing::info!(
        "obojeno {} listening on {}",
        env!("CARGO_PKG_VERSION"),
        socket.display()
    );

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

    daemon().gallery.lock().expect("gallery lock").index = Index::load(
        &gallery::index_file(&daemon().paths),
        imageio::EXTRACTION_VERSION,
    );
    let _watchers = watch::start(&daemon().paths.config_file)?;
    // Before the first theme, so a broken config shows up as an apply error.
    if let Ok(config) = Config::load(&daemon().paths.config_file) {
        desktop::apply_settings(config.finish, config.lid_animation, mtm);
    }
    lid::watch();
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
    // sockaddr_un holds 104 bytes including the terminating NUL.
    let len = socket.as_os_str().len();
    if len >= 104 {
        bail!(
            "the socket path {} is {len} bytes, but macOS allows 103; set XDG_STATE_HOME to a shorter folder",
            socket.display()
        );
    }
    if let Some(dir) = socket.parent() {
        std::fs::create_dir_all(dir).with_context(|| format!("creating {}", dir.display()))?;
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
        // Objects AppKit hands out autoreleased go when the job is done, not
        // when the run loop next drains its pool.
        objc2::rc::autoreleasepool(|_| run_job(job));
    }
    DRAINING.set(false);
}

fn run_job(job: Job) {
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

fn handle(request: Request) -> Reply {
    let paths = &daemon().paths;
    match request {
        Request::State => state_reply(),
        Request::Gallery => gallery_now().map_or_else(error, Reply::Gallery),
        Request::Targets => {
            setup::targets(paths).map_or_else(error, |targets| Reply::Targets { targets })
        }
        Request::Import { paths: images } => {
            change_gallery(|config| gallery::import(paths, config, &images).map(drop))
        }
        Request::Remove { path } => change_gallery(|config| gallery::remove(paths, config, &path)),
        Request::LidPreview { percent } => {
            // Jobs run on the main thread.
            let Some(mtm) = MainThreadMarker::new() else {
                return error(anyhow::anyhow!("not on the main thread"));
            };
            lid_view::preview(percent, mtm);
            state_reply()
        }
        request => actions::execute(paths, &request)
            .and_then(|applied| {
                publish(&applied, &Config::load(&paths.config_file)?);
                Ok(applied)
            })
            .map_or_else(error, |applied| Reply::Report(applied.report)),
    }
}

fn state_reply() -> Reply {
    current_state().map_or_else(error, |state| Reply::State(Box::new(state)))
}

/// Changes the library, then replies with the gallery as it is now.
fn change_gallery(change: impl FnOnce(&Config) -> anyhow::Result<()>) -> Reply {
    Config::load(&daemon().paths.config_file)
        .map_err(anyhow::Error::from)
        .and_then(|config| change(&config))
        .and_then(|()| gallery_now())
        .map_or_else(error, Reply::Gallery)
}

#[expect(clippy::needless_pass_by_value)] // Used as a `map_or_else` callback.
fn error(e: anyhow::Error) -> Reply {
    Reply::Error {
        message: format!("{e:#}"),
    }
}

fn handle_change(change: Change) -> anyhow::Result<()> {
    match change {
        Change::Space => {
            if let Some(mtm) = MainThreadMarker::new() {
                desktop::screens_changed(mtm);
                screen_images::refresh(mtm);
            }
            let config = Config::load(&daemon().paths.config_file)?;
            let shown = macos::current_wallpaper()?;
            let chosen =
                current().and_then(|s| (s.fixed_color.is_none()).then_some(s.wallpaper).flatten());
            match chosen {
                // macOS only changes the Space in view, so bring this one up
                // to date now that it is visible.
                Some(chosen) if config.all_spaces && chosen != shown => {
                    tracing::info!("bringing this Space to {}", chosen.display());
                    macos::set_wallpaper(&chosen)
                }
                _ => handle_change(Change::Wallpaper),
            }
        }
        Change::Wallpaper => {
            let path = macos::current_wallpaper()?;
            if current().is_some_and(|s| s.wallpaper.as_deref() == Some(path.as_path())) {
                return Ok(());
            }
            tracing::info!("wallpaper is now {}", path.display());
            run_apply(None, Some(&path)).map(drop)
        }
        Change::Appearance => {
            let mode = macos::system_mode();
            if current().is_some_and(|s| s.mode == mode) {
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
    publish(&applied, &Config::load(&paths.config_file)?);
    Ok(applied)
}

/// Records what is on screen and tells subscribers.
fn publish(applied: &Applied, config: &Config) {
    let state = state_from(
        applied.report.wallpaper.clone(),
        applied.report.mode,
        &applied.palette,
        config,
    );
    if let Some(mtm) = MainThreadMarker::new() {
        desktop::apply_settings(config.finish, config.lid_animation, mtm);
        // Ready before the next transition or lid movement, so neither
        // waits on decoding.
        screen_images::refresh(mtm);
    }
    *daemon().current.lock().expect("state lock") = Some(state.clone());
    *daemon().config_text.lock().expect("config lock") =
        std::fs::read_to_string(&daemon().paths.config_file).unwrap_or_default();
    broadcast(&Reply::State(Box::new(state)));
}

fn state_from(
    wallpaper: Option<PathBuf>,
    mode: Mode,
    palette: &Palette,
    config: &Config,
) -> ipc::State {
    let fixed_color = match State::load(&daemon().paths.state_file()).source {
        Source::Wallpaper => None,
        Source::Color { color } => Some(color),
    };
    ipc::State {
        version: env!("CARGO_PKG_VERSION").to_owned(),
        configured: daemon().paths.config_file.exists(),
        wallpaper,
        fixed_color,
        mode,
        palette: palette.clone(),
        transition: config.transition.style,
        duration: config.transition.duration,
        scheme: config.scheme,
        contrast: config.contrast,
        wallpaper_dirs: config.wallpaper_dirs.clone(),
        all_spaces: config.all_spaces,
        lid_animation: config.lid_animation,
        finish: config.finish,
    }
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

/// Images indexed at once in the background. Indexing decodes each image at
/// full size (over 100 MB for a 6K wallpaper), so one at a time keeps the
/// daemon's memory near a single image; nobody waits on it.
const INDEX_WORKERS: usize = 1;
/// Results gathered before subscribers get the gallery again.
const ANNOUNCE_EVERY: usize = 4;

/// Indexes images on background threads, reporting each result to the main
/// thread.
fn start_indexing(todo: Vec<PathBuf>) {
    let queue = Mutex::new(todo);
    let spawned = std::thread::Builder::new()
        .name("obojeno-index".into())
        .spawn(move || {
            std::thread::scope(|scope| {
                for _ in 0..INDEX_WORKERS {
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
    let announce = work.indexing.is_empty() || work.unannounced >= ANNOUNCE_EVERY;
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

/// What is on screen; `None` until the first apply.
fn current() -> Option<ipc::State> {
    daemon().current.lock().expect("state lock").clone()
}

fn current_state() -> anyhow::Result<ipc::State> {
    current().context("not themed yet")
}

fn accept(listener: &UnixListener) {
    for stream in listener.incoming() {
        match stream {
            Ok(stream) => {
                let spawned = std::thread::Builder::new()
                    .name("obojeno-client".into())
                    .spawn(move || {
                        if let Err(e) = serve(&stream) {
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

fn serve(stream: &UnixStream) -> anyhow::Result<()> {
    let mut line = String::new();
    BufReader::new(stream).read_line(&mut line)?;
    let mut writer = stream;
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
            send(&mut writer, &Reply::State(Box::new(state)))?;
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

/// Writes one message as a line of JSON, the protocol's framing.
fn send(writer: &mut impl std::io::Write, message: &impl serde::Serialize) -> anyhow::Result<()> {
    let mut line = serde_json::to_vec(message)?;
    line.push(b'\n');
    writer.write_all(&line)?;
    writer.flush()?;
    Ok(())
}

/// Sends one request to a running daemon and returns its reply; an error
/// reply becomes an error. `None` when no daemon runs.
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
    send(&mut stream, request)?;
    let mut line = String::new();
    BufReader::new(&stream).read_line(&mut line)?;
    match serde_json::from_str(&line).context("bad reply from the daemon")? {
        Reply::Error { message } => bail!(message),
        reply => Ok(Some(reply)),
    }
}
