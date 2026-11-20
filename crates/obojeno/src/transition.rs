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
use objc2_core_graphics::{CGColor, CGMutablePath};
use objc2_foundation::{NSNumber, NSString, ns_string};
use objc2_quartz_core::{
    CABasicAnimation, CALayer, CAMediaTiming as _, CAMediaTimingFunction, CAShapeLayer,
    CATransaction, kCAFillModeForwards, kCAFillRuleEvenOdd, kCAGravityCenter, kCAGravityResize,
    kCAGravityResizeAspect, kCAGravityResizeAspectFill, kCAMediaTimingFunctionEaseInEaseOut,
};
use obojeno_core::config::TransitionStyle;

use crate::imageio::{self, LayerPixels};
use crate::layers::{self, Layer};

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
                    tracing::warn!("no transition on {}: {e:#}", screen.localizedName());
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
            overlay.dispose();
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
        let path = crate::macos::wallpaper_of(screen)?;
        let placement = Placement::of(workspace, screen, &path)?;
        // Decoded ahead of time, unless the last change was just now.
        let image = match crate::screen_images::with(screen, |shown| shown.image.clone()) {
            Some(image) => image,
            None => LayerPixels::decode(&path, placement.needed_side())
                .map_err(anyhow::Error::msg)?
                .into_surface()
                .context("cannot show the wallpaper")?,
        };

        let frame = screen.frame();
        let window = layers::overlay_window(frame, Layer::Wallpaper, mtm);
        let root = layers::root_layer(&window).context("window has no layer")?;
        let bounds = CGRect::new(CGPoint::new(0.0, 0.0), frame.size);

        CATransaction::begin();
        CATransaction::setDisableActions(true);
        let layer = CALayer::new();
        layer.setFrame(bounds);
        // SAFETY: an IOSurface is valid layer contents.
        unsafe { layer.setContents(Some(layers::cf_object(&image))) };
        placement.apply(&layer);
        layer.setContentsScale(window.backingScaleFactor());
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
                self.mask.setPath(Some(layers::as_path(&to)));
                self.image.setMask(Some(&self.mask));
            }
            self.mask.addAnimation_forKey(&animation, Some(key));
        }
        CATransaction::commit();
    }

    /// Closes the window and lets go of the old wallpaper. Clearing the
    /// layers first matters: Core Animation keeps its own copy of their
    /// images for as long as the layer tree lives, which can outlast the
    /// window, and that copy is the size of the screen.
    fn dispose(&self) {
        CATransaction::begin();
        CATransaction::setDisableActions(true);
        self.mask.removeAllAnimations();
        self.image.removeAllAnimations();
        // SAFETY: no mask and no contents are valid values.
        unsafe {
            self.image.setMask(None);
            self.image.setContents(None);
        }
        self.image.removeFromSuperlayer();
        CATransaction::commit();
        self.window.orderOut(None);
        self.window.setContentView(None);
        self.window.close();
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
        layers::retain_cf(&path)
    }
}

fn rect_path(rect: CGRect) -> Retained<AnyObject> {
    let path = CGMutablePath::new();
    // SAFETY: a null transform is allowed.
    unsafe { CGMutablePath::add_rect(Some(&path), std::ptr::null(), rect) };
    layers::retain_cf(&path)
}

fn farthest_corner(bounds: CGRect, p: CGPoint) -> CGFloat {
    let (w, h) = (bounds.size.width, bounds.size.height);
    [(0.0, 0.0), (w, 0.0), (0.0, h), (w, h)]
        .into_iter()
        .map(|(x, y)| (x - p.x).hypot(y - p.y))
        .fold(0.0, CGFloat::max)
}

/// How macOS draws a wallpaper on a screen: its scaling options, the color
/// around images that do not cover the screen, and the sizes involved.
pub struct Placement {
    scaling: Scaling,
    fill: Option<Retained<CGColor>>,
    /// The image's size and the screen's, in pixels.
    image: CGSize,
    screen: CGSize,
}

