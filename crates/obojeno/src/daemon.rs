//! `obojeno daemon`: the long-running process that owns theming.
//!
//! Clients talk to it over a Unix socket (see `obojeno_core::ipc`). Socket
//! threads only parse and queue; all work runs one job at a time on the main
//! thread, which AppKit (transitions) requires anyway.

use std::collections::{HashSet, VecDeque};
use std::io::{BufRead as _, BufReader, Read as _};
use std::os::fd::AsRawFd as _;
use std::os::unix::fs::{DirBuilderExt as _, PermissionsExt as _};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Sender};
use std::sync::{Mutex, OnceLock};
use std::time::Duration;

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
    actions, desktop, gallery, imageio, lid_view, log_file, macos, screen_images, setup, watch,
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
    log_file::keep_bounded(&log_file::path(&paths.dirs.home));
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
    // Only this user may reach the socket: its folder is private, and so is
    // the socket itself (connecting needs write permission).
    if let Some(dir) = socket.parent() {
        std::fs::DirBuilder::new()
            .recursive(true)
            .mode(0o700)
            .create(dir)
            .with_context(|| format!("creating {}", dir.display()))?;
        std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700))
            .with_context(|| format!("restricting {}", dir.display()))?;
    }
    match std::fs::remove_file(socket) {
        Ok(()) => {}
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => return Err(e).with_context(|| format!("removing stale {}", socket.display())),
    }
    let listener =
        UnixListener::bind(socket).with_context(|| format!("binding {}", socket.display()))?;
    std::fs::set_permissions(socket, std::fs::Permissions::from_mode(0o600))
        .with_context(|| format!("restricting {}", socket.display()))?;
    Ok(listener)
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
        Request::SetUp { targets } => setup::link(paths, &targets).map_or_else(error, |results| {
            // The apps read the lines on their next reload.
            if results.iter().any(|r| r.file.is_some()) {
                let apply = Request::Apply { mode: None };
                if let Err(e) = actions::execute(paths, &apply).and_then(|applied| {
                    publish(&applied, &Config::load(&paths.config_file)?);
                    Ok(())
                }) {
                    tracing::warn!("theming after the setup: {e:#}");
                }
            }
            Reply::SetUp { results }
        }),
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

/// The longest request line: far beyond any real one (importing a thousand
/// images is about 100 KB), and a bound on what a client can make the
/// daemon hold.
const MAX_REQUEST: u64 = 1 << 20;
/// How long a client may take to send its request.
const REQUEST_TIMEOUT: Duration = Duration::from_secs(10);

/// Whether the other end of `stream` runs as this user, as the socket's
/// permissions already ensure; checked again in case they were changed.
fn same_user(stream: &UnixStream) -> bool {
    let (mut uid, mut gid) = (0, 0);
    // SAFETY: a connected socket and pointers to two writable ids.
    let ok = unsafe { libc::getpeereid(stream.as_raw_fd(), &raw mut uid, &raw mut gid) } == 0;
    // SAFETY: getuid cannot fail.
    ok && uid == unsafe { libc::getuid() }
}

/// Reads the one request line a client sends, within the limits.
fn read_request(stream: &UnixStream) -> anyhow::Result<String> {
    stream.set_read_timeout(Some(REQUEST_TIMEOUT))?;
    let mut line = String::new();
    BufReader::new(stream.take(MAX_REQUEST)).read_line(&mut line)?;
    if !line.ends_with('\n') && line.len() as u64 >= MAX_REQUEST {
        bail!("request longer than {MAX_REQUEST} bytes");
    }
    stream.set_read_timeout(None)?;
    Ok(line)
}

fn serve(stream: &UnixStream) -> anyhow::Result<()> {
    if !same_user(stream) {
        bail!("refused a client of another user");
    }
    let mut writer = stream;
    let line = match read_request(stream) {
        Ok(line) => line,
        Err(e) => {
            return send(
                &mut writer,
                &Reply::Error {
                    message: format!("bad request: {e:#}"),
                },
            );
        }
    };
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

#[cfg(test)]
mod tests {
    use std::io::Write as _;

    use super::*;

    fn mode(path: &Path) -> u32 {
        std::fs::metadata(path).unwrap().permissions().mode() & 0o777
    }

    #[test]
    fn only_this_user_can_reach_the_socket() {
        let tmp = tempfile::tempdir().unwrap();
        let socket = tmp.path().join("state/obojeno.sock");
        let _listener = bind(&socket).unwrap();
        assert_eq!(mode(socket.parent().unwrap()), 0o700);
        assert_eq!(mode(&socket), 0o600);
    }

    #[test]
    fn requests_are_one_bounded_line() {
        let (mut client, server) = UnixStream::pair().unwrap();
        assert!(same_user(&server));
        client.write_all(b"{\"cmd\":\"state\"}\n").unwrap();
        assert_eq!(read_request(&server).unwrap(), "{\"cmd\":\"state\"}\n");

        let (mut client, server) = UnixStream::pair().unwrap();
        let writer = std::thread::spawn(move || {
            // Stops when the daemon side closes after reading its limit.
            let chunk = vec![b'x'; 1 << 16];
            while client.write_all(&chunk).is_ok() {}
        });
        let err = read_request(&server).unwrap_err().to_string();
        assert!(err.contains("longer than"), "{err}");
        drop(server);
        writer.join().unwrap();
    }
}
