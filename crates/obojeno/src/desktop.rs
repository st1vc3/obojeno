//! Windows drawn over the wallpaper: the finish (grain, rounded corners) and
//! the lid animation. Stacking, bottom to top: wallpaper, transition and lid
//! windows, finish windows, desktop icons, everything else.

use std::cell::RefCell;
use std::path::{Path, PathBuf};

use anyhow::Context as _;
use objc2::rc::Retained;
use objc2::runtime::{AnyClass, AnyObject};
use objc2::{AllocAnyThread as _, MainThreadMarker, MainThreadOnly as _, msg_send};
use objc2_app_kit::{
    NSBackingStoreType, NSColor, NSImage, NSScreen, NSWindow, NSWindowCollectionBehavior,
    NSWindowStyleMask, NSWorkspace,
};
use objc2_core_foundation::{CGFloat, CGPoint, CGRect, CGSize};
use objc2_core_graphics::{
    CGBitmapContextCreate, CGBitmapContextCreateImage, CGColorSpace, CGDisplayIsBuiltin, CGImage,
    CGImageAlphaInfo, CGMutablePath, CGWindowLevelForKey, CGWindowLevelKey,
};
use objc2_foundation::{NSArray, NSNumber, ns_string};
use objc2_quartz_core::{CALayer, CAShapeLayer, CATransaction, CATransform3D};
use obojeno_core::config::{Finish, Grain};

use crate::{imageio, transition};

/// How far above the wallpaper a window sits.
#[derive(Debug, Clone, Copy)]
pub enum Layer {
    /// Transitions and the lid animation: they show a wallpaper.
    Wallpaper = 1,
    /// Grain and corners, over whatever wallpaper is showing.
    Finish = 2,
}

/// A borderless, click-through window on every Space, `layer` above the
/// wallpaper and below desktop icons and all other windows.
pub fn overlay_window(frame: CGRect, layer: Layer, mtm: MainThreadMarker) -> Retained<NSWindow> {
    // SAFETY: a plain borderless window, closed explicitly and not released
    // on close, so the Retained stays valid.
    let window = unsafe {
        let window = NSWindow::initWithContentRect_styleMask_backing_defer(
            NSWindow::alloc(mtm),
            frame,
            NSWindowStyleMask::Borderless,
            NSBackingStoreType::Buffered,
            false,
        );
        window.setReleasedWhenClosed(false);
        window
    };
    let desktop = CGWindowLevelForKey(CGWindowLevelKey::DesktopWindowLevelKey);
    window.setLevel(desktop as isize + layer as isize);
    window.setCollectionBehavior(
        NSWindowCollectionBehavior::CanJoinAllSpaces
            | NSWindowCollectionBehavior::Stationary
            | NSWindowCollectionBehavior::IgnoresCycle,
    );
    window.setIgnoresMouseEvents(true);
    window.setHasShadow(false);
    window.setOpaque(false);
    window.setBackgroundColor(Some(&NSColor::clearColor()));
    if let Some(view) = window.contentView() {
        view.setWantsLayer(true);
    }
    window
}

fn root_layer(window: &NSWindow) -> Option<Retained<CALayer>> {
    window.contentView().and_then(|v| v.layer())
}

/// Whether a screen is a MacBook's built-in display.
pub fn is_builtin(screen: &NSScreen) -> bool {
    screen
        .deviceDescription()
        .objectForKey(ns_string!("NSScreenNumber"))
        .and_then(|n| n.downcast::<NSNumber>().ok())
        .is_some_and(|n| CGDisplayIsBuiltin(n.unsignedIntValue()))
}

struct LidWindow {
    window: Retained<NSWindow>,
    /// Pivots on the bottom edge.
    wallpaper: Retained<CALayer>,
    shade: Retained<CALayer>,
    blur: Retained<AnyObject>,
    /// The wallpaper the layer shows, to reload when it changes.
    showing: Option<PathBuf>,
    visible: bool,
}

#[derive(Default)]
struct Overlays {
    finish: Vec<Retained<NSWindow>>,
    lid: Vec<LidWindow>,
    /// The settings the windows were built for.
    settings: Option<(Finish, bool)>,
}

thread_local! {
    static OVERLAYS: RefCell<Overlays> = RefCell::default();
}

