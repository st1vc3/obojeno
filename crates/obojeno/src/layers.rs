//! Windows and Core Animation pieces shared by everything drawn over the
//! wallpaper: transitions, the finish and the lid animation.

use objc2::rc::Retained;
use objc2::runtime::AnyObject;
use objc2::{MainThreadMarker, MainThreadOnly as _};
use objc2_app_kit::{
    NSBackingStoreType, NSColor, NSScreen, NSWindow, NSWindowCollectionBehavior, NSWindowStyleMask,
};
use objc2_core_foundation::{CGFloat, CGRect};
use objc2_core_graphics::{
    CGColor, CGDisplayIsBuiltin, CGPath, CGWindowLevelForKey, CGWindowLevelKey,
};
use objc2_foundation::{NSArray, NSNumber, ns_string};
use objc2_quartz_core::{CAGradientLayer, CALayer, CATransform3D, CATransform3DIdentity};

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

/// The layer of a window's content view, which overlay windows draw into.
pub fn root_layer(window: &NSWindow) -> Option<Retained<CALayer>> {
    window.contentView().and_then(|view| view.layer())
}

/// The screen's display ID, which stays the same while it is attached.
pub fn display_id(screen: &NSScreen) -> Option<u32> {
    screen
        .deviceDescription()
        .objectForKey(ns_string!("NSScreenNumber"))
        .and_then(|n| n.downcast::<NSNumber>().ok())
        .map(|n| n.unsignedIntValue())
}

/// Whether a screen is a MacBook's built-in display.
pub fn is_builtin(screen: &NSScreen) -> bool {
    display_id(screen).is_some_and(|id| CGDisplayIsBuiltin(id))
}

/// The MacBook's built-in display, when attached.
pub fn builtin_screen(mtm: MainThreadMarker) -> Option<Retained<NSScreen>> {
    NSScreen::screens(mtm)
        .iter()
        .find(|screen| is_builtin(screen))
}

/// A Core Foundation object seen as an Objective-C object (they are
/// toll-free bridged), for layer contents and animation values.
pub fn cf_object<T: AsRef<AnyObject>>(value: &T) -> &AnyObject {
    value.as_ref()
}

/// Like [`cf_object`], but owned, for arrays and animation values.
pub fn retain_cf<T: AsRef<AnyObject>>(value: &T) -> Retained<AnyObject> {
    let object: &AnyObject = value.as_ref();
    // SAFETY: retaining a valid object.
    unsafe { Retained::retain(std::ptr::from_ref(object).cast_mut()) }.expect("non-null")
}

/// The reverse of [`retain_cf`] for paths.
///
/// # Safety
///
/// `object` must be a `CGPath`.
pub unsafe fn as_path(object: &AnyObject) -> &CGPath {
    // SAFETY: guaranteed by the caller; CF and Objective-C objects share
    // their representation.
    unsafe { &*std::ptr::from_ref(object).cast() }
}

/// Sets a gradient's colors, first to last.
pub fn set_colors(layer: &CAGradientLayer, colors: &[&CGColor]) {
    let objects: Vec<Retained<AnyObject>> = colors.iter().map(retain_cf).collect();
    // SAFETY: an array of CGColors, as `colors` expects.
    unsafe { layer.setColors(Some(&NSArray::from_retained_slice(&objects))) };
}

/// Gradient stop positions as the `NSNumber`s Core Animation wants.
pub fn locations(stops: &[CGFloat]) -> Retained<NSArray<NSNumber>> {
    let numbers: Vec<Retained<NSNumber>> = stops.iter().map(|&s| NSNumber::new_f64(s)).collect();
    NSArray::from_retained_slice(&numbers)
}

/// A perspective for transforms that tip a layer in 3D: the smaller
/// `distance`, the stronger.
pub fn perspective(distance: CGFloat) -> CATransform3D {
    CATransform3D {
        m34: -1.0 / distance,
        // SAFETY: a constant Core Animation provides.
        ..unsafe { CATransform3DIdentity }
    }
}
