//! Decoding images with macOS ImageIO, so every format the desktop can show
//! works, HEIC included (all built-in wallpapers are HEIC).

use std::path::Path;

use image::{DynamicImage, RgbaImage};
use objc2_core_foundation::{
    CFBoolean, CFDictionary, CFNumber, CFRetained, CFString, CFType, CFURL, CFURLPathStyle,
    CGPoint, CGRect, CGSize,
};
use objc2_core_graphics::{
    CGBitmapContextCreate, CGColorSpace, CGContext, CGImage, CGImageAlphaInfo,
    CGImageByteOrderInfo, kCGColorSpaceSRGB,
};
use objc2_image_io::{
    CGImageSource, kCGImageSourceCreateThumbnailFromImageAlways,
    kCGImageSourceCreateThumbnailWithTransform, kCGImageSourceThumbnailMaxPixelSize,
};

/// Decodes `path` to sRGB pixels, converting from embedded color profiles
/// (Display P3 wallpapers, for example). Falls back to the pure-Rust decoders
/// for anything ImageIO rejects.
pub fn decode(path: &Path) -> Result<DynamicImage, String> {
    decode_with_imageio(path).or_else(|imageio_error| {
        obojeno_core::palette::open_image(path).map_err(|e| format!("{imageio_error}; {e}"))
    })
}

fn not_readable() -> String {
    "macOS cannot read it as an image".to_owned()
}

fn open(path: &Path) -> Result<CFRetained<CGImageSource>, String> {
    let cf_path = CFString::from_str(&path.to_string_lossy());
    let url = CFURL::with_file_system_path(
        None,
        Some(&cf_path),
        CFURLPathStyle::CFURLPOSIXPathStyle,
        false,
    )
    .ok_or_else(not_readable)?;
    // SAFETY: no options dictionary is passed.
    unsafe { CGImageSource::with_url(&url, None) }.ok_or_else(not_readable)
}

/// The image as macOS decodes it, for drawing with Core Graphics.
pub fn cg_image(path: &Path) -> Result<CFRetained<CGImage>, String> {
    let source = open(path)?;
    // SAFETY: no options dictionary is passed.
    unsafe { source.image_at_index(source.primary_image_index(), None) }.ok_or_else(not_readable)
}

/// A small version of the image, at most `max_side` pixels wide or high.
/// Much faster than decoding the whole image: formats with embedded
/// previews or scaled decoding (JPEG, HEIC) never decode every pixel.
pub fn thumbnail(path: &Path, max_side: u32) -> Result<DynamicImage, String> {
    let source = open(path)?;
    let max_side = CFNumber::new_i32(i32::try_from(max_side).unwrap_or(i32::MAX));
    // SAFETY: the keys are valid ImageIO constants.
    let (always, max_size, transform) = unsafe {
        (
            kCGImageSourceCreateThumbnailFromImageAlways,
            kCGImageSourceThumbnailMaxPixelSize,
            kCGImageSourceCreateThumbnailWithTransform,
        )
    };
    let yes: &CFType = CFBoolean::new(true);
    let options = CFDictionary::<CFString, CFType>::from_slices(
        &[always, max_size, transform],
        &[yes, &max_side, yes],
    );
    // SAFETY: the options have the types ImageIO documents for these keys.
    let image = unsafe {
        source.thumbnail_at_index(source.primary_image_index(), Some(options.as_opaque()))
    }
    .ok_or_else(not_readable)?;
    to_pixels(&image)
}

fn decode_with_imageio(path: &Path) -> Result<DynamicImage, String> {
    to_pixels(&*cg_image(path)?)
}

/// Draws the image into an sRGB bitmap, converting from its color profile.
fn to_pixels(image: &CGImage) -> Result<DynamicImage, String> {
    let (width, height) = (CGImage::width(Some(image)), CGImage::height(Some(image)));
    if width == 0 || height == 0 {
        return Err(not_readable());
    }
    let mut pixels = vec![0u8; width * height * 4];
    // SAFETY: the color space name is a valid CoreGraphics constant.
    let srgb =
        CGColorSpace::with_name(Some(unsafe { kCGColorSpaceSRGB })).ok_or("no sRGB color space")?;
    // SAFETY: `pixels` is `height` rows of `width * 4` bytes and outlives the
    // context, which is dropped at the end of this function.
    let context = unsafe {
        CGBitmapContextCreate(
            pixels.as_mut_ptr().cast(),
            width,
            height,
            8,
            width * 4,
            Some(&srgb),
            CGImageAlphaInfo::PremultipliedLast.0 | CGImageByteOrderInfo::Order32Big.0,
        )
    }
    .ok_or("creating a bitmap context failed")?;
    #[allow(clippy::cast_precision_loss)] // Image sides are far below 2^52.
    let rect = CGRect::new(
        CGPoint::new(0.0, 0.0),
        CGSize::new(width as f64, height as f64),
    );
    CGContext::draw_image(Some(&context), rect, Some(image));
    drop(context);

    let width = u32::try_from(width).map_err(|_| "image too large")?;
    let height = u32::try_from(height).map_err(|_| "image too large")?;
    RgbaImage::from_raw(width, height, pixels)
        .map(DynamicImage::ImageRgba8)
        .ok_or_else(|| "pixel buffer size mismatch".to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    const FIXTURES: &str = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../obojeno-core/tests/fixtures"
    );

    #[test]
    fn decodes_like_the_rust_decoder_for_png() {
        let path = Path::new(FIXTURES).join("ember.png");
        let ours = decode_with_imageio(&path).unwrap();
        let reference = image::open(&path).unwrap();
        assert_eq!(
            (ours.width(), ours.height()),
            (reference.width(), reference.height())
        );
        let (a, b) = (ours.to_rgba8(), reference.to_rgba8());
        let max_diff = a
            .pixels()
            .zip(b.pixels())
            .flat_map(|(p, q)| p.0.iter().zip(q.0).map(|(x, y)| x.abs_diff(y)))
            .max()
            .unwrap();
        assert!(max_diff <= 2, "pixels differ by up to {max_diff}");
    }

    #[test]
    fn decodes_heic() {
        // Made with `sips -s format heic` from the core's lagoon.png fixture.
        let heic = Path::new(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/tests/fixtures/lagoon.heic"
        ));
        assert!(
            obojeno_core::palette::open_image(heic).is_err(),
            "precondition: pure Rust cannot"
        );
        let image = decode(heic).unwrap();
        assert_eq!((image.width(), image.height()), (160, 100));
    }

    #[test]
    fn thumbnails_fit_the_size_and_keep_the_aspect_ratio() {
        let heic = Path::new(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/tests/fixtures/lagoon.heic"
        ));
        let thumb = thumbnail(heic, 64).unwrap();
        assert_eq!((thumb.width(), thumb.height()), (64, 40));
    }

    #[test]
    fn rejects_non_images() {
        let err = decode(Path::new(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/Cargo.toml"
        )))
        .unwrap_err();
        assert!(err.contains("macOS cannot read it"), "{err}");
    }
}
