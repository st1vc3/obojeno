//! Decoding images with macOS ImageIO, so every format the desktop can show
//! works, HEIC included (all built-in wallpapers are HEIC).
//!
//! Every entry point runs in its own autorelease pool: ImageIO hands out
//! autoreleased objects, and threads Rust starts have no pool to free them.
//!
//! The daemon decodes in helper processes (see [`decode_in_helpers`]), and
//! hands pixels to Core Animation as `IOSurface`s, never as drawn images:
//! the system keeps memory from both for reuse, which a long-running process
//! would otherwise hold on to.

use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};

use image::{DynamicImage, RgbaImage};
use objc2::rc::autoreleasepool;
use objc2_core_foundation::{
    CFBoolean, CFDictionary, CFNumber, CFRetained, CFString, CFType, CFURL, CFURLPathStyle,
};
use objc2_core_graphics::{
    CGBitmapContextCreate, CGBitmapContextCreateImage, CGColorSpace, CGImage, CGImageAlphaInfo,
    CGImageByteOrderInfo, kCGColorSpaceSRGB,
};
use objc2_image_io::{
    CGImageSource, kCGImagePropertyOrientation, kCGImagePropertyPixelHeight,
    kCGImagePropertyPixelWidth, kCGImageSourceShouldCache, kCGImageSourceSubsampleFactor,
};
use objc2_io_surface::{
    IOSurfaceLockOptions, IOSurfaceRef, kIOSurfaceBytesPerElement, kIOSurfaceColorSpace,
    kIOSurfaceHeight, kIOSurfacePixelFormat, kIOSurfaceWidth,
};

/// Changes when source colors may change: core's extraction version, plus
/// how images are decoded here (1: through ImageIO). Images are analyzed at
/// full size, like matugen does; a smaller copy shifts the colors.
pub const EXTRACTION_VERSION: u32 = obojeno_core::palette::EXTRACTION_VERSION * 100 + 1;

