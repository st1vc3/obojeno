//! Windows drawn over the wallpaper: the finish (grain, rounded corners) and
//! the lid animation. Stacking, bottom to top: wallpaper, transition and lid
//! windows, finish windows, desktop icons, everything else.

use std::cell::RefCell;
use std::path::{Path, PathBuf};

use anyhow::Context as _;
use objc2::rc::Retained;
use objc2::runtime::{AnyClass, AnyObject};
use objc2::{MainThreadMarker, MainThreadOnly as _, msg_send};
use objc2_app_kit::{
    NSBackingStoreType, NSColor, NSScreen, NSWindow, NSWindowCollectionBehavior, NSWindowStyleMask,
    NSWorkspace,
};
use objc2_core_foundation::{CFRetained, CGFloat, CGPoint, CGRect, CGSize};
use objc2_core_graphics::{
    CGBitmapContextCreate, CGBitmapContextCreateImage, CGColor, CGColorSpace, CGDisplayIsBuiltin,
    CGImage, CGImageAlphaInfo, CGMutablePath, CGWindowLevelForKey, CGWindowLevelKey,
};
use objc2_foundation::{NSArray, NSNumber, ns_string};
use objc2_quartz_core::{
    CAGradientLayer, CALayer, CAShapeLayer, CATransaction, CATransform3D, kCAFilterNearest,
    kCAGradientLayerRadial,
};
use obojeno_core::config::Finish;

use crate::{imageio, transition};

/// How far above the wallpaper a window sits.
#[derive(Debug, Clone, Copy)]
pub enum Layer {
    /// Transitions and the lid animation: they show a wallpaper.
    Wallpaper,
    /// Grain and corners, over whatever wallpaper is showing.
    Finish,
    /// The lid's darkness, over the menu bar and everything else.
    Curtain,
}

impl Layer {
    fn level(self) -> isize {
        let (key, above) = match self {
            Self::Wallpaper => (CGWindowLevelKey::DesktopWindowLevelKey, 1),
            Self::Finish => (CGWindowLevelKey::DesktopWindowLevelKey, 2),
            Self::Curtain => (CGWindowLevelKey::StatusWindowLevelKey, 1),
        };
        CGWindowLevelForKey(key) as isize + above
    }
}