/// Builds the overlay windows for these settings, unless they are built
/// for them already.
pub fn apply_settings(finish: Finish, lid_animation: bool, mtm: MainThreadMarker) {
    let current = OVERLAYS.with_borrow(|o| o.settings);
    if current != Some((finish, lid_animation)) {
        rebuild(finish, lid_animation, mtm);
    }
}

/// Rebuilds the overlay windows for the screens attached now.
pub fn screens_changed(mtm: MainThreadMarker) {
    if let Some((finish, lid)) = OVERLAYS.with_borrow(|o| o.settings) {
        rebuild(finish, lid, mtm);
    }
}

fn rebuild(finish: Finish, lid_animation: bool, mtm: MainThreadMarker) {
    OVERLAYS.with_borrow_mut(|overlays| {
        for window in overlays.finish.drain(..) {
            window.close();
        }
        for lid in overlays.lid.drain(..) {
            lid.window.close();
        }
        let screens = NSScreen::screens(mtm);
        if finish.grain != Grain::Off || finish.corner_radius > 0.0 {
            let noise = noise_color();
            overlays.finish = screens
                .iter()
                .map(|screen| finish_window(&screen, finish, noise.as_deref(), mtm))
                .collect();
        }
        overlays.settings = Some((finish, lid_animation));
        if lid_animation {
            overlays.lid = screens
                .iter()
                .filter(|s| is_builtin(s))
                .filter_map(|screen| lid_window(&screen, mtm))
                .collect();
        }
    });
}

fn finish_window(
    screen: &NSScreen,
    finish: Finish,
    noise: Option<&NSColor>,
    mtm: MainThreadMarker,
) -> Retained<NSWindow> {
    let frame = screen.frame();
    let window = overlay_window(frame, Layer::Finish, mtm);
    let bounds = CGRect::new(CGPoint::new(0.0, 0.0), frame.size);
    if let Some(root) = root_layer(&window) {
        CATransaction::begin();
        CATransaction::setDisableActions(true);
        if let (Some(noise), true) = (noise, finish.grain != Grain::Off) {
            let grain = CALayer::new();
            grain.setFrame(bounds);
            grain.setBackgroundColor(Some(&noise.CGColor()));
            grain.setOpacity(match finish.grain {
                Grain::Subtle => 0.05,
                Grain::Strong => 0.11,
                Grain::Off => 0.0,
            });
            root.addSublayer(&grain);
        }
        if finish.corner_radius > 0.0 {
            let corners = CAShapeLayer::new();
            corners.setFrame(bounds);
            corners.setFillColor(Some(&NSColor::blackColor().CGColor()));
            // A MacBook's screen is already rounded at the top; match it at
            // the bottom. Other displays get all four.
            corners.setPath(Some(&corner_path(
                bounds.size,
                finish.corner_radius,
                !is_builtin(screen),
            )));
            root.addSublayer(&corners);
        }
        CATransaction::commit();
    }
    window.orderFrontRegardless();
    window
}

/// The black areas outside rounded corners.
fn corner_path(
    size: CGSize,
    radius: CGFloat,
    top_too: bool,
) -> objc2_core_foundation::CFRetained<CGMutablePath> {
    let (w, h, r) = (size.width, size.height, radius);
    let path = CGMutablePath::new();
    // Each corner: the corner point, along one edge, an arc tangent to both
    // edges, back. (0, 0) is the bottom left.
    let corner = |corner: (CGFloat, CGFloat), along_x: CGFloat, along_y: CGFloat| {
        // SAFETY: null transforms are allowed.
        unsafe {
            CGMutablePath::move_to_point(Some(&path), std::ptr::null(), corner.0, corner.1);
            CGMutablePath::add_line_to_point(Some(&path), std::ptr::null(), corner.0, along_y);
            CGMutablePath::add_arc_to_point(
                Some(&path),
                std::ptr::null(),
                corner.0,
                corner.1,
                along_x,
                corner.1,
                r,
            );
            CGMutablePath::close_subpath(Some(&path));
        }
    };
    corner((0.0, 0.0), r, r);
    corner((w, 0.0), w - r, r);
    if top_too {
        corner((0.0, h), r, h - r);
        corner((w, h), w - r, h - r);
    }
    path
}