/// Decodes `path` to sRGB pixels, converting from embedded color profiles
/// (Display P3 wallpapers, for example): at full size, or given `min_side`,
/// reduced while decoding as far as the longest side stays that long. Falls
/// back to the pure-Rust decoders for anything ImageIO rejects.
pub fn decode(path: &Path, min_side: Option<u32>) -> Result<DynamicImage, String> {
    decode_with_imageio(path, min_side).or_else(|imageio_error| {
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
    let options = no_cache();
    // SAFETY: the options have the types ImageIO documents for their keys.
    unsafe { CGImageSource::with_url(&url, Some(options.as_opaque())) }.ok_or_else(not_readable)
}

/// Options that keep ImageIO from caching decoded pixels. Its cache is for
/// apps that draw an image again and again; obojeno decodes each image once,
/// and the cache would keep every wallpaper decoded (over 100 MB each).
fn no_cache() -> CFRetained<CFDictionary<CFString, CFType>> {
    // SAFETY: the key is a valid ImageIO constant.
    let key = unsafe { kCGImageSourceShouldCache };
    let no: &CFType = CFBoolean::new(false);
    CFDictionary::from_slices(&[key], &[no])
}

/// What an image file says about itself, read without decoding it.
struct Info {
    /// Pixels as stored, before turning upright.
    width: u32,
    height: u32,
    /// The EXIF orientation, 1 (upright) to 8.
    orientation: u32,
}

impl Info {
    fn read(source: &CGImageSource) -> Result<Self, String> {
        // SAFETY: no options dictionary is passed.
        let properties = unsafe { source.properties_at_index(source.primary_image_index(), None) }
            .ok_or_else(not_readable)?;
        // SAFETY: image properties map CFString keys to property values.
        let properties: CFRetained<CFDictionary<CFString, CFType>> =
            unsafe { CFRetained::cast_unchecked(properties) };
        let number = |key: &CFString| {
            properties
                .get(key)
                .and_then(|value| value.downcast::<CFNumber>().ok())
                .and_then(|n| n.as_i64())
                .and_then(|n| u32::try_from(n).ok())
        };
        // SAFETY: the keys are valid ImageIO constants.
        let (width_key, height_key, orientation_key) = unsafe {
            (
                kCGImagePropertyPixelWidth,
                kCGImagePropertyPixelHeight,
                kCGImagePropertyOrientation,
            )
        };
        Ok(Self {
            width: number(width_key).ok_or_else(not_readable)?,
            height: number(height_key).ok_or_else(not_readable)?,
            orientation: number(orientation_key).unwrap_or(1),
        })
    }

    /// Width and height as shown, upright.
    const fn upright_size(&self) -> (u32, u32) {
        if self.orientation >= 5 {
            (self.height, self.width)
        } else {
            (self.width, self.height)
        }
    }
}

/// The image's size as shown (upright), in pixels. Also how obojeno checks
/// that macOS can read a file as an image, without decoding it.
pub fn size(path: &Path) -> Result<(u32, u32), String> {
    autoreleasepool(|_| Ok(Info::read(&*open(path)?)?.upright_size()))
}

/// Whether decoding happens in helper processes: in the daemon, which
/// lives long, while short-lived commands decode themselves.
static IN_HELPERS: AtomicBool = AtomicBool::new(false);

/// Decodes in helper processes from now on. ImageIO decodes into memory
/// the system allocator keeps for reuse (the size of the largest wallpaper
/// decoded so far); a helper gives it back when it exits.
pub fn decode_in_helpers() {
    IN_HELPERS.store(true, Ordering::Relaxed);
}

/// The image's pixels in `layout`, upright, decoded at full size or, given
/// `min_side`, reduced while decoding as far as it stays at least that long.
fn pixels(
    path: &Path,
    min_side: Option<u32>,
    layout: PixelLayout,
) -> Result<(usize, usize, Vec<u8>), String> {
    if IN_HELPERS.load(Ordering::Relaxed) {
        pixels_in_helper(path, min_side, layout)
    } else {
        pixels_here(path, min_side, layout)
    }
}

/// [`pixels`] through `obojeno decode-pixels`, which writes them to a
/// temporary file and prints their size.
fn pixels_in_helper(
    path: &Path,
    min_side: Option<u32>,
    layout: PixelLayout,
) -> Result<(usize, usize, Vec<u8>), String> {
    let out = tempfile::NamedTempFile::new().map_err(|e| format!("a temporary file: {e}"))?;
    // An absolute program path lets std spawn without forking, which is not
    // safe while other threads are inside CoreFoundation.
    let program = std::env::current_exe().map_err(|e| format!("finding obojeno: {e}"))?;
    let mut command = std::process::Command::new(program);
    command
        .arg("decode-pixels")
        .arg(path)
        .arg("--out")
        .arg(out.path());
    if let Some(side) = min_side {
        command.arg("--min-side").arg(side.to_string());
    }
    if matches!(layout, PixelLayout::Bgra) {
        command.arg("--bgra");
    }
    let output = command
        .output()
        .map_err(|e| format!("starting the decoder: {e}"))?;
    if !output.status.success() {
        let message = String::from_utf8_lossy(&output.stderr);
        return Err(message.trim().trim_start_matches("obojeno: ").to_owned());
    }
    let size = String::from_utf8_lossy(&output.stdout);
    let (width, height) = size
        .trim()
        .split_once(' ')
        .and_then(|(w, h)| Some((w.parse::<usize>().ok()?, h.parse::<usize>().ok()?)))
        .ok_or_else(|| format!("the decoder said {size:?}"))?;
    let bytes = std::fs::read(out.path()).map_err(|e| format!("reading the pixels: {e}"))?;
    if bytes.len() != width * height * 4 {
        return Err("the decoder wrote the wrong number of pixels".to_owned());
    }
    Ok((width, height, bytes))
}

/// `obojeno decode-pixels`: decodes here and writes the pixels to `out`,
/// returning their size.
pub fn write_pixels(
    path: &Path,
    min_side: Option<u32>,
    bgra: bool,
    out: &Path,
) -> Result<(usize, usize), String> {
    let layout = if bgra {
        PixelLayout::Bgra
    } else {
        PixelLayout::Rgba
    };
    let (width, height, bytes) = pixels_here(path, min_side, layout)?;
    std::fs::write(out, bytes).map_err(|e| format!("writing {}: {e}", out.display()))?;
    Ok((width, height))
}

/// [`pixels`] in this process.
///
/// ImageIO only decodes here; it never draws or scales. Drawing makes Core
/// Graphics keep a full-size copy of the pixels in a cache that outlives the
/// image, so a daemon that sees many wallpapers would hold on to all of
/// them. vImage converts the decoded pixels instead.
fn pixels_here(
    path: &Path,
    min_side: Option<u32>,
    layout: PixelLayout,
) -> Result<(usize, usize, Vec<u8>), String> {
    autoreleasepool(|_| {
        let source = open(path)?;
        let info = Info::read(&source)?;
        let longest = info.width.max(info.height);
        // ImageIO decodes JPEG, HEIF and some other formats directly at a
        // half, quarter or eighth of the size.
        let factor = min_side.map_or(1, |side| {
            [8, 4, 2]
                .into_iter()
                .find(|f| longest / f >= side)
                .unwrap_or(1)
        });
        // SAFETY: the keys are valid ImageIO constants.
        let (cache_key, factor_key) =
            unsafe { (kCGImageSourceShouldCache, kCGImageSourceSubsampleFactor) };
        let no: &CFType = CFBoolean::new(false);
        let factor_value = CFNumber::new_i32(i32::try_from(factor).unwrap_or(1));
        let options = CFDictionary::<CFString, CFType>::from_slices(
            &[cache_key, factor_key],
            &[no, &factor_value],
        );
        // SAFETY: the options have the types ImageIO documents for their keys.
        let image = unsafe {
            source.image_at_index(source.primary_image_index(), Some(options.as_opaque()))
        }
        .ok_or_else(not_readable)?;
        let (width, height, bytes) = convert(&image, layout)?;
        Ok(upright(width, height, bytes, info.orientation))
    })
}

/// Turns 4-byte pixels upright for their EXIF orientation: mirrored (2, 4,
/// 5, 7) and rotated by quarter turns (3, 5 to 8).
fn upright(
    width: usize,
    height: usize,
    bytes: Vec<u8>,
    orientation: u32,
) -> (usize, usize, Vec<u8>) {
    use image::imageops::{flip_horizontal, flip_vertical, rotate90, rotate180, rotate270};
    if orientation <= 1 || orientation > 8 {
        return (width, height, bytes);
    }
    // The pixel order within each four bytes does not matter for moving
    // pixels around, so RGBA stands in for any layout.
    #[expect(clippy::cast_possible_truncation)] // Decoded images fit u32.
    let Some(image) = RgbaImage::from_raw(width as u32, height as u32, bytes) else {
        unreachable!("four bytes per pixel")
    };
    let turned = match orientation {
        2 => flip_horizontal(&image),
        3 => rotate180(&image),
        4 => flip_vertical(&image),
        5 => flip_horizontal(&rotate90(&image)),
        6 => rotate90(&image),
        7 => flip_vertical(&rotate90(&image)),
        _ => rotate270(&image),
    };
    (
        turned.width() as usize,
        turned.height() as usize,
        turned.into_raw(),
    )
}

fn decode_with_imageio(path: &Path, min_side: Option<u32>) -> Result<DynamicImage, String> {
    let (width, height, bytes) = pixels(path, min_side, PixelLayout::Rgba)?;
    let width = u32::try_from(width).map_err(|_| "image too large")?;
    let height = u32::try_from(height).map_err(|_| "image too large")?;
    RgbaImage::from_raw(width, height, bytes)
        .map(DynamicImage::ImageRgba8)
        .ok_or_else(|| "pixel buffer size mismatch".to_owned())
}

/// How converted pixels are laid out.
#[derive(Debug, Clone, Copy)]
enum PixelLayout {
    /// What the `image` crate works with.
    Rgba,
    /// What Core Animation draws as is, without a converted copy.
    Bgra,
}

impl PixelLayout {
    const fn bitmap_info(self) -> u32 {
        match self {
            Self::Rgba => {
                CGImageAlphaInfo::PremultipliedLast.0 | CGImageByteOrderInfo::Order32Big.0
            }
            Self::Bgra => {
                CGImageAlphaInfo::PremultipliedFirst.0 | CGImageByteOrderInfo::Order32Little.0
            }
        }
    }
}

/// Decoded pixels in the layout Core Animation draws as is (BGRA,
/// premultiplied, sRGB). Images it would otherwise convert or decode
/// itself leave copies behind in Core Graphics caches.
pub struct LayerPixels {
    width: usize,
    height: usize,
    bytes: Vec<u8>,
}

impl LayerPixels {
    /// The image at `path`, at most `max_side` pixels wide or high.
    pub fn decode(path: &Path, max_side: u32) -> Result<Self, String> {
        let (width, height, bytes) = pixels(path, Some(max_side), PixelLayout::Bgra)?;
        Self {
            width,
            height,
            bytes,
        }
        .scaled(max_side)
    }

    /// A copy at most `max_side` pixels wide or high, keeping the aspect
    /// ratio; never larger.
    pub fn scaled(&self, max_side: u32) -> Result<Self, String> {
        let max_side = max_side.max(1) as usize;
        let longest = self.width.max(self.height);
        if longest <= max_side {
            return Ok(Self {
                width: self.width,
                height: self.height,
                bytes: self.bytes.clone(),
            });
        }
        let scaled_side = |side: usize| (side * max_side).div_ceil(longest).max(1);
        let (width, height) = (scaled_side(self.width), scaled_side(self.height));
        let mut bytes = vec![0u8; width * height * 4];
        let source = vimage::Buffer {
            data: self.bytes.as_ptr().cast_mut().cast(),
            height: self.height,
            width: self.width,
            row_bytes: self.width * 4,
        };
        let destination = vimage::Buffer {
            data: bytes.as_mut_ptr().cast(),
            height,
            width,
            row_bytes: width * 4,
        };
        // vImage needs scratch memory, as large as the source for big
        // reductions. Given none, it would take it from the system
        // allocator, which keeps it after the call; ours gives it back.
        // SAFETY: both buffers describe memory of their stated size that
        // lives through the call; vImage only reads the source.
        let scratch = unsafe {
            vimage::vImageScale_ARGB8888(
                &raw const source,
                &raw const destination,
                std::ptr::null_mut(),
                vimage::HIGH_QUALITY_RESAMPLING | vimage::GET_TEMP_BUFFER_SIZE,
            )
        };
        let scratch = usize::try_from(scratch)
            .map_err(|_| format!("scaling the pixels failed (vImage error {scratch})"))?;
        let mut scratch = vec![0u8; scratch];
        // SAFETY: as above, with scratch memory of the size vImage asked for.
        let error = unsafe {
            vimage::vImageScale_ARGB8888(
                &raw const source,
                &raw const destination,
                scratch.as_mut_ptr().cast(),
                vimage::HIGH_QUALITY_RESAMPLING,
            )
        };
        if error != 0 {
            return Err(format!("scaling the pixels failed (vImage error {error})"));
        }
        Ok(Self {
            width,
            height,
            bytes,
        })
    }

    /// The pixels as the `image` crate's RGBA, for processing them there.
    pub fn to_rgba(&self) -> RgbaImage {
        let bytes = self
            .bytes
            .as_chunks::<4>()
            .0
            .iter()
            .flat_map(|&[b, g, r, a]| [r, g, b, a])
            .collect();
        #[expect(clippy::cast_possible_truncation)] // Decoded images fit u32.
        RgbaImage::from_raw(self.width as u32, self.height as u32, bytes)
            .expect("four bytes per pixel")
    }

    /// Opaque gray pixels, rows from the top.
    pub fn from_gray(width: usize, height: usize, gray: &[u8]) -> Self {
        debug_assert_eq!(gray.len(), width * height);
        Self {
            width,
            height,
            bytes: gray.iter().flat_map(|&v| [v, v, v, u8::MAX]).collect(),
        }
    }

    /// Opaque pixels from the `image` crate.
    pub fn from_rgba(pixels: &RgbaImage) -> Self {
        let bytes = pixels
            .as_raw()
            .as_chunks::<4>()
            .0
            .iter()
            .flat_map(|&[r, g, b, _]| [b, g, r, u8::MAX])
            .collect();
        Self {
            width: pixels.width() as usize,
            height: pixels.height() as usize,
            bytes,
        }
    }

    /// The pixels as layer contents Core Animation shows as they are: an
    /// `IOSurface`, which the GPU reads directly. A `CGImage` would make it
    /// convert a copy of each image first, the size of the screen.
    pub fn into_surface(self) -> Option<CFRetained<IOSurfaceRef>> {
        let number = |n: usize| CFNumber::new_i64(i64::try_from(n).unwrap_or(0));
        // SAFETY: the keys are valid IOSurface constants.
        let keys = unsafe {
            [
                kIOSurfaceWidth,
                kIOSurfaceHeight,
                kIOSurfaceBytesPerElement,
                kIOSurfacePixelFormat,
            ]
        };
        // 'BGRA': four bytes, blue first, as `PixelLayout::Bgra`.
        let values = [
            number(self.width),
            number(self.height),
            number(4),
            number(0x4247_5241),
        ];
        let properties = CFDictionary::<CFString, CFNumber>::from_slices(
            &keys,
            &values.iter().map(|v| &**v).collect::<Vec<_>>(),
        );
        // SAFETY: the properties have the types IOSurface documents.
        let surface = unsafe { IOSurfaceRef::new(properties.as_opaque()) }?;
        // SAFETY: locked for writing below; null asks for no seed.
        if unsafe { surface.lock(IOSurfaceLockOptions::empty(), std::ptr::null_mut()) } != 0 {
            return None;
        }
        let stride = surface.bytes_per_row();
        let base = surface.base_address().as_ptr().cast::<u8>();
        let row = self.width * 4;
        for (y, pixels) in self.bytes.chunks_exact(row).enumerate() {
            // SAFETY: the surface holds `height` rows of `stride` bytes, at
            // least `row` each, and stays locked while they are written.
            unsafe { std::ptr::copy_nonoverlapping(pixels.as_ptr(), base.add(y * stride), row) };
        }
        // SAFETY: unlocking the lock taken above.
        unsafe { surface.unlock(IOSurfaceLockOptions::empty(), std::ptr::null_mut()) };
        // Tagged as sRGB, which the pixels are in.
        // SAFETY: the color space name is a valid CoreGraphics constant.
        let srgb = CGColorSpace::with_name(Some(unsafe { kCGColorSpaceSRGB }))?;
        if let Some(list) = srgb.property_list() {
            // SAFETY: a color space property list is what this key holds.
            unsafe { surface.set_value(kIOSurfaceColorSpace, &list) };
        }
        Some(surface)
    }
}

/// The image as 8-bit sRGB pixels, converted from its color profile.
///
/// Through vImage rather than by drawing into a bitmap context: drawing makes
/// Core Graphics keep its own copy of the decoded pixels in a cache that
/// outlives the image, over 100 MB for each wallpaper a long-running daemon
/// ever looked at.
fn convert(image: &CGImage, layout: PixelLayout) -> Result<(usize, usize, Vec<u8>), String> {
    let (width, height) = (CGImage::width(Some(image)), CGImage::height(Some(image)));
    if width == 0 || height == 0 {
        return Err(not_readable());
    }
    // SAFETY: the color space name is a valid CoreGraphics constant.
    let srgb =
        CGColorSpace::with_name(Some(unsafe { kCGColorSpaceSRGB })).ok_or("no sRGB color space")?;
    let mut pixels = vec![0u8; width * height * 4];
    let mut buffer = vimage::Buffer {
        data: pixels.as_mut_ptr().cast(),
        height,
        width,
        row_bytes: width * 4,
    };
    let mut format = vimage::CGImageFormat {
        bits_per_component: 8,
        bits_per_pixel: 32,
        color_space: std::ptr::from_ref(&*srgb),
        bitmap_info: layout.bitmap_info(),
        version: 0,
        decode: std::ptr::null(),
        rendering_intent: 0,
    };
    // SAFETY: `buffer` points to `height` rows of `row_bytes` bytes, which
    // `pixels` holds for the whole call; `format` and `image` are valid, and
    // NO_ALLOCATE makes vImage write into that buffer.
    let error = unsafe {
        vimage::vImageBuffer_InitWithCGImage(
            &raw mut buffer,
            &raw mut format,
            std::ptr::null(),
            image,
            vimage::NO_ALLOCATE,
        )
    };
    if error != 0 {
        return Err(format!(
            "converting the pixels failed (vImage error {error})"
        ));
    }
    Ok((width, height, pixels))
}

/// The few vImage (Accelerate) declarations `convert` needs, from
/// `vImage_Types.h` and `vImage_Utilities.h`.
mod vimage {
    use objc2_core_foundation::CGFloat;
    use objc2_core_graphics::{CGColorSpace, CGImage};

    /// Write into the caller's buffer instead of allocating one.
    pub const NO_ALLOCATE: u32 = 512;
    /// Resample with a better (Lanczos) kernel when scaling.
    pub const HIGH_QUALITY_RESAMPLING: u32 = 32;
    /// Asks for the size of the scratch memory instead of scaling.
    pub const GET_TEMP_BUFFER_SIZE: u32 = 128;

    #[repr(C)]
    pub struct Buffer {
        pub data: *mut std::ffi::c_void,
        pub height: usize,
        pub width: usize,
        pub row_bytes: usize,
    }

    #[repr(C)]
    pub struct CGImageFormat {
        pub bits_per_component: u32,
        pub bits_per_pixel: u32,
        pub color_space: *const CGColorSpace,
        pub bitmap_info: u32,
        pub version: u32,
        pub decode: *const CGFloat,
        pub rendering_intent: i32,
    }

    #[link(name = "Accelerate", kind = "framework")]
    unsafe extern "C" {
        pub fn vImageBuffer_InitWithCGImage(
            buf: *mut Buffer,
            format: *mut CGImageFormat,
            background_color: *const CGFloat,
            image: &CGImage,
            flags: u32,
        ) -> isize;

        pub fn vImageScale_ARGB8888(
            source: *const Buffer,
            destination: *const Buffer,
            temp_buffer: *mut std::ffi::c_void,
            flags: u32,
        ) -> isize;
    }
}

/// How the bytes given to [`image_from_pixels`] are laid out.
#[derive(Clone, Copy)]
pub struct Bitmap<'a> {
    pub width: usize,
    pub height: usize,
    pub bytes_per_row: usize,
    /// `None` for alpha-only images (masks).
    pub color_space: Option<&'a CGColorSpace>,
    /// `CGImageAlphaInfo` and byte order flags.
    pub info: u32,
}

/// An image copied from `pixels`, which hold `bitmap.height` rows from the
/// top. `None` if the layout does not fit them.
pub fn image_from_pixels(pixels: &mut [u8], bitmap: Bitmap<'_>) -> Option<CFRetained<CGImage>> {
    if bitmap.width == 0 || pixels.len() < bitmap.bytes_per_row * bitmap.height {
        return None;
    }
    // SAFETY: `pixels` holds `height` rows of `bytes_per_row` bytes (checked
    // above) and outlives the context, which is dropped at the end of this
    // block; the image copies the pixels.
    unsafe {
        let context = CGBitmapContextCreate(
            pixels.as_mut_ptr().cast(),
            bitmap.width,
            bitmap.height,
            8,
            bitmap.bytes_per_row,
            bitmap.color_space,
            bitmap.info,
        )?;
        CGBitmapContextCreateImage(Some(&context))
    }
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
        let ours = decode_with_imageio(&path, None).unwrap();
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
        let image = decode(heic, None).unwrap();
        assert_eq!((image.width(), image.height()), (160, 100));
    }

    #[test]
    fn orientation_turns_pixels_upright() {
        // 3 x 2, stored as rows of pixels numbered 1 to 6:
        //   1 2 3
        //   4 5 6
        let stored: Vec<u8> = (1..=6).flat_map(|n| [n, 0, 0, 0]).collect();
        let shown = |orientation| {
            let (w, h, bytes) = upright(3, 2, stored.clone(), orientation);
            let ids: Vec<u8> = bytes.chunks(4).map(|p| p[0]).collect();
            (w, h, ids)
        };
        assert_eq!(shown(1), (3, 2, vec![1, 2, 3, 4, 5, 6]));
        assert_eq!(shown(2), (3, 2, vec![3, 2, 1, 6, 5, 4]), "mirrored");
        assert_eq!(shown(3), (3, 2, vec![6, 5, 4, 3, 2, 1]), "upside down");
        assert_eq!(shown(4), (3, 2, vec![4, 5, 6, 1, 2, 3]), "flipped");
        assert_eq!(shown(5), (2, 3, vec![1, 4, 2, 5, 3, 6]), "transposed");
        assert_eq!(
            shown(6),
            (2, 3, vec![4, 1, 5, 2, 6, 3]),
            "a quarter turn clockwise"
        );
        assert_eq!(shown(7), (2, 3, vec![6, 3, 5, 2, 4, 1]), "transversed");
        assert_eq!(
            shown(8),
            (2, 3, vec![3, 6, 2, 5, 1, 4]),
            "a quarter turn back"
        );
    }

    #[test]
    fn photos_come_out_upright() {
        // ember.png as a JPEG stored sideways: 160 x 100 pixels with EXIF
        // orientation 6, shown as 100 x 160.
        let rotated = Path::new(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/tests/fixtures/ember-rotated.jpg"
        ));
        assert_eq!(size(rotated).unwrap(), (100, 160));
        let decoded = decode(rotated, None).unwrap().to_rgba8();
        assert_eq!(decoded.dimensions(), (100, 160));
        // Turned the right way: like the original turned clockwise, up to
        // JPEG's loss.
        let original = image::open(Path::new(FIXTURES).join("ember.png"))
            .unwrap()
            .rotate90()
            .to_rgba8();
        let total: u64 = decoded
            .pixels()
            .zip(original.pixels())
            .flat_map(|(p, q)| p.0.iter().zip(q.0).map(|(x, y)| u64::from(x.abs_diff(y))))
            .sum();
        let mean = total / u64::from(decoded.width() * decoded.height() * 4);
        assert!(mean <= 4, "off by {mean} on average");
        let layer = LayerPixels::decode(rotated, 1000).unwrap();
        assert_eq!((layer.width, layer.height), (100, 160));
    }

    #[test]
    fn size_reads_the_upright_size_without_decoding() {
        let heic = Path::new(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/tests/fixtures/lagoon.heic"
        ));
        assert_eq!(size(heic).unwrap(), (160, 100));
        assert!(
            size(Path::new(concat!(
                env!("CARGO_MANIFEST_DIR"),
                "/Cargo.toml"
            )))
            .is_err()
        );
    }

    #[test]
    fn layer_pixels_scale_down_keeping_the_aspect_ratio() {
        let heic = Path::new(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/tests/fixtures/lagoon.heic"
        ));
        let pixels = LayerPixels::decode(heic, 64).unwrap();
        assert_eq!((pixels.width, pixels.height), (64, 40));
        let rgba = pixels.to_rgba();
        assert_eq!(rgba.dimensions(), (64, 40));
        // Back and forth between the layouts keeps the pixels.
        let again = LayerPixels::from_rgba(&rgba);
        assert_eq!(again.bytes, pixels.bytes);
        // Never scaled up.
        assert_eq!(pixels.scaled(1000).unwrap().width, 64);
        let surface = pixels.into_surface().unwrap();
        assert_eq!((surface.width(), surface.height()), (64, 40));
    }

    #[test]
    fn rejects_non_images() {
        let err = decode(
            Path::new(concat!(env!("CARGO_MANIFEST_DIR"), "/Cargo.toml")),
            None,
        )
        .unwrap_err();
        assert!(err.contains("macOS cannot read it"), "{err}");
    }
}