/// A borderless, click-through window on every Space at `layer`: just above
/// the wallpaper and below desktop icons and all other windows, except for
/// the curtain.
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
    window.setLevel(layer.level());
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
    /// Holds the wallpaper; sinks toward its bottom edge.
    sinking: Retained<CALayer>,
    /// Softens the edges of `sinking` more the further the lid closes.
    feather: Retained<CALayer>,
    wallpaper: Retained<CALayer>,
    /// Comes down from the top as the lid closes, over the menu bar too.
    curtain_window: Retained<NSWindow>,
    curtain: Retained<CAGradientLayer>,
    /// Darkens the edges of the screen.
    vignette: Retained<CAGradientLayer>,
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
    /// A previewed lid angle; the sensor is ignored while set.
    preview: Option<u16>,
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
            lid.curtain_window.close();
        }
        let screens = NSScreen::screens(mtm);
        if finish.grain > 0.0 || finish.corner_radius > 0.0 {
            overlays.finish = screens
                .iter()
                .map(|screen| finish_window(&screen, finish, mtm))
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

fn finish_window(screen: &NSScreen, finish: Finish, mtm: MainThreadMarker) -> Retained<NSWindow> {
    let frame = screen.frame();
    let window = overlay_window(frame, Layer::Finish, mtm);
    let bounds = CGRect::new(CGPoint::new(0.0, 0.0), frame.size);
    if let Some(root) = root_layer(&window) {
        CATransaction::begin();
        CATransaction::setDisableActions(true);
        let scale = screen.backingScaleFactor();
        let noise = (finish.grain > 0.0)
            .then(|| noise_image(bounds.size, scale))
            .flatten();
        if let Some(noise) = noise {
            let grain = CALayer::new();
            grain.setFrame(bounds);
            // One noise pixel per device pixel: no tiling, no scaling.
            // SAFETY: a CGImage is valid layer contents.
            unsafe { grain.setContents(Some(transition::cf_object(&noise))) };
            grain.setContentsScale(scale);
            // SAFETY: the filter name is a valid constant.
            grain.setMagnificationFilter(unsafe { kCAFilterNearest });
            // Full strength is a quarter-opaque noise layer; more turns the
            // wallpaper to static.
            #[allow(clippy::cast_possible_truncation)] // An opacity.
            grain.setOpacity((finish.grain.clamp(0.0, 1.0) * 0.25) as f32);
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
    // Each corner is a mask from just outside the screen to the rounded
    // curve. Reaching past the edges keeps them from being anti-aliased,
    // which left a hairline of wallpaper along the edge. `dx`/`dy` point
    // from the corner into the screen; (0, 0) is the bottom left.
    const BLEED: CGFloat = 2.0;
    let corner = |(cx, cy): (CGFloat, CGFloat), dx: CGFloat, dy: CGFloat| {
        let (out_x, out_y) = (cx - dx * BLEED, cy - dy * BLEED);
        // SAFETY: null transforms are allowed.
        unsafe {
            let p = Some(&*path);
            let none = std::ptr::null();
            CGMutablePath::move_to_point(p, none, out_x, out_y);
            CGMutablePath::add_line_to_point(p, none, out_x, cy + dy * r);
            CGMutablePath::add_line_to_point(p, none, cx, cy + dy * r);
            // The curve, tangent to both edges, ending r along the other.
            CGMutablePath::add_arc_to_point(p, none, cx, cy, cx + dx * r, cy, r);
            CGMutablePath::add_line_to_point(p, none, cx + dx * r, out_y);
            CGMutablePath::close_subpath(p);
        }
    };
    corner((0.0, 0.0), 1.0, 1.0);
    corner((w, 0.0), -1.0, 1.0);
    if top_too {
        corner((0.0, h), 1.0, -1.0);
        corner((w, h), -1.0, -1.0);
    }
    path
}

/// Grayscale noise the size of a screen in device pixels. Each value is
/// the mean of two random bytes, which clusters them around mid-gray for a
/// softer, film-like grain than uniform noise.
fn noise_image(size: CGSize, scale: CGFloat) -> Option<CFRetained<CGImage>> {
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)] // Screen sizes.
    let (width, height) = (
        (size.width * scale).round() as usize,
        (size.height * scale).round() as usize,
    );
    if width == 0 || height == 0 {
        return None;
    }
    let mut pixels = vec![0u8; width * height];
    // xorshift: any decent noise will do, and it needs no dependency.
    let mut x: u32 = 0x9E37_79B9;
    for px in &mut pixels {
        x ^= x << 13;
        x ^= x >> 17;
        x ^= x << 5;
        let [a, b, ..] = x.to_le_bytes();
        *px = u8::try_from((u16::from(a) + u16::from(b)) / 2).unwrap_or(u8::MAX);
    }
    let gray = CGColorSpace::new_device_gray()?;
    // SAFETY: `pixels` is `height` rows of `width` bytes and outlives the
    // context, which is dropped below; the image copies the pixels.
    unsafe {
        let context = CGBitmapContextCreate(
            pixels.as_mut_ptr().cast(),
            width,
            height,
            8,
            width,
            Some(&gray),
            CGImageAlphaInfo::None.0,
        )?;
        CGBitmapContextCreateImage(Some(&context))
    }
}

/// How much of each side `feather_image` fades out.
const FEATHER: CGFloat = 0.2;

/// A mask that is opaque in the middle and fades to clear over the outer
/// `FEATHER` of each side. Small, since it is smooth when stretched.
fn feather_image() -> Option<CFRetained<CGImage>> {
    const W: usize = 320;
    const H: usize = 200;
    // 0 at an edge, 1 from `FEATHER` inward, eased.
    let ramp = |i: usize, n: usize| {
        #[allow(clippy::cast_precision_loss)] // Small sizes.
        let from_edge = (i.min(n - 1 - i) as f64 + 0.5) / n as f64;
        let x = (from_edge / FEATHER).min(1.0);
        x * x * (3.0 - 2.0 * x)
    };
    let mut pixels = vec![0u8; W * H];
    for (y, row) in pixels.as_chunks_mut::<W>().0.iter_mut().enumerate() {
        let vertical = ramp(y, H);
        for (x, px) in row.iter_mut().enumerate() {
            #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)] // 0..=255.
            {
                *px = (ramp(x, W) * vertical * 255.0).round() as u8;
            }
        }
    }
    // SAFETY: `pixels` is `H` rows of `W` bytes and outlives the context,
    // which is dropped below; the image copies the pixels.
    unsafe {
        let context = CGBitmapContextCreate(
            pixels.as_mut_ptr().cast(),
            W,
            H,
            8,
            W,
            None,
            CGImageAlphaInfo::Only.0,
        )?;
        CGBitmapContextCreateImage(Some(&context))
    }
}

/// Where the feather mask goes at progress `t`: at 0 its fading edges lie
/// outside `bounds`, then they move in.
fn feather_frame(bounds: CGRect, t: CGFloat) -> CGRect {
    let outside = 1.0 / (1.0 - 2.0 * FEATHER) + 0.02;
    let k = outside + (1.0 - outside) * t;
    let (w, h) = (bounds.size.width * k, bounds.size.height * k);
    CGRect::new(
        CGPoint::new(
            (bounds.size.width - w) / 2.0,
            (bounds.size.height - h) / 2.0,
        ),
        CGSize::new(w, h),
    )
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
        m34: -1.0 / 900.0,
        ..identity()
    });
    let sinking = CALayer::new();
    sinking.setAnchorPoint(CGPoint::new(0.5, 0.0));
    sinking.setBounds(bounds);
    sinking.setPosition(CGPoint::new(bounds.size.width / 2.0, 0.0));
    let feather = CALayer::new();
    feather.setFrame(feather_frame(bounds, 0.0));
    let image = feather_image()?;
    // SAFETY: a CGImage is valid layer contents.
    unsafe { feather.setContents(Some(transition::cf_object(&image))) };
    // SAFETY: any layer is a valid mask.
    unsafe { sinking.setMask(Some(&feather)) };
    let wallpaper = CALayer::new();
    wallpaper.setFrame(bounds);
    wallpaper.setContentsScale(screen.backingScaleFactor());
    sinking.addSublayer(&wallpaper);
    let black = NSColor::blackColor().CGColor();
    let clear = NSColor::clearColor().CGColor();
    let vignette = CAGradientLayer::new();
    vignette.setFrame(bounds);
    // SAFETY: the type is a valid constant.
    vignette.setType(unsafe { kCAGradientLayerRadial });
    vignette.setStartPoint(CGPoint::new(0.5, 0.35));
    vignette.setEndPoint(CGPoint::new(1.15, 1.15));
    // SAFETY: an array of CGColors, as `colors` expects.
    unsafe {
        vignette.setColors(Some(&NSArray::from_retained_slice(&[
            cf_retained(&clear),
            cf_retained(&black),
        ])))
    };
    vignette.setOpacity(0.0);
    let shade = CALayer::new();
    shade.setFrame(bounds);
    shade.setBackgroundColor(Some(&black));
    shade.setOpacity(0.0);
    root.addSublayer(&sinking);
    root.addSublayer(&vignette);
    root.addSublayer(&shade);
    CATransaction::commit();

    let blur = gaussian_blur()?;
    let curtain_window = overlay_window(frame, Layer::Curtain, mtm);
    CATransaction::begin();
    CATransaction::setDisableActions(true);
    let curtain = CAGradientLayer::new();
    curtain.setFrame(bounds);
    // Clear below the locations, black above: in layer coordinates the
    // gradient runs bottom (0) to top (1).
    curtain.setStartPoint(CGPoint::new(0.5, 0.0));
    curtain.setEndPoint(CGPoint::new(0.5, 1.0));
    // SAFETY: an array of CGColors, as `colors` expects.
    unsafe {
        curtain.setColors(Some(&NSArray::from_retained_slice(&[
            cf_retained(&clear),
            cf_retained(&black),
        ])))
    };
    curtain.setLocations(Some(&locations(1.0, 1.0)));
    curtain_window.contentView()?.layer()?.addSublayer(&curtain);
    CATransaction::commit();
    Some(LidWindow {
        window,
        sinking,
        feather,
        wallpaper,
        curtain_window,
        curtain,
        vignette,
        shade,
        blur,
        showing: None,
        visible: false,
    })
}