/// A tile of monochrome noise as a pattern color, one pixel per device pixel
/// on Retina screens.
fn noise_color() -> Option<Retained<NSColor>> {
    const SIDE: usize = 128;
    let mut pixels = vec![0u8; SIDE * SIDE * 4];
    // xorshift: any decent noise will do, and it needs no dependency.
    let mut x: u32 = 0x9E37_79B9;
    for px in pixels.as_chunks_mut::<4>().0 {
        x ^= x << 13;
        x ^= x >> 17;
        x ^= x << 5;
        let v = x.to_le_bytes()[0];
        px.copy_from_slice(&[v, v, v, 255]);
    }
    let srgb = CGColorSpace::new_device_rgb()?;
    // SAFETY: `pixels` is SIDE rows of SIDE * 4 bytes and outlives the
    // context, which is dropped below; the image copies the pixels.
    let image = unsafe {
        let context = CGBitmapContextCreate(
            pixels.as_mut_ptr().cast(),
            SIDE,
            SIDE,
            8,
            SIDE * 4,
            Some(&srgb),
            CGImageAlphaInfo::NoneSkipLast.0,
        )?;
        CGBitmapContextCreateImage(Some(&context))?
    };
    #[allow(clippy::cast_precision_loss)] // 64 is exact.
    let points = (SIDE / 2) as CGFloat;
    let tile = NSImage::initWithCGImage_size(NSImage::alloc(), &image, CGSize::new(points, points));
    Some(NSColor::colorWithPatternImage(&tile))
}

fn lid_window(screen: &NSScreen, mtm: MainThreadMarker) -> Option<LidWindow> {
    let frame = screen.frame();
    let window = overlay_window(frame, Layer::Wallpaper, mtm);
    let view = window.contentView()?;
    // The blur is a Core Image filter on the layer.
    view.setLayerUsesCoreImageFilters(true);
    let root = view.layer()?;
    let bounds = CGRect::new(CGPoint::new(0.0, 0.0), frame.size);

    CATransaction::begin();
    CATransaction::setDisableActions(true);
    root.setBackgroundColor(Some(&NSColor::blackColor().CGColor()));
    // Perspective, so tipping back looks like it goes away from you.
    root.setSublayerTransform(CATransform3D {
        m34: -1.0 / 1400.0,
        ..identity()
    });
    let wallpaper = CALayer::new();
    wallpaper.setAnchorPoint(CGPoint::new(0.5, 0.0));
    wallpaper.setBounds(bounds);
    wallpaper.setPosition(CGPoint::new(bounds.size.width / 2.0, 0.0));
    wallpaper.setContentsScale(screen.backingScaleFactor());
    let shade = CALayer::new();
    shade.setFrame(bounds);
    shade.setBackgroundColor(Some(&NSColor::blackColor().CGColor()));
    shade.setOpacity(0.0);
    root.addSublayer(&wallpaper);
    root.addSublayer(&shade);
    CATransaction::commit();

    let blur = gaussian_blur()?;
    Some(LidWindow {
        window,
        wallpaper,
        shade,
        blur,
        showing: None,
        visible: false,
    })
}

fn identity() -> CATransform3D {
    CATransform3D {
        m11: 1.0,
        m12: 0.0,
        m13: 0.0,
        m14: 0.0,
        m21: 0.0,
        m22: 1.0,
        m23: 0.0,
        m24: 0.0,
        m31: 0.0,
        m32: 0.0,
        m33: 1.0,
        m34: 0.0,
        m41: 0.0,
        m42: 0.0,
        m43: 0.0,
        m44: 1.0,
    }
}

fn gaussian_blur() -> Option<Retained<AnyObject>> {
    let class = AnyClass::get(c"CIFilter")?;
    // SAFETY: +[CIFilter filterWithName:] returns an autoreleased filter or
    // nil; the name is a built-in filter.
    let filter: Option<Retained<AnyObject>> =
        unsafe { msg_send![class, filterWithName: ns_string!("CIGaussianBlur")] };
    filter
}

/// How far the lid effect has progressed: 0 open (at or beyond this
/// angle), 1 nearly shut.
fn progress(angle: u16) -> CGFloat {
    const START: CGFloat = 95.0;
    const END: CGFloat = 25.0;
    ((START - CGFloat::from(angle)) / (START - END)).clamp(0.0, 1.0)
}