/// The ways macOS fits a wallpaper to a screen.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Scaling {
    /// Covers the screen, cropping the overflow.
    Fill,
    /// Fits in the screen, with the fill color around it.
    Fit,
    Stretch,
    /// At its own size, in the middle.
    Center,
}

impl Placement {
    /// For the image at `path` on `screen`. Reads the image's size without
    /// decoding it.
    pub fn of(workspace: &NSWorkspace, screen: &NSScreen, path: &Path) -> anyhow::Result<Self> {
        let (width, height) = imageio::size(path).map_err(anyhow::Error::msg)?;
        let image = CGSize::new(CGFloat::from(width), CGFloat::from(height));
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
        let ns_scaling = number(scaling_key)
            .map_or(NSImageScaling::ScaleProportionallyUpOrDown, |n| {
                NSImageScaling(n.unsignedIntegerValue())
            });
        let clipping = number(clipping_key).is_none_or(|n| n.boolValue());
        let frame = screen.frame();
        let scale = screen.backingScaleFactor();
        let screen_px = CGSize::new(frame.size.width * scale, frame.size.height * scale);
        let larger_than_screen = image.width > screen_px.width || image.height > screen_px.height;
        let scaling = match ns_scaling {
            NSImageScaling::ScaleProportionallyUpOrDown if clipping => Scaling::Fill,
            NSImageScaling::ScaleProportionallyUpOrDown => Scaling::Fit,
            NSImageScaling::ScaleAxesIndependently => Scaling::Stretch,
            NSImageScaling::ScaleProportionallyDown if larger_than_screen => Scaling::Fit,
            _ => Scaling::Center,
        };
        let fill = get(fill_key)
            .and_then(|o| o.downcast::<NSColor>().ok())
            .map(|c| c.CGColor());
        Ok(Self {
            scaling,
            fill,
            image,
            screen: screen_px,
        })
    }

    /// Draws a layer's contents like macOS draws the wallpaper.
    pub fn apply(&self, layer: &CALayer) {
        // SAFETY: the gravities are valid constants.
        let gravity = unsafe {
            match self.scaling {
                Scaling::Fill => kCAGravityResizeAspectFill,
                Scaling::Fit => kCAGravityResizeAspect,
                Scaling::Stretch => kCAGravityResize,
                Scaling::Center => kCAGravityCenter,
            }
        };
        layer.setContentsGravity(gravity);
        layer.setBackgroundColor(self.fill.as_deref());
    }

    /// The longest side the image is shown at: decoding it any larger only
    /// costs memory.
    pub fn needed_side(&self) -> u32 {
        needed_side(self.scaling, self.image, self.screen)
    }
}

fn needed_side(scaling: Scaling, image: CGSize, screen: CGSize) -> u32 {
    let (x, y) = (screen.width / image.width, screen.height / image.height);
    let scale = match scaling {
        // Both sides must reach the screen's.
        Scaling::Fill | Scaling::Stretch => x.max(y),
        Scaling::Fit => x.min(y),
        Scaling::Center => 1.0,
    };
    // Never above the image's own size: scaling up adds no detail.
    #[expect(clippy::cast_possible_truncation, clippy::cast_sign_loss)] // Positive pixel sizes.
    let side = (image.width.max(image.height) * scale.min(1.0)).round() as u32;
    side.max(1)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decodes_only_as_large_as_shown() {
        let screen = CGSize::new(3456.0, 2234.0);
        let six_k = CGSize::new(6016.0, 6016.0);
        // A square filling a wide screen is shown as wide as the screen.
        assert_eq!(needed_side(Scaling::Fill, six_k, screen), 3456);
        assert_eq!(needed_side(Scaling::Fit, six_k, screen), 2234);
        assert_eq!(needed_side(Scaling::Center, six_k, screen), 6016);
        // A panorama filling the screen needs its full height on screen.
        let panorama = CGSize::new(12000.0, 2000.0);
        assert_eq!(needed_side(Scaling::Fill, panorama, screen), 12000);
        // Small images are never asked for larger than they are.
        let small = CGSize::new(800.0, 600.0);
        assert_eq!(needed_side(Scaling::Fill, small, screen), 800);
    }

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