/// Gradient stop positions as the NSNumbers Core Animation wants.
fn locations(a: CGFloat, b: CGFloat) -> Retained<NSArray<NSNumber>> {
    NSArray::from_retained_slice(&[NSNumber::new_f64(a), NSNumber::new_f64(b)])
}

/// A CGColor as an object for `colors` arrays.
fn cf_retained(color: &CGColor) -> Retained<AnyObject> {
    transition::retain_cf(color)
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

/// Follows the lid and turns angles into the effect's progress: 0 at rest,
/// 1 nearly shut. The effect is relative to where the lid usually rests,
/// so it starts as soon as the lid begins to close from any working angle.
#[derive(Debug, Default)]
pub struct LidTracker {
    /// The angle the lid rests at, once known.
    rest: Option<f64>,
    /// A lower angle the lid has stayed at, and since when.
    settling: Option<(u16, std::time::Instant)>,
}

impl LidTracker {
    /// How far below the resting angle the effect starts.
    const SLACK: f64 = 6.0;
    /// The effect never starts above this, so shifting a lid that rests
    /// wide open, on a couch or in bed, does not set it off.
    const HIGHEST_START: f64 = 90.0;
    /// The angle at which the effect is complete.
    const SHUT: f64 = 8.0;
    /// Below this, the lid is closing rather than resting.
    const LOWEST_REST: u16 = 45;
    /// How long the lid has to stay put at a lower angle to rest there.
    const SETTLE: std::time::Duration = std::time::Duration::from_secs(3);

    /// Feeds a reading taken at `now` and returns the progress.
    pub fn update(&mut self, angle: u16, now: std::time::Instant) -> f64 {
        let value = f64::from(angle);
        match self.rest {
            // Raising the lid rests it higher right away.
            Some(rest) if value <= rest => {
                if angle >= Self::LOWEST_REST {
                    match self.settling {
                        Some((at, since)) if at == angle => {
                            if now.duration_since(since) >= Self::SETTLE {
                                self.rest = Some(value);
                                self.settling = None;
                            }
                        }
                        _ => self.settling = Some((angle, now)),
                    }
                } else {
                    self.settling = None;
                }
            }
            _ => {
                self.rest = Some(value.max(f64::from(Self::LOWEST_REST)));
                self.settling = None;
            }
        }
        self.progress(value)
    }

    fn progress(&self, angle: f64) -> f64 {
        let rest = self.rest.unwrap_or(f64::from(Self::LOWEST_REST));
        let start = (rest - Self::SLACK).min(Self::HIGHEST_START);
        ((start - angle) / (start - Self::SHUT)).clamp(0.0, 1.0)
    }
}

/// Shows the effect at `percent` regardless of the sensor, or follows the
/// sensor again with `None`.
pub fn preview_lid(percent: Option<u16>, mtm: MainThreadMarker) {
    OVERLAYS.with_borrow_mut(|o| o.preview = percent);
    show_lid(percent.map_or(0.0, |p| f64::from(p.min(100)) / 100.0), mtm);
}

/// The sensor's progress; ignored while a preview is shown.
pub fn lid_progress(t: f64, mtm: MainThreadMarker) {
    if OVERLAYS.with_borrow(|o| o.preview.is_none()) {
        show_lid(t, mtm);
    }
}

/// Shows the effect at progress `t`: darkness comes down from the top like
/// a curtain while the wallpaper blurs and sinks a little toward the
/// bottom. Main thread only.
fn show_lid(t: f64, mtm: MainThreadMarker) {
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
                    lid.curtain_window.orderOut(None);
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
                lid.curtain_window.orderFrontRegardless();
                lid.visible = true;
            }
            CATransaction::begin();
            // Short linear steps between sensor readings keep it fluid.
            CATransaction::setAnimationDuration(0.06);
            // Only a hint of tipping away (negative rotation; positive
            // brings the top toward you); mostly it sinks toward the
            // bottom edge it is anchored at.
            let tilt = CATransform3D::new_rotation(-t * 10.0_f64.to_radians(), 1.0, 0.0, 0.0);
            let shrink = 1.0 - 0.14 * t;
            lid.sinking.setTransform(tilt.scale(shrink, shrink, 1.0));
            lid.feather.setFrame(feather_frame(lid.sinking.bounds(), t));
            // A wide, soft curtain: fully open at 0, only a glow at the
            // bottom at 1.
            let edge = 1.5 - 1.25 * t;
            lid.curtain.setLocations(Some(&locations(edge - 0.5, edge)));
            #[allow(clippy::cast_possible_truncation)] // Opacities.
            {
                lid.vignette.setOpacity((t * 0.7) as f32);
                lid.shade.setOpacity((t * 0.25) as f32);
            }
            let radius = NSNumber::new_f64(t * 60.0);
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

/// Polls the lid sensor on a background thread and forwards the progress
/// to `lid_progress` on the main thread. Fast while the lid moves or the
/// effect shows, slow while it rests.
pub fn watch_lid() {
    let Some(sensor) = crate::lid::LidSensor::open() else {
        tracing::info!("no lid sensor; the lid animation is off");
        return;
    };
    let spawned =
        std::thread::Builder::new()
            .name("obojeno-lid".into())
            .spawn(move || {
                let mut tracker = LidTracker::default();
                let mut last: Option<u16> = None;
                let mut shown = 0.0;
                loop {
                    let angle = sensor.angle();
                    if let Some(angle) = angle {
                        let t = tracker.update(angle, std::time::Instant::now());
                        if (t - shown).abs() > f64::EPSILON {
                            shown = t;
                            dispatch2::DispatchQueue::main().exec_async(move || {
                                if let Some(mtm) = MainThreadMarker::new() {
                                    lid_progress(t, mtm);
                                }
                            });
                        }
                    }
                    let moving = angle != last;
                    last = angle;
                    std::thread::sleep(std::time::Duration::from_millis(
                        if moving || shown > 0.0 { 33 } else { 250 },
                    ));
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
    fn lid_effect_starts_below_the_resting_angle() {
        let now = std::time::Instant::now();
        let mut lid = LidTracker::default();
        assert!(lid.update(101, now) < f64::EPSILON);
        assert!(lid.update(96, now) < f64::EPSILON, "within the slack");
        let half = lid.update(49, now);
        assert!((half - 41.0 / 82.0).abs() < 1e-9, "{half}");
        assert!(lid.update(110, now) < f64::EPSILON);
        assert!(lid.update(92, now) < f64::EPSILON, "never above 90°");
        assert!(lid.update(89, now) > 0.0);
        assert!((lid.update(8, now) - 1.0).abs() < f64::EPSILON);
        // Opening again reveals it back up to where it rested.
        assert!(lid.update(95, now) < f64::EPSILON);
    }

    #[test]
    fn lid_rests_lower_only_after_staying_there() {
        let start = std::time::Instant::now();
        let later = |s| start + std::time::Duration::from_secs(s);
        let mut lid = LidTracker::default();
        lid.update(110, start);
        assert!(lid.update(80, later(1)) > 0.0);
        assert!(lid.update(80, later(3)) > 0.0, "not settled yet");
        assert!(lid.update(80, later(4)) < f64::EPSILON, "settled at 80°");
        assert!(lid.update(70, later(5)) > 0.0);
        // Held halfway shut, it is still closing, not resting.
        lid.update(30, later(6));
        assert!((lid.update(30, later(60)) - lid.update(30, later(61))).abs() < f64::EPSILON);
        assert!(lid.update(30, later(62)) > 0.5);
    }

    #[test]
    fn feather_starts_outside_the_screen_and_moves_in() {
        let bounds = CGRect::new(CGPoint::new(0.0, 0.0), CGSize::new(1000.0, 600.0));
        let open = feather_frame(bounds, 0.0);
        let inset = open.size.width * FEATHER;
        assert!(open.origin.x + inset < 0.0, "fully opaque over the screen");
        let shut = feather_frame(bounds, 1.0);
        assert!((shut.size.width - 1000.0).abs() < 1e-9);
        assert!(feather_image().is_some());
    }

    #[test]
    fn lid_first_seen_closing_assumes_the_lowest_rest() {
        let now = std::time::Instant::now();
        let mut lid = LidTracker::default();
        let t = lid.update(20, now);
        assert!((t - 19.0 / 31.0).abs() < 1e-9, "{t}");
        assert!(lid.update(40, now) < f64::EPSILON);
    }
}

#[cfg(test)]
mod corner_tests {
    use super::*;
    use objc2_core_graphics::CGContext;

    /// Renders the corner masks into a `w`×`h` alpha bitmap (row 0 at the
    /// bottom, like the layer).
    fn render(w: usize, h: usize, radius: CGFloat, top_too: bool) -> Vec<u8> {
        let mut pixels = vec![0u8; w * h * 4];
        let space = CGColorSpace::new_device_rgb().unwrap();
        #[allow(clippy::cast_precision_loss)]
        let size = CGSize::new(w as CGFloat, h as CGFloat);
        let path = corner_path(size, radius, top_too);
        // SAFETY: the buffer is h rows of w * 4 bytes and outlives the context.
        let context = unsafe {
            CGBitmapContextCreate(
                pixels.as_mut_ptr().cast(),
                w,
                h,
                8,
                w * 4,
                Some(&space),
                CGImageAlphaInfo::PremultipliedLast.0,
            )
        }
        .unwrap();
        CGContext::set_rgb_fill_color(Some(&context), 0.0, 0.0, 0.0, 1.0);
        CGContext::add_path(Some(&context), Some(&path));
        CGContext::fill_path(Some(&context));
        drop(context);
        // The bitmap's first row is the top; flip to bottom-up.
        let rows: Vec<_> = pixels
            .chunks(w * 4)
            .rev()
            .flat_map(|row| row.iter().skip(3).step_by(4).copied())
            .collect();
        rows
    }

    #[test]
    fn corners_cover_the_screen_edges_completely() {
        let (w, h) = (120, 80);
        let alpha = render(w, h, 12.0, false);
        let at = |x: usize, y: usize| alpha[y * w + x];
        for i in 0..6 {
            assert_eq!(at(w - 1, i), 255, "right edge, row {i}");
            assert_eq!(at(0, i), 255, "left edge, row {i}");
            assert_eq!(
                at(w - 1 - i, 0),
                255,
                "bottom edge near the right, column {i}"
            );
            assert_eq!(at(i, 0), 255, "bottom edge near the left, column {i}");
        }
        assert_eq!(at(w / 2, h / 2), 0, "the middle stays clear");
        assert_eq!(
            at(w - 1, h - 1),
            0,
            "top corners are left alone on the built-in screen"
        );
        assert_eq!(at(w / 2, 0), 0, "only the corners are masked");
    }

    #[test]
    fn other_displays_get_all_four_corners() {
        let (w, h) = (120, 80);
        let alpha = render(w, h, 12.0, true);
        assert_eq!(alpha[(h - 1) * w + (w - 1)], 255);
        assert_eq!(alpha[(h - 1) * w], 255);
    }
}
