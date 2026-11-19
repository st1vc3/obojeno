//! Noticing changes the daemon reacts to: the wallpaper (System Settings,
//! Space switches, screens), the appearance and the config file.

use std::path::{Path, PathBuf};
use std::ptr::NonNull;
use std::sync::mpsc;
use std::time::Duration;

use anyhow::Context as _;
use block2::RcBlock;
use notify::{RecursiveMode, Watcher as _};
use objc2::rc::Retained;
use objc2::runtime::ProtocolObject;
use objc2_app_kit::{
    NSApplicationDidChangeScreenParametersNotification, NSWorkspace,
    NSWorkspaceActiveSpaceDidChangeNotification,
};
use objc2_foundation::{
    NSDistributedNotificationCenter, NSNotification, NSNotificationCenter, NSNotificationName,
    NSObjectProtocol, ns_string,
};

use crate::daemon::{self, Change, Job};

/// Bursts of file events (macOS rewrites the wallpaper store several times
/// per change) collapse into one change after this much quiet.
const DEBOUNCE: Duration = Duration::from_millis(300);

/// Keeps the watchers alive; dropping it stops them.
pub struct Watchers {
    _files: notify::RecommendedWatcher,
    _observers: Vec<Retained<ProtocolObject<dyn NSObjectProtocol>>>,
}

pub fn start(config_file: &Path) -> anyhow::Result<Watchers> {
    Ok(Watchers {
        _files: watch_files(config_file)?,
        _observers: observe_notifications(),
    })
}

fn wallpaper_store() -> Option<PathBuf> {
    let home = std::env::var_os("HOME")?;
    Some(Path::new(&home).join("Library/Application Support/com.apple.wallpaper/Store"))
}

fn watch_files(config_file: &Path) -> anyhow::Result<notify::RecommendedWatcher> {
    let (tx, rx) = mpsc::channel::<Change>();
    let config_file = config_file.to_owned();
    let store = wallpaper_store();
    let classify = {
        let config_file = config_file.clone();
        let store = store.clone();
        move |path: &Path| {
            if path == config_file {
                Some(Change::Config)
            } else if store.as_deref().is_some_and(|s| path.starts_with(s)) {
                Some(Change::Wallpaper)
            } else {
                None
            }
        }
    };
    let mut watcher =
        notify::recommended_watcher(move |event: notify::Result<notify::Event>| match event {
            Ok(event) => {
                for change in event.paths.iter().filter_map(|p| classify(p)) {
                    let _ = tx.send(change);
                }
            }
            Err(e) => tracing::warn!("file watcher: {e}"),
        })?;
    // The folder, not the file: editors and obojeno replace files by rename.
    if let Some(dir) = config_file.parent() {
        std::fs::create_dir_all(dir).with_context(|| format!("creating {}", dir.display()))?;
        watcher.watch(dir, RecursiveMode::NonRecursive)?;
    }
    match &store {
        Some(store) if store.is_dir() => watcher.watch(store, RecursiveMode::NonRecursive)?,
        _ => tracing::warn!(
            "no wallpaper store found; wallpaper changes from System Settings go unnoticed"
        ),
    }
    std::thread::Builder::new()
        .name("obojeno-watch".into())
        .spawn(move || debounce(&rx))?;
    Ok(watcher)
}

/// Forwards each kind of change once things have been quiet for a moment.
fn debounce(rx: &mpsc::Receiver<Change>) {
    while let Ok(first) = rx.recv() {
        let mut pending = vec![first];
        while let Ok(change) = rx.recv_timeout(DEBOUNCE) {
            if !pending.contains(&change) {
                pending.push(change);
            }
        }
        for change in pending {
            daemon::submit(Job::Change(change));
        }
    }
}

fn observe_notifications() -> Vec<Retained<ProtocolObject<dyn NSObjectProtocol>>> {
    let workspace_center = NSWorkspace::sharedWorkspace().notificationCenter();
    let app_center = NSNotificationCenter::defaultCenter();
    let distributed = NSDistributedNotificationCenter::defaultCenter();
    // SAFETY: the names are valid AppKit constants.
    let (space, screens) = unsafe {
        (
            NSWorkspaceActiveSpaceDidChangeNotification,
            NSApplicationDidChangeScreenParametersNotification,
        )
    };
    vec![
        // Each Space can have its own wallpaper.
        observe(&workspace_center, space, Change::Wallpaper),
        observe(&app_center, screens, Change::Wallpaper),
        // Posted after the switch, including the automatic one at sunset.
        observe(
            &distributed,
            ns_string!("AppleInterfaceThemeChangedNotification"),
            Change::Appearance,
        ),
    ]
}

fn observe(
    center: &NSNotificationCenter,
    name: &NSNotificationName,
    change: Change,
) -> Retained<ProtocolObject<dyn NSObjectProtocol>> {
    let block = RcBlock::new(move |_: NonNull<NSNotification>| {
        // Let macOS finish updating before the daemon reads the new state.
        std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(150));
            daemon::submit(Job::Change(change));
        });
    });
    // SAFETY: the block only captures `Copy` data and is retained by the
    // center for as long as the observer token lives.
    unsafe { center.addObserverForName_object_queue_usingBlock(Some(name), None, None, &block) }
}
