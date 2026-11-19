//! The bits of macOS obojeno talks to: wallpaper, appearance and processes.

use std::ffi::{CStr, c_int};
use std::path::{Path, PathBuf};

use anyhow::{Context as _, bail};
use objc2::MainThreadMarker;
use objc2_app_kit::{NSScreen, NSWorkspace};
use objc2_foundation::{NSFileManager, NSString, NSURL, NSUserDefaults, ns_string};
use obojeno_core::Mode;

fn main_thread() -> anyhow::Result<MainThreadMarker> {
    MainThreadMarker::new().context("AppKit calls must run on the main thread")
}

/// The wallpaper of the main screen's current Space.
pub fn current_wallpaper() -> anyhow::Result<PathBuf> {
    let mtm = main_thread()?;
    let screen = NSScreen::mainScreen(mtm).context("no screen attached")?;
    wallpaper_of(&screen)
}

/// The wallpaper of a screen's current Space.
pub fn wallpaper_of(screen: &NSScreen) -> anyhow::Result<PathBuf> {
    let url = NSWorkspace::sharedWorkspace()
        .desktopImageURLForScreen(screen)
        .with_context(|| format!("macOS reports no wallpaper for {}", screen.localizedName()))?;
    let path = url.path().context("the wallpaper is not a local file")?;
    Ok(PathBuf::from(path.to_string()))
}

/// Sets the wallpaper on every screen, keeping each screen's scaling options.
pub fn set_wallpaper(path: &Path) -> anyhow::Result<()> {
    let mtm = main_thread()?;
    let path = path
        .canonicalize()
        .with_context(|| format!("{} does not exist", path.display()))?;
    let url = NSURL::fileURLWithPath(&NSString::from_str(&path.to_string_lossy()));
    let workspace = NSWorkspace::sharedWorkspace();
    for screen in NSScreen::screens(mtm) {
        let options = workspace
            .desktopImageOptionsForScreen(&screen)
            .unwrap_or_default();
        // SAFETY: the options come from AppKit itself for this screen.
        unsafe { workspace.setDesktopImageURL_forScreen_options_error(&url, &screen, &options) }
            .map_err(|e| {
                anyhow::anyhow!(
                    "setting the wallpaper on {}: {}",
                    screen.localizedName(),
                    e.localizedDescription()
                )
            })?;
    }
    Ok(())
}

/// The appearance macOS currently shows. Also correct in Auto mode, where
/// macOS keeps `AppleInterfaceStyle` in sync with what is on screen.
pub fn system_mode() -> Mode {
    let style =
        NSUserDefaults::standardUserDefaults().stringForKey(ns_string!("AppleInterfaceStyle"));
    if style.is_some_and(|s| s.to_string().eq_ignore_ascii_case("dark")) {
        Mode::Dark
    } else {
        Mode::Light
    }
}

/// Moves a file to the Trash, where the user can still restore it.
pub fn trash(path: &Path) -> anyhow::Result<()> {
    let url = NSURL::fileURLWithPath(&NSString::from_str(&path.to_string_lossy()));
    NSFileManager::defaultManager()
        .trashItemAtURL_resultingItemURL_error(&url, None)
        .map_err(|e| {
            anyhow::anyhow!(
                "moving {} to the Trash: {}",
                path.display(),
                e.localizedDescription()
            )
        })
}

/// Switches macOS to light or dark: directly through the window server,
/// which needs no permission, or through System Events if that is not
/// available (macOS asks once for permission to control it).
pub fn set_system_mode(mode: Mode) -> anyhow::Result<()> {
    match skylight_appearance() {
        Some(appearance) if appearance.set(mode.is_dark()) => Ok(()),
        _ => set_system_mode_with_script(mode),
    }
}

/// The window server's own appearance switch, what System Settings uses.
/// Private, so looked up at runtime: a macOS without it falls back to
/// System Events.
struct SkylightAppearance {
    get: unsafe extern "C" fn() -> bool,
    set: unsafe extern "C" fn(bool),
}

impl SkylightAppearance {
    /// Switches and reports whether it took effect.
    fn set(&self, dark: bool) -> bool {
        // SAFETY: both functions take and return plain booleans.
        unsafe {
            (self.set)(dark);
            (self.get)() == dark
        }
    }
}

