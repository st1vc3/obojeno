//! The wallpaper on each screen, decoded ahead of time at the size the
//! screen shows it: a transition starts by showing the current wallpaper,
//! and the lid animation shows it whenever the lid moves. Decoding a large
//! wallpaper takes a moment and a lot of memory, so it happens once, in the
//! background, after the wallpaper changes.

use std::cell::RefCell;
use std::path::PathBuf;

use image::RgbaImage;
use objc2::MainThreadMarker;
use objc2_app_kit::{NSScreen, NSWorkspace};
use objc2_core_foundation::CFRetained;
use objc2_io_surface::IOSurfaceRef;

use crate::imageio::LayerPixels;
use crate::transition::Placement;
use crate::{layers, lid_view, macos};

/// The small copy's longest side: enough for the lid's blurred layers.
pub const SMALL_SIDE: u32 = 480;

/// A wallpaper decoded for one screen.
pub struct ScreenImage {
    pub path: PathBuf,
    /// The longest side it was decoded at, from its placement.
    side: u32,
    /// As layer contents.
    pub image: CFRetained<IOSurfaceRef>,
    /// A small copy, for blurring.
    pub small: RgbaImage,
}

#[derive(Default)]
struct Images {
    /// By display.
    ready: Vec<(u32, ScreenImage)>,
    /// Being decoded: display, path and side.
    loading: Vec<(u32, PathBuf, u32)>,
}

thread_local! {
    static IMAGES: RefCell<Images> = RefCell::default();
}

/// Decodes the wallpaper of every screen that does not have it ready, and
/// forgets screens that are gone.
pub fn refresh(mtm: MainThreadMarker) {
    let workspace = NSWorkspace::sharedWorkspace();
    let screens = NSScreen::screens(mtm);
    let attached: Vec<u32> = screens
        .iter()
        .filter_map(|s| layers::display_id(&s))
        .collect();
    IMAGES.with_borrow_mut(|images| images.ready.retain(|(id, _)| attached.contains(id)));
    for screen in &screens {
        let Some(id) = layers::display_id(&screen) else {
            continue;
        };
        let Ok(path) = macos::wallpaper_of(&screen) else {
            continue;
        };
        let side = match Placement::of(&workspace, &screen, &path) {
            Ok(placement) => placement.needed_side(),
            Err(e) => {
                tracing::warn!("{}: {e:#}", path.display());
                continue;
            }
        };
        let start = IMAGES.with_borrow_mut(|images| {
            let wanted = |p: &PathBuf, s: u32| *p == path && s == side;
            let ready = images
                .ready
                .iter()
                .any(|(i, r)| *i == id && wanted(&r.path, r.side));
            let loading = images
                .loading
                .iter()
                .any(|(i, p, s)| *i == id && wanted(p, *s));
            if ready || loading {
                return false;
            }
            images.loading.push((id, path.clone(), side));
            true
        });
        if start {
            std::thread::spawn(move || {
                let decoded = LayerPixels::decode(&path, side).and_then(|pixels| {
                    let small = pixels.scaled(SMALL_SIDE)?.to_rgba();
                    Ok((pixels, small))
                });
                dispatch2::DispatchQueue::main().exec_async(move || {
                    if let Some(mtm) = MainThreadMarker::new() {
                        finish(id, path, side, decoded, mtm);
                    }
                });
            });
        }
    }
}

fn finish(
    id: u32,
    path: PathBuf,
    side: u32,
    decoded: Result<(LayerPixels, RgbaImage), String>,
    mtm: MainThreadMarker,
) {
    IMAGES.with_borrow_mut(|images| {
        images
            .loading
            .retain(|(i, p, s)| !(*i == id && *p == path && *s == side));
    });
    let (pixels, small) = match decoded {
        Ok(decoded) => decoded,
        Err(e) => {
            tracing::warn!("{}: {e}", path.display());
            return;
        }
    };
    let Some(image) = pixels.into_surface() else {
        return;
    };
    IMAGES.with_borrow_mut(|images| {
        images.ready.retain(|(i, _)| *i != id);
        images.ready.push((
            id,
            ScreenImage {
                path,
                side,
                image,
                small,
            },
        ));
    });
    lid_view::wallpaper_ready(mtm);
}

/// Runs `f` with the decoded wallpaper of `screen`, if it is ready and
/// still the one shown there at the size it is shown.
pub fn with<T>(screen: &NSScreen, f: impl FnOnce(&ScreenImage) -> T) -> Option<T> {
    let id = layers::display_id(screen)?;
    let path = macos::wallpaper_of(screen).ok()?;
    let side = Placement::of(&NSWorkspace::sharedWorkspace(), screen, &path)
        .ok()?
        .needed_side();
    IMAGES.with_borrow(|images| {
        images
            .ready
            .iter()
            .find(|(i, r)| *i == id && r.path == path && r.side == side)
            .map(|(_, r)| f(r))
    })
}
