//! Drawing the lid animation: as the MacBook closes, the wallpaper's edges
//! soften, darkness comes down from the top over everything and the
//! wallpaper blurs and sinks toward the bottom. `lid` feeds it the progress.

use std::cell::RefCell;
use std::path::{Path, PathBuf};

use objc2::MainThreadMarker;
use objc2::rc::Retained;
use objc2_app_kit::{NSColor, NSScreen, NSWindow, NSWorkspace};
use objc2_core_foundation::{CFRetained, CGFloat, CGPoint, CGRect, CGSize};
use objc2_core_graphics::{CGImage, CGImageAlphaInfo};
use objc2_quartz_core::{
    CAGradientLayer, CALayer, CATransaction, CATransform3D, kCAGradientLayerRadial,
};

use crate::imageio::{self, Bitmap, LayerPixels};
use crate::layers::{self, Layer};
use crate::screen_images;
use crate::transition::Placement;
use image::RgbaImage;

/// How much of the top and sides the soft edges fade out.
const FEATHER: CGFloat = 0.2;
/// How much of the bottom fades: little, since the wallpaper sinks toward
/// it and glows there last.
const FEATHER_BOTTOM: CGFloat = 0.05;
/// The feather mask's size; small, since it is smooth when stretched.
const FEATHER_W: usize = 320;
const FEATHER_H: usize = 200;

/// Blur strengths of the two blurred copies, in pixels of the small copy
/// `screen_images` keeps (`SMALL_SIDE` long).
const BLURS: [f32; 2] = [3.0, 9.0];

struct LidWindow {
    window: Retained<NSWindow>,
    /// Holds the wallpaper; sinks toward its bottom edge.
    sinking: Retained<CALayer>,
    /// Softens the edges of `sinking` more the further the lid closes.
    feather: Retained<CALayer>,
    /// The wallpaper, then two blurred copies over it that fade in as the
    /// lid closes: crossfading is smooth where animating a blur is not.
    wallpaper: Retained<CALayer>,
    soft: Retained<CALayer>,
    softer: Retained<CALayer>,
    /// Comes down from the top as the lid closes, over the menu bar too.
    curtain_window: Retained<NSWindow>,
    curtain: Retained<CAGradientLayer>,
    /// Darkens the edges of the screen.
    vignette: Retained<CAGradientLayer>,
    shade: Retained<CALayer>,
    /// The wallpaper the layers show.
    showing: Option<PathBuf>,
    visible: bool,
}

#[derive(Default)]
struct Lid {
    /// On the built-in screen, while the lid animation is on.
    window: Option<LidWindow>,
    /// A previewed progress in percent; the sensor is ignored while set.
    preview: Option<u16>,
}

thread_local! {
    static LID: RefCell<Lid> = RefCell::default();
}

/// Builds the lid windows when the animation is on, or removes them.
pub fn rebuild(enabled: bool, mtm: MainThreadMarker) {
    LID.with_borrow_mut(|lid| {
        if let Some(old) = lid.window.take() {
            old.window.close();
            old.curtain_window.close();
        }
        if enabled {
            lid.window = layers::builtin_screen(mtm).and_then(|screen| lid_window(&screen, mtm));
        }
    });
    wallpaper_ready(mtm);
}

/// Shows the effect at `percent` regardless of the sensor, or follows the
/// sensor again with `None`.
pub fn preview(percent: Option<u16>, mtm: MainThreadMarker) {
    LID.with_borrow_mut(|lid| lid.preview = percent);
    show(percent.map_or(0.0, |p| f64::from(p.min(100)) / 100.0), mtm);
}

/// The sensor's progress, 0 open to 1 shut; ignored while previewing.
pub fn progress(t: f64, mtm: MainThreadMarker) {
    if LID.with_borrow(|lid| lid.preview.is_none()) {
        show(t, mtm);
    }
}