fn skylight_appearance() -> Option<SkylightAppearance> {
    // SAFETY: dlopen and dlsym with valid C strings; the framework stays
    // loaded for the life of the process (it is never closed).
    unsafe {
        let framework = libc::dlopen(
            c"/System/Library/PrivateFrameworks/SkyLight.framework/SkyLight".as_ptr(),
            libc::RTLD_LAZY,
        );
        if framework.is_null() {
            return None;
        }
        let get = libc::dlsym(framework, c"SLSGetAppearanceThemeLegacy".as_ptr());
        let set = libc::dlsym(framework, c"SLSSetAppearanceThemeLegacy".as_ptr());
        if get.is_null() || set.is_null() {
            return None;
        }
        Some(SkylightAppearance {
            get: std::mem::transmute::<*mut libc::c_void, unsafe extern "C" fn() -> bool>(get),
            set: std::mem::transmute::<*mut libc::c_void, unsafe extern "C" fn(bool)>(set),
        })
    }
}

fn set_system_mode_with_script(mode: Mode) -> anyhow::Result<()> {
    let script = format!(
        "tell application \"System Events\" to tell appearance preferences to set dark mode to {}",
        mode.is_dark()
    );
    // Not the reload timeout: the first run waits for the user to answer
    // the permission prompt.
    let output = std::process::Command::new("/usr/bin/osascript")
        .args(["-e", &script])
        .output()
        .context("running osascript")?;
    if output.status.success() {
        return Ok(());
    }
    let stderr = String::from_utf8_lossy(&output.stderr);
    if stderr.contains("-1743") {
        bail!(
            "not allowed to switch the appearance. Allow it under System Settings > Privacy & Security > \
             Automation (System Events)"
        );
    }
    bail!("switching to {mode} mode failed: {}", stderr.trim());
}

/// Pids of this user's processes whose executable is named `name`.
pub fn pids_named(name: &str) -> anyhow::Result<Vec<c_int>> {
    // SAFETY: a null buffer asks for the number of pids.
    let count = unsafe { libc::proc_listallpids(std::ptr::null_mut(), 0) };
    if count <= 0 {
        bail!("listing processes failed");
    }
    // Leave room for processes started between the two calls.
    let mut pids: Vec<c_int> = vec![0; usize::try_from(count)? + 64];
    let bytes = c_int::try_from(pids.len() * size_of::<c_int>())?;
    // SAFETY: the buffer is `bytes` long.
    let count = unsafe { libc::proc_listallpids(pids.as_mut_ptr().cast(), bytes) };
    if count <= 0 {
        bail!("listing processes failed");
    }
    pids.truncate(usize::try_from(count)?);

    let mut path = vec![0u8; usize::try_from(libc::PROC_PIDPATHINFO_MAXSIZE)?];
    let path_len = u32::try_from(path.len())?;
    Ok(pids
        .into_iter()
        .filter(|&pid| {
            // SAFETY: the buffer is `path_len` long; on success it holds a
            // NUL-terminated path. Fails for other users' processes.
            let n = unsafe { libc::proc_pidpath(pid, path.as_mut_ptr().cast(), path_len) };
            n > 0
                && CStr::from_bytes_until_nul(&path)
                    .ok()
                    .and_then(|p| {
                        Path::new(&*p.to_string_lossy())
                            .file_name()
                            .map(|f| same_name(&f.to_string_lossy(), name))
                    })
                    .unwrap_or(false)
        })
        .collect())
}

/// File names on macOS are stored decomposed (NFD: `U` + combining
/// diaeresis for `Ü`), while names typed in code are usually composed.
fn same_name(a: &str, b: &str) -> bool {
    use unicode_normalization::UnicodeNormalization;
    a.nfc().eq(b.nfc())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_compare_across_unicode_normalization() {
        assert!(same_name("U\u{308}bersicht", "\u{dc}bersicht"));
        assert!(same_name("kitty", "kitty"));
        assert!(!same_name("kitty", "kitten"));
    }

    #[test]
    fn finds_own_process() {
        let exe = std::env::current_exe().unwrap();
        let name = exe.file_name().unwrap().to_str().unwrap();
        let pids = pids_named(name).unwrap();
        assert!(
            pids.contains(&c_int::try_from(std::process::id()).unwrap()),
            "{pids:?}"
        );
        assert_eq!(
            pids_named("surely-no-process-has-this-name").unwrap(),
            Vec::<c_int>::new()
        );
    }
}
