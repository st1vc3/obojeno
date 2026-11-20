//! The windows drawn over the wallpaper for as long as the settings ask for
//! them: the finish (grain, rounded corners) and the lid animation
//! (`lid_view`). Stacking, bottom to top: wallpaper, transition and lid
//! windows, finish windows, desktop icons, everything else.

use std::cell::RefCell;

use objc2::MainThreadMarker;
use objc2::rc::Retained;
use objc2_app_kit::{NSColor, NSScreen, NSWindow};
use objc2_core_foundation::{CFRetained, CGFloat, CGPoint, CGRect, CGSize};
use objc2_core_graphics::CGMutablePath;
use objc2_quartz_core::{
    CALayer, CAReplicatorLayer, CAShapeLayer, CATransaction, CATransform3D, kCAFilterNearest,
};
use obojeno_core::config::Finish;

use crate::imageio::LayerPixels;
use crate::layers::{self, Layer};
use crate::lid_view;

#[derive(Default)]
struct Overlays {
    finish: Vec<Retained<NSWindow>>,
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
        if finish.grain > 0.0 || finish.corner_radius > 0.0 {
            overlays.finish = NSScreen::screens(mtm)
                .iter()
                .map(|screen| finish_window(&screen, finish, mtm))
                .collect();
        }
        overlays.settings = Some((finish, lid_animation));
    });
    lid_view::rebuild(lid_animation, mtm);
}

fn finish_window(screen: &NSScreen, finish: Finish, mtm: MainThreadMarker) -> Retained<NSWindow> {
    let frame = screen.frame();
    let window = layers::overlay_window(frame, Layer::Finish, mtm);
    let bounds = CGRect::new(CGPoint::new(0.0, 0.0), frame.size);
    if let Some(root) = layers::root_layer(&window) {
        CATransaction::begin();
        CATransaction::setDisableActions(true);
        if finish.grain > 0.0
            && let Some(grain) = grain_layer(bounds, screen.backingScaleFactor(), finish.grain)
        {
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
                !layers::is_builtin(screen),
            )));
            root.addSublayer(&corners);
        }
        CATransaction::commit();
    }
    window.orderFrontRegardless();
    window
}

/// The black areas outside rounded corners.
fn corner_path(size: CGSize, radius: CGFloat, top_too: bool) -> CFRetained<CGMutablePath> {
    // Each corner is a mask from just outside the screen to the rounded
    // curve. Reaching past the edges keeps them from being anti-aliased,
    // which left a hairline of wallpaper along the edge.
    const BLEED: CGFloat = 2.0;
    let (w, h, r) = (size.width, size.height, radius);
    let path = CGMutablePath::new();
    // `dx`/`dy` point from the corner into the screen; (0, 0) is the bottom
    // left.
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

/// The side of the grain's noise tile in device pixels: large enough that
/// its repeats go unnoticed, and a fraction of a screen-sized image.
const GRAIN_TILE: usize = 1024;

/// Noise over `bounds`, one noise pixel per device pixel: a tile of it,
/// repeated across and down by replicator layers, which all show the same
/// surface. `strength` is 0 to 1.
fn grain_layer(bounds: CGRect, scale: CGFloat, strength: f64) -> Option<Retained<CALayer>> {
    let surface = LayerPixels::from_gray(GRAIN_TILE, GRAIN_TILE, &noise(GRAIN_TILE * GRAIN_TILE))
        .into_surface()?;
    #[expect(clippy::cast_precision_loss)] // A small size.
    let side = GRAIN_TILE as CGFloat / scale;
    let tile = CALayer::new();
    tile.setFrame(CGRect::new(CGPoint::new(0.0, 0.0), CGSize::new(side, side)));
    // SAFETY: an IOSurface is valid layer contents.
    unsafe { tile.setContents(Some(layers::cf_object(&surface))) };
    tile.setContentsScale(scale);
    // SAFETY: the filter name is a valid constant.
    tile.setMagnificationFilter(unsafe { kCAFilterNearest });
    // Full strength is quarter-opaque noise; more turns the wallpaper to
    // static. On the tile, as the tiles never overlap: on a container it
    // would make Core Animation draw them offscreen first.
    #[expect(clippy::cast_possible_truncation)] // An opacity.
    tile.setOpacity((strength.clamp(0.0, 1.0) * 0.25) as f32);

    let row = CAReplicatorLayer::new();
    row.setFrame(bounds);
    row.setInstanceCount(tiles(bounds.size.width, side));
    row.setInstanceTransform(CATransform3D::new_translation(side, 0.0, 0.0));
    row.addSublayer(&tile);
    let grid = CAReplicatorLayer::new();
    grid.setFrame(bounds);
    grid.setInstanceCount(tiles(bounds.size.height, side));
    grid.setInstanceTransform(CATransform3D::new_translation(0.0, side, 0.0));
    grid.addSublayer(&row);
    Some(Retained::into_super(grid))
}

/// How many tiles of `side` it takes to cover `length`.
fn tiles(length: CGFloat, side: CGFloat) -> isize {
    #[expect(clippy::cast_possible_truncation)] // Screen sizes.
    let count = (length / side).ceil() as isize;
    count.max(1)
}

/// `len` grayscale noise values. Each is the mean of two random bytes,
/// which clusters them around mid-gray for a softer, film-like grain than
/// uniform noise.
fn noise(len: usize) -> Vec<u8> {
    // xorshift: any decent noise will do, and it needs no dependency.
    let mut x: u32 = 0x9E37_79B9;
    (0..len)
        .map(|_| {
            x ^= x << 13;
            x ^= x >> 17;
            x ^= x << 5;
            let [a, b, ..] = x.to_le_bytes();
            a.midpoint(b)
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use objc2_core_graphics::{CGBitmapContextCreate, CGColorSpace, CGContext, CGImageAlphaInfo};

    /// Renders the corner masks into a `w`×`h` alpha bitmap (row 0 at the
    /// bottom, like the layer).
    fn render(w: usize, h: usize, radius: CGFloat, top_too: bool) -> Vec<u8> {
        let mut pixels = vec![0u8; w * h * 4];
        let space = CGColorSpace::new_device_rgb().unwrap();
        #[expect(clippy::cast_precision_loss)]
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
        pixels
            .chunks(w * 4)
            .rev()
            .flat_map(|row| row.iter().skip(3).step_by(4).copied())
            .collect()
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

    #[test]
    fn grain_tiles_cover_the_screen() {
        // A 3456x2234 pixel screen at 2x, in 512 point tiles.
        assert_eq!(tiles(1728.0, 512.0), 4);
        assert_eq!(tiles(1117.0, 512.0), 3);
        assert_eq!(tiles(1024.0, 512.0), 2, "exactly");
        assert_eq!(tiles(0.0, 512.0), 1, "at least one");
    }

    #[test]
    fn noise_clusters_around_mid_gray() {
        let values = noise(64 * 64);
        let mean = values.iter().map(|&v| f64::from(v)).sum::<f64>() / 4096.0;
        assert!((mean - 127.5).abs() < 4.0, "{mean}");
        assert!(values.iter().any(|&v| v < 64) && values.iter().any(|&v| v > 192));
    }
}
