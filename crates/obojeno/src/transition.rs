//! Animated wallpaper changes.
//!
//! macOS swaps wallpapers instantly. To animate, every screen gets a window
//! just above the wallpaper (below desktop icons and all app windows) that
//! shows the old wallpaper, drawn with the screen's own scaling options. The
//! new wallpaper is set underneath, and the window animates away to reveal it.

use std::path::Path;
use std::time::{Duration, Instant};

use anyhow::Context as _;
use objc2::MainThreadMarker;
use objc2::rc::Retained;
use objc2::runtime::AnyObject;
use objc2_app_kit::{
    NSApplication, NSApplicationActivationPolicy, NSColor, NSEvent, NSImageScaling, NSScreen,
    NSWindow, NSWorkspace, NSWorkspaceDesktopImageAllowClippingKey,
    NSWorkspaceDesktopImageFillColorKey, NSWorkspaceDesktopImageScalingKey,
};
use objc2_core_foundation::{CFRunLoop, CGFloat, CGPoint, CGRect, CGSize, kCFRunLoopDefaultMode};
use objc2_core_graphics::{CGImage, CGMutablePath};
use objc2_foundation::{NSNumber, NSString, ns_string};
use objc2_quartz_core::{
    CABasicAnimation, CALayer, CALayerContentsGravity, CAMediaTiming as _, CAMediaTimingFunction,
    CAShapeLayer, CATransaction, kCAFillModeForwards, kCAFillRuleEvenOdd, kCAGravityCenter,
    kCAGravityResize, kCAGravityResizeAspect, kCAGravityResizeAspectFill,
    kCAMediaTimingFunctionEaseInEaseOut,
};
use obojeno_core::config::TransitionStyle;

use crate::{desktop, imageio};

/// Time for macOS to draw the new wallpaper under the overlay before it
/// starts to move.
const SETTLE: Duration = Duration::from_millis(300);
/// Time for the window server to show the overlay before the wallpaper
/// changes underneath it.
const SHOW: Duration = Duration::from_millis(50);

#[derive(Debug, Clone, Copy)]
pub struct Options {
    pub style: TransitionStyle,
    pub duration: Duration,
    /// Where grow-circle starts, in global screen coordinates (origin at the
    /// bottom left of the main screen). Defaults to the mouse pointer.
    pub origin: Option<CGPoint>,
}

/// Sets `path` as the wallpaper of every screen with an animated transition,
/// running `during` (the theming) at the same time so apps change color
/// together with the wallpaper.
pub fn set_wallpaper<T: Send>(
    path: &Path,
    options: &Options,
    during: impl FnOnce() -> T + Send,
) -> anyhow::Result<T> {
    let mtm = MainThreadMarker::new().context("AppKit calls must run on the main thread")?;
    if options.style == TransitionStyle::None || options.duration.is_zero() {
        crate::macos::set_wallpaper(path)?;
        return Ok(during());
    }

    // An accessory app has no Dock icon and never takes focus.
    NSApplication::sharedApplication(mtm)
        .setActivationPolicy(NSApplicationActivationPolicy::Accessory);
    let origin = options.origin.unwrap_or_else(NSEvent::mouseLocation);
    let workspace = NSWorkspace::sharedWorkspace();
    let overlays: Vec<Overlay> = NSScreen::screens(mtm)
        .iter()
        .filter_map(|screen| {
            Overlay::new(&screen, &workspace, mtm)
                .inspect_err(|e| {
                    tracing::warn!("no transition on {}: {e:#}", screen.localizedName())
                })
                .ok()
        })
        .collect();
    CATransaction::flush();
    pump(SHOW);

    crate::macos::set_wallpaper(path)?;
    let result = std::thread::scope(|scope| {
        let theming = scope.spawn(during);
        pump(SETTLE);
        for overlay in &overlays {
            overlay.animate(options, origin);
        }
        CATransaction::flush();
        pump(options.duration + Duration::from_millis(50));
        for overlay in &overlays {
            overlay.window.close();
        }
        theming.join()
    });
    Ok(result.unwrap_or_else(|panic| std::panic::resume_unwind(panic)))
}

/// Runs the main run loop so AppKit can draw, for `duration`.
fn pump(duration: Duration) {
    let deadline = Instant::now() + duration;
    loop {
        let left = deadline.saturating_duration_since(Instant::now());
        if left.is_zero() {
            break;
        }
        // SAFETY: the default mode is a valid constant.
        CFRunLoop::run_in_mode(unsafe { kCFRunLoopDefaultMode }, left.as_secs_f64(), false);
    }
}