fn lid_window(screen: &NSScreen, mtm: MainThreadMarker) -> Option<LidWindow> {
    let frame = screen.frame();
    let bounds = CGRect::new(CGPoint::new(0.0, 0.0), frame.size);
    let black = NSColor::blackColor().CGColor();
    let clear = NSColor::clearColor().CGColor();

    let window = layers::overlay_window(frame, Layer::Wallpaper, mtm);
    let root = layers::root_layer(&window)?;
    let feather_mask = feather_image()?;
    CATransaction::begin();
    CATransaction::setDisableActions(true);
    root.setBackgroundColor(Some(&black));
    let sinking = CALayer::new();
    sinking.setAnchorPoint(CGPoint::new(0.5, 0.0));
    sinking.setBounds(bounds);
    sinking.setPosition(CGPoint::new(bounds.size.width / 2.0, 0.0));
    let feather = CALayer::new();
    feather.setFrame(feather_frame(bounds, 0.0));
    // SAFETY: a CGImage is valid layer contents, and any layer is a valid
    // mask.
    unsafe {
        feather.setContents(Some(layers::cf_object(&feather_mask)));
        sinking.setMask(Some(&feather));
    }
    let image_layer = |opacity: f32| {
        let layer = CALayer::new();
        layer.setFrame(bounds);
        layer.setContentsScale(screen.backingScaleFactor());
        layer.setOpacity(opacity);
        sinking.addSublayer(&layer);
        layer
    };
    let wallpaper = image_layer(1.0);
    let soft = image_layer(0.0);
    let softer = image_layer(0.0);
    let vignette = CAGradientLayer::new();
    vignette.setFrame(bounds);
    // SAFETY: the type is a valid constant.
    vignette.setType(unsafe { kCAGradientLayerRadial });
    vignette.setStartPoint(CGPoint::new(0.5, 0.35));
    vignette.setEndPoint(CGPoint::new(1.15, 1.15));
    layers::set_colors(&vignette, &[&clear, &black]);
    vignette.setOpacity(0.0);
    let shade = CALayer::new();
    shade.setFrame(bounds);
    shade.setBackgroundColor(Some(&black));
    shade.setOpacity(0.0);
    root.addSublayer(&sinking);
    root.addSublayer(&vignette);
    root.addSublayer(&shade);

    let curtain_window = layers::overlay_window(frame, Layer::Curtain, mtm);
    let curtain = CAGradientLayer::new();
    curtain.setFrame(bounds);
    // Clear below the locations, black above: in layer coordinates the
    // gradient runs bottom (0) to top (1).
    curtain.setStartPoint(CGPoint::new(0.5, 0.0));
    curtain.setEndPoint(CGPoint::new(0.5, 1.0));
    layers::set_colors(&curtain, &[&clear, &black]);
    curtain.setLocations(Some(&layers::locations(&[1.0, 1.0])));
    layers::root_layer(&curtain_window)?.addSublayer(&curtain);
    CATransaction::commit();

    Some(LidWindow {
        window,
        sinking,
        feather,
        wallpaper,
        soft,
        softer,
        curtain_window,
        curtain,
        vignette,
        shade,
        showing: None,
        visible: false,
    })
}