/// Shows the lid angle: the wallpaper tips back, blurs and darkens as the
/// lid closes. Main thread only.
pub fn lid_angle(angle: u16, mtm: MainThreadMarker) {
    let t = progress(angle);
    OVERLAYS.with_borrow_mut(|overlays| {
        if overlays.lid.is_empty() {
            return;
        }
        let workspace = NSWorkspace::sharedWorkspace();
        let screens = NSScreen::screens(mtm);
        for lid in &mut overlays.lid {
            if t <= 0.0 {
                if lid.visible {
                    lid.window.orderOut(None);
                    lid.visible = false;
                }
                continue;
            }
            if !lid.visible {
                if let Some(screen) = screens.iter().find(|s| is_builtin(s))
                    && let Err(e) = show_wallpaper(lid, &workspace, &screen)
                {
                    tracing::warn!("lid animation: {e:#}");
                    continue;
                }
                lid.window.orderFrontRegardless();
                lid.visible = true;
            }
            CATransaction::begin();
            // Short linear steps between sensor readings keep it fluid.
            CATransaction::setAnimationDuration(0.06);
            lid.wallpaper.setTransform(CATransform3D::new_rotation(
                t * 70.0_f64.to_radians(),
                1.0,
                0.0,
                0.0,
            ));
            #[allow(clippy::cast_possible_truncation)] // An opacity.
            lid.shade.setOpacity((t * 0.85) as f32);
            let radius = NSNumber::new_f64(t * 30.0);
            // SAFETY: inputRadius takes an NSNumber.
            let _: () = unsafe {
                msg_send![&lid.blur, setValue: &*radius, forKey: ns_string!("inputRadius")]
            };
            let filters = NSArray::from_retained_slice(std::slice::from_ref(&lid.blur));
            // SAFETY: an array of CIFilters, as `filters` expects.
            unsafe { lid.wallpaper.setFilters(Some(&filters)) };
            CATransaction::commit();
        }
    });
}

/// Loads the wallpaper of `screen` into the lid layer, drawn like macOS
/// draws it.
fn show_wallpaper(
    lid: &mut LidWindow,
    workspace: &NSWorkspace,
    screen: &NSScreen,
) -> anyhow::Result<()> {
    let url = workspace
        .desktopImageURLForScreen(screen)
        .context("no wallpaper")?;
    let path = PathBuf::from(url.path().context("wallpaper is not a file")?.to_string());
    if lid.showing.as_deref() == Some(path.as_path()) {
        return Ok(());
    }
    let image: objc2_core_foundation::CFRetained<CGImage> =
        imageio::cg_image(Path::new(&path)).map_err(anyhow::Error::msg)?;
    let (gravity, fill) = transition::scaling(workspace, screen, &image);
    CATransaction::begin();
    CATransaction::setDisableActions(true);
    // SAFETY: a CGImage is valid layer contents.
    unsafe {
        lid.wallpaper
            .setContents(Some(transition::cf_object(&image)))
    };
    lid.wallpaper.setContentsGravity(gravity);
    lid.wallpaper.setBackgroundColor(fill.as_deref());
    CATransaction::commit();
    lid.showing = Some(path);
    Ok(())
}

/// Polls the lid sensor on a background thread and forwards changes to
/// `lid_angle` on the main thread. Fast while the lid moves or is low,
/// slow while it rests open.
pub fn watch_lid() {
    let Some(sensor) = crate::lid::LidSensor::open() else {
        tracing::info!("no lid sensor; the lid animation is off");
        return;
    };
    let spawned = std::thread::Builder::new()
        .name("obojeno-lid".into())
        .spawn(move || {
            let mut last: Option<u16> = None;
            loop {
                let angle = sensor.angle();
                if angle.is_some() && angle != last {
                    let value = angle.unwrap_or_default();
                    dispatch2::DispatchQueue::main().exec_async(move || {
                        if let Some(mtm) = MainThreadMarker::new() {
                            lid_angle(value, mtm);
                        }
                    });
                }
                let moving = angle != last;
                last = angle;
                let low = angle.is_some_and(|a| progress(a + 10) > 0.0);
                std::thread::sleep(std::time::Duration::from_millis(if moving || low {
                    33
                } else {
                    250
                }));
            }
        });
    if let Err(e) = spawned {
        tracing::warn!("cannot watch the lid: {e}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lid_progress_ramps_from_95_to_25_degrees() {
        assert!((progress(120) - 0.0).abs() < f64::EPSILON);
        assert!((progress(95) - 0.0).abs() < f64::EPSILON);
        assert!((progress(60) - 0.5).abs() < f64::EPSILON);
        assert!((progress(25) - 1.0).abs() < f64::EPSILON);
        assert!((progress(0) - 1.0).abs() < f64::EPSILON);
    }
}