struct Overlay {
    window: Retained<NSWindow>,
    image: Retained<CALayer>,
    /// Masks the old wallpaper for grow-circle and wipe.
    mask: Retained<CAShapeLayer>,
    bounds: CGRect,
    /// The screen's frame in global coordinates.
    frame: CGRect,
}

impl Overlay {
    fn new(
        screen: &NSScreen,
        workspace: &NSWorkspace,
        mtm: MainThreadMarker,
    ) -> anyhow::Result<Self> {
        let url = workspace
            .desktopImageURLForScreen(screen)
            .context("no wallpaper")?;
        let path = url.path().context("wallpaper is not a file")?.to_string();
        let image = imageio::cg_image(Path::new(&path)).map_err(anyhow::Error::msg)?;

        let frame = screen.frame();
        let window = desktop::overlay_window(frame, desktop::Layer::Wallpaper, mtm);
        let root = window
            .contentView()
            .and_then(|v| v.layer())
            .context("window has no layer")?;
        let bounds = CGRect::new(CGPoint::new(0.0, 0.0), frame.size);

        CATransaction::begin();
        CATransaction::setDisableActions(true);
        let (gravity, fill) = scaling(workspace, screen, &image);
        let layer = CALayer::new();
        layer.setFrame(bounds);
        // SAFETY: a CGImage is valid layer contents.
        unsafe { layer.setContents(Some(cf_object(&image))) };
        layer.setContentsGravity(gravity);
        layer.setContentsScale(window.backingScaleFactor());
        layer.setBackgroundColor(fill.as_deref());
        let mask = CAShapeLayer::new();
        mask.setFrame(bounds);
        // SAFETY: the rule is a valid constant.
        mask.setFillRule(unsafe { kCAFillRuleEvenOdd });
        root.addSublayer(&layer);
        CATransaction::commit();

        window.orderFrontRegardless();
        Ok(Self {
            window,
            image: layer,
            mask,
            bounds,
            frame,
        })
    }

    fn animate(&self, options: &Options, origin: CGPoint) {
        let (key, from, to): (&NSString, Retained<AnyObject>, Retained<AnyObject>) =
            match options.style {
                TransitionStyle::Fade | TransitionStyle::None => {
                    self.image.setOpacity(0.0);
                    (
                        ns_string!("opacity"),
                        NSNumber::new_f32(1.0).into(),
                        NSNumber::new_f32(0.0).into(),
                    )
                }
                TransitionStyle::GrowCircle => {
                    let center = CGPoint::new(
                        origin.x - self.frame.origin.x,
                        origin.y - self.frame.origin.y,
                    );
                    let radius = farthest_corner(self.bounds, center) + 2.0;
                    (
                        ns_string!("path"),
                        self.with_hole(center, 0.0),
                        self.with_hole(center, radius),
                    )
                }
                TransitionStyle::Wipe => {
                    let w = self.bounds.size.width;
                    let h = self.bounds.size.height;
                    (
                        ns_string!("path"),
                        rect_path(self.bounds),
                        rect_path(CGRect::new(CGPoint::new(w, 0.0), CGSize::new(0.0, h))),
                    )
                }
            };
        CATransaction::begin();
        CATransaction::setDisableActions(true);
        let animation = CABasicAnimation::animationWithKeyPath(Some(key));
        // SAFETY: the values match the animated property's type.
        unsafe {
            animation.setFromValue(Some(&from));
            animation.setToValue(Some(&to));
            animation.setFillMode(kCAFillModeForwards);
            animation.setTimingFunction(Some(&CAMediaTimingFunction::functionWithName(
                kCAMediaTimingFunctionEaseInEaseOut,
            )));
        }
        animation.setDuration(options.duration.as_secs_f64());
        animation.setRemovedOnCompletion(false);
        if key == ns_string!("opacity") {
            self.image.addAnimation_forKey(&animation, Some(key));
        } else {
            // SAFETY: the final path is a CGPath, the shape layer's type.
            unsafe {
                self.mask.setPath(Some(cf_cast(&to)));
                self.image.setMask(Some(&self.mask));
            }
            self.mask.addAnimation_forKey(&animation, Some(key));
        }
        CATransaction::commit();
    }

    /// The whole screen minus a circle: with even-odd filling, the old
    /// wallpaper stays visible outside the circle only.
    fn with_hole(&self, center: CGPoint, radius: CGFloat) -> Retained<AnyObject> {
        let path = CGMutablePath::new();
        let circle = CGRect::new(
            CGPoint::new(center.x - radius, center.y - radius),
            CGSize::new(radius * 2.0, radius * 2.0),
        );
        // SAFETY: null transforms are allowed.
        unsafe {
            CGMutablePath::add_rect(Some(&path), std::ptr::null(), self.bounds);
            CGMutablePath::add_ellipse_in_rect(Some(&path), std::ptr::null(), circle);
        }
        retain_cf(&path)
    }
}