/// Shows the effect at progress `t`, 0 open to 1 shut.
fn show(t: f64, mtm: MainThreadMarker) {
    if t > 0.0 && LID.with_borrow(|lid| lid.window.as_ref().is_some_and(|w| w.showing.is_none())) {
        // Usually decoded when the wallpaper changed already.
        screen_images::refresh(mtm);
    }
    LID.with_borrow_mut(|lid| {
        let Some(lid) = lid.window.as_mut() else {
            return;
        };
        if t <= 0.0 {
            if lid.visible {
                lid.window.orderOut(None);
                lid.curtain_window.orderOut(None);
                lid.visible = false;
            }
            return;
        }
        if !lid.visible {
            lid.curtain_window.orderFrontRegardless();
            lid.visible = true;
        }
        // The wallpaper part only with the current wallpaper's picture:
        // until it is ready, the desktop shows through under the curtain,
        // never the previous wallpaper.
        let ready = lid.showing.is_some();
        if ready != lid.window.isVisible() {
            if ready {
                lid.window.orderFrontRegardless();
            } else {
                lid.window.orderOut(None);
            }
        }
        CATransaction::begin();
        // Every frame comes smoothed from the sensor; implicit animations
        // would only lag behind it.
        CATransaction::setDisableActions(true);
        // The edges soften first; only then does the wallpaper move, so its
        // border is already faded out when it leaves the screen's. It mostly
        // sinks toward its bottom edge, with a hint of tipping away
        // (negative rotation; positive brings the top toward you).
        let (soften, sink) = stages(t);
        let tilt = CATransform3D::new_rotation(-sink * 10.0_f64.to_radians(), 1.0, 0.0, 0.0);
        let shrink = 1.0 - 0.14 * sink;
        // Perspective last, about the layer's own anchor (its bottom
        // center), so it stays symmetric: the root layer of a view is
        // anchored at its corner, which skews a sublayer transform.
        lid.sinking.setTransform(
            tilt.scale(shrink, shrink, 1.0)
                .concat(layers::perspective(900.0)),
        );
        lid.feather
            .setFrame(feather_frame(lid.sinking.bounds(), soften));
        // A wide, soft curtain: fully open at 0, only a glow at the bottom
        // at 1.
        let edge = 1.5 - 1.25 * t;
        lid.curtain
            .setLocations(Some(&layers::locations(&[edge - 0.5, edge])));
        #[expect(clippy::cast_possible_truncation)] // Opacities.
        {
            lid.vignette.setOpacity((t * 0.7) as f32);
            lid.shade.setOpacity((t * 0.25) as f32);
            lid.soft.setOpacity(ease(t / 0.35) as f32);
            lid.softer.setOpacity(ease((t - 0.3) / 0.5) as f32);
        }
        CATransaction::commit();
    });
}

/// How far the edges have softened and the wallpaper has sunk at progress
/// `t`: softening takes the first fifth, sinking the rest.
fn stages(t: f64) -> (f64, f64) {
    const SOFTEN: f64 = 0.2;
    (ease(t / SOFTEN), ease((t - SOFTEN) / (1.0 - SOFTEN)))
}

/// Smoothstep on `x` clamped to 0..=1.
fn ease(x: f64) -> f64 {
    let x = x.clamp(0.0, 1.0);
    x * x * (3.0 - 2.0 * x)
}

/// A mask that is opaque in the middle and fades to clear over the outer
/// `FEATHER` of the top and sides, `FEATHER_BOTTOM` at the bottom.
fn feather_image() -> Option<CFRetained<CGImage>> {
    imageio::image_from_pixels(
        &mut feather_pixels(),
        Bitmap {
            width: FEATHER_W,
            height: FEATHER_H,
            bytes_per_row: FEATHER_W,
            color_space: None,
            info: CGImageAlphaInfo::Only.0,
        },
    )
}

/// The mask's alpha, rows top to bottom.
fn feather_pixels() -> Vec<u8> {
    const W: usize = FEATHER_W;
    const H: usize = FEATHER_H;
    // 0 at an edge, 1 from `width` inward, eased.
    #[expect(clippy::cast_precision_loss)] // Small sizes.
    let ramp =
        |from_edge: usize, n: usize, width: f64| ease((from_edge as f64 + 0.5) / n as f64 / width);
    let mut pixels = vec![0u8; W * H];
    for (y, row) in pixels.as_chunks_mut::<W>().0.iter_mut().enumerate() {
        let vertical = ramp(y, H, FEATHER).min(ramp(H - 1 - y, H, FEATHER_BOTTOM));
        for (x, px) in row.iter_mut().enumerate() {
            #[expect(clippy::cast_possible_truncation, clippy::cast_sign_loss)] // 0..=255.
            {
                *px = (ramp(x.min(W - 1 - x), W, FEATHER) * vertical * 255.0).round() as u8;
            }
        }
    }
    pixels
}

/// Where the feather mask goes as the edges soften (`t` from 0 to 1): at 0
/// its fading edges lie outside `bounds`, at 1 they end on them.
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