fn rect_path(rect: CGRect) -> Retained<AnyObject> {
    let path = CGMutablePath::new();
    // SAFETY: a null transform is allowed.
    unsafe { CGMutablePath::add_rect(Some(&path), std::ptr::null(), rect) };
    retain_cf(&path)
}

fn farthest_corner(bounds: CGRect, p: CGPoint) -> CGFloat {
    let (w, h) = (bounds.size.width, bounds.size.height);
    [(0.0, 0.0), (w, 0.0), (0.0, h), (w, h)]
        .into_iter()
        .map(|(x, y)| (x - p.x).hypot(y - p.y))
        .fold(0.0, CGFloat::max)
}

/// How macOS draws the wallpaper on this screen, as a layer gravity, plus
/// the color around images that do not cover the screen.
pub(crate) fn scaling(
    workspace: &NSWorkspace,
    screen: &NSScreen,
    image: &CGImage,
) -> (
    &'static CALayerContentsGravity,
    Option<Retained<objc2_core_graphics::CGColor>>,
) {
    let options = workspace.desktopImageOptionsForScreen(screen);
    let get = |key: &NSString| options.as_ref().and_then(|o| o.objectForKey(key));
    // SAFETY: the keys are valid AppKit constants.
    let (scaling_key, clipping_key, fill_key) = unsafe {
        (
            NSWorkspaceDesktopImageScalingKey,
            NSWorkspaceDesktopImageAllowClippingKey,
            NSWorkspaceDesktopImageFillColorKey,
        )
    };
    let number = |key| get(key).and_then(|o| o.downcast::<NSNumber>().ok());
    let scaling = number(scaling_key).map_or(NSImageScaling::ScaleProportionallyUpOrDown, |n| {
        NSImageScaling(n.unsignedIntegerValue())
    });
    let clipping = number(clipping_key).is_none_or(|n| n.boolValue());
    let fill = get(fill_key)
        .and_then(|o| o.downcast::<NSColor>().ok())
        .map(|c| c.CGColor());

    let frame = screen.frame();
    let scale = screen.backingScaleFactor();
    #[allow(clippy::cast_precision_loss)] // Image sides are far below 2^52.
    let larger_than_screen = CGImage::width(Some(image)) as CGFloat > frame.size.width * scale
        || CGImage::height(Some(image)) as CGFloat > frame.size.height * scale;
    // SAFETY: the gravities are valid constants.
    let gravity = unsafe {
        match scaling {
            NSImageScaling::ScaleProportionallyUpOrDown if clipping => kCAGravityResizeAspectFill,
            NSImageScaling::ScaleProportionallyUpOrDown => kCAGravityResizeAspect,
            NSImageScaling::ScaleAxesIndependently => kCAGravityResize,
            NSImageScaling::ScaleProportionallyDown if larger_than_screen => kCAGravityResizeAspect,
            _ => kCAGravityCenter,
        }
    };
    (gravity, fill)
}

/// A Core Foundation object seen as an Objective-C object (they are
/// toll-free bridged).
pub(crate) fn cf_object<T: AsRef<AnyObject>>(value: &T) -> &AnyObject {
    value.as_ref()
}

pub(crate) fn retain_cf<T: AsRef<AnyObject>>(value: &T) -> Retained<AnyObject> {
    let object: &AnyObject = value.as_ref();
    // SAFETY: retaining a valid object.
    unsafe { Retained::retain(std::ptr::from_ref(object).cast_mut()) }.expect("non-null")
}

/// The reverse of [`retain_cf`] for paths created there.
///
/// # Safety
///
/// `object` must be a `CGPath`.
unsafe fn cf_cast(object: &AnyObject) -> &objc2_core_graphics::CGPath {
    // SAFETY: guaranteed by the caller; CF and Objective-C objects share
    // their representation.
    unsafe { &*std::ptr::from_ref(object).cast() }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn farthest_corner_covers_the_screen() {
        let bounds = CGRect::new(CGPoint::new(0.0, 0.0), CGSize::new(300.0, 400.0));
        assert!((farthest_corner(bounds, CGPoint::new(0.0, 0.0)) - 500.0).abs() < 1e-9);
        assert!((farthest_corner(bounds, CGPoint::new(150.0, 200.0)) - 250.0).abs() < 1e-9);
        // Origins on another screen still reach every corner.
        assert!(
            (farthest_corner(bounds, CGPoint::new(-300.0, 0.0)) - 600.0f64.hypot(400.0)).abs()
                < 1e-9
        );
    }
}