/// Shows the built-in screen's wallpaper in the lid layers once
/// `screen_images` has it decoded, with its blurred copies.
pub fn wallpaper_ready(mtm: MainThreadMarker) {
    let Some(screen) = layers::builtin_screen(mtm) else {
        return;
    };
    let needed = LID.with_borrow(|lid| lid.window.as_ref().map(|w| w.showing.clone()));
    let Some(showing) = needed else {
        return; // The lid animation is off.
    };
    let ready = screen_images::with(&screen, |shown| {
        (showing.as_ref() != Some(&shown.path)).then(|| {
            (
                shown.path.clone(),
                shown.image.clone(),
                blurred(&shown.small),
            )
        })
    })
    .flatten();
    let Some((path, sharp, [soft, softer])) = ready else {
        return;
    };
    let placement = match Placement::of(&NSWorkspace::sharedWorkspace(), &screen, &path) {
        Ok(placement) => placement,
        Err(e) => {
            tracing::warn!("lid animation: {e:#}");
            return;
        }
    };
    let (Some(soft), Some(softer)) = (soft.into_surface(), softer.into_surface()) else {
        return;
    };
    LID.with_borrow_mut(|lid| {
        let Some(lid) = lid.window.as_mut() else {
            return;
        };
        CATransaction::begin();
        CATransaction::setDisableActions(true);
        for (layer, image) in [
            (&lid.wallpaper, &sharp),
            (&lid.soft, &soft),
            (&lid.softer, &softer),
        ] {
            // SAFETY: an IOSurface is valid layer contents.
            unsafe { layer.setContents(Some(layers::cf_object(image))) };
            // All three are drawn like macOS draws the wallpaper.
            placement.apply(layer);
        }
        CATransaction::commit();
        lid.showing = Some(path);
        if lid.visible {
            // Joins the curtain mid-movement.
            lid.window.orderFrontRegardless();
        }
    });
}

/// The built-in screen's wallpaper is now `path`: forgets the picture of
/// another one, so the lid never shows a previous wallpaper while the new
/// one is decoded.
pub fn wallpaper_changed(path: &Path) {
    LID.with_borrow_mut(|lid| {
        if let Some(lid) = lid.window.as_mut()
            && lid.showing.as_deref() != Some(path)
        {
            lid.showing = None;
        }
    });
}

/// The two blurred copies of the wallpaper, from its small copy.
fn blurred(small: &RgbaImage) -> [LayerPixels; 2] {
    BLURS.map(|sigma| LayerPixels::from_rgba(&image::imageops::blur(small, sigma)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn edges_soften_before_the_wallpaper_moves() {
        assert_eq!(stages(0.0), (0.0, 0.0));
        let (soften, sink) = stages(0.2);
        assert!((soften - 1.0).abs() < f64::EPSILON && sink.abs() < f64::EPSILON);
        let (soften, sink) = stages(0.6);
        assert!((soften - 1.0).abs() < f64::EPSILON && sink > 0.0 && sink < 1.0);
        assert_eq!(stages(1.0), (1.0, 1.0));
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
    fn feather_fades_the_bottom_least() {
        let pixels = feather_pixels();
        let at = |x: usize, y: usize| pixels[y * FEATHER_W + x];
        let middle = FEATHER_W / 2;
        assert_eq!(at(middle, FEATHER_H / 2), 255, "opaque in the middle");
        assert_eq!(at(middle, 0), 0, "clear at the top edge");
        assert_eq!(at(0, FEATHER_H / 2), 0, "clear at the side edges");
        assert_eq!(at(FEATHER_W - 1, FEATHER_H / 2), 0);
        // Twelve rows in: past the bottom fade, well inside the top one.
        assert_eq!(at(middle, FEATHER_H - 1 - 12), 255);
        assert!(at(middle, 12) < 128);
    }

    #[test]
    fn blurs_keep_the_small_copy_size() {
        let small = RgbaImage::from_pixel(48, 30, image::Rgba([200, 30, 60, 255]));
        let [soft, softer] = blurred(&small);
        for blur in [soft, softer] {
            let surface = blur.into_surface().expect("a surface");
            assert_eq!((surface.width(), surface.height()), (48, 30));
        }
    }
}
