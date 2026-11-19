//! Wallpaper -> Material You palette.
//!
//! Source color extraction mirrors matugen so the same wallpaper yields the same
//! colors in both tools.

use std::collections::BTreeMap;
use std::fmt;
use std::path::Path;
use std::str::FromStr;

use image::imageops::{self, FilterType};
use image::{DynamicImage, ImageReader};
use material_colors::blend::harmonize;
use material_colors::color::Rgb;
use material_colors::dynamic_color::{DynamicScheme, Variant};
use material_colors::hct::Hct;
use material_colors::quantize::{Quantizer, QuantizerCelebi};
use material_colors::scheme::Scheme;
use material_colors::score::Score;
use serde::{Deserialize, Serialize};

use crate::color::Color;

/// Side length the image is downscaled to before quantizing (same as matugen).
const SAMPLE_SIZE: u32 = 112;
/// Number of colors the quantizer reduces the image to.
const MAX_COLORS: usize = 128;
/// Quantized colors below this chroma are ignored as source candidates.
const MIN_CHROMA: f64 = 5.0;

/// Bumped whenever extraction gives different colors for the same image, so
/// cached source colors are recomputed.
/// 3: wallpapers without color keep their gray instead of Google blue.
pub const EXTRACTION_VERSION: u32 = 3;

/// Whether a color is (nearly) gray. Gray sources make monochrome themes.
pub fn is_achromatic(color: Color) -> bool {
    Hct::new(color.into()).get_chroma() < MIN_CHROMA
}

#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default, Serialize, Deserialize,
)]
#[serde(rename_all = "kebab-case")]
pub enum SchemeVariant {
    Content,
    #[default]
    TonalSpot,
    Expressive,
    Fidelity,
    FruitSalad,
    Monochrome,
    Neutral,
    Rainbow,
    Vibrant,
}

impl SchemeVariant {
    pub const ALL: [Self; 9] = [
        Self::Content,
        Self::TonalSpot,
        Self::Expressive,
        Self::Fidelity,
        Self::FruitSalad,
        Self::Monochrome,
        Self::Neutral,
        Self::Rainbow,
        Self::Vibrant,
    ];

    pub const fn name(self) -> &'static str {
        match self {
            Self::Content => "content",
            Self::TonalSpot => "tonal-spot",
            Self::Expressive => "expressive",
            Self::Fidelity => "fidelity",
            Self::FruitSalad => "fruit-salad",
            Self::Monochrome => "monochrome",
            Self::Neutral => "neutral",
            Self::Rainbow => "rainbow",
            Self::Vibrant => "vibrant",
        }
    }

    const fn material(self) -> Variant {
        match self {
            Self::Content => Variant::Content,
            Self::TonalSpot => Variant::TonalSpot,
            Self::Expressive => Variant::Expressive,
            Self::Fidelity => Variant::Fidelity,
            Self::FruitSalad => Variant::FruitSalad,
            Self::Monochrome => Variant::Monochrome,
            Self::Neutral => Variant::Neutral,
            Self::Rainbow => Variant::Rainbow,
            Self::Vibrant => Variant::Vibrant,
        }
    }
}

impl fmt::Display for SchemeVariant {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.name())
    }
}

#[derive(Debug, thiserror::Error)]
#[error("unknown scheme variant {0:?}")]
pub struct ParseVariantError(String);

impl FromStr for SchemeVariant {
    type Err = ParseVariantError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Self::ALL
            .into_iter()
            .find(|v| v.name() == s)
            .ok_or_else(|| ParseVariantError(s.to_owned()))
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Mode {
    Light,
    Dark,
}

impl Mode {
    pub const fn name(self) -> &'static str {
        match self {
            Self::Light => "light",
            Self::Dark => "dark",
        }
    }

    pub const fn is_dark(self) -> bool {
        matches!(self, Self::Dark)
    }
}

impl fmt::Display for Mode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.name())
    }
}

#[derive(Debug, thiserror::Error)]
#[error("unknown mode {0:?}, expected light or dark")]
pub struct ParseModeError(String);

impl FromStr for Mode {
    type Err = ParseModeError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "light" => Ok(Self::Light),
            "dark" => Ok(Self::Dark),
            _ => Err(ParseModeError(s.to_owned())),
        }
    }
}

/// Material color roles (`primary`, `on_surface`, ...) for one mode.
pub type Roles = BTreeMap<String, Color>;

/// Everything generated from one source color: Material roles and a 16-color
/// terminal palette, for both modes.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Palette {
    pub source: Color,
    pub variant: SchemeVariant,
    pub contrast: f64,
    pub light: Roles,
    pub dark: Roles,
    pub terminal_light: [Color; 16],
    pub terminal_dark: [Color; 16],
}

impl Palette {
    /// A gray source always gets the monochrome scheme: every other variant
    /// would invent a hue the wallpaper does not have.
    pub fn from_source(source: Color, variant: SchemeVariant, contrast: f64) -> Self {
        let variant = if is_achromatic(source) {
            SchemeVariant::Monochrome
        } else {
            variant
        };
        let contrast = contrast.clamp(-1.0, 1.0);
        let light = scheme(source, variant, false, contrast);
        let dark = scheme(source, variant, true, contrast);
        Self {
            source,
            variant,
            contrast,
            terminal_light: terminal(source, variant, &light, Mode::Light),
            terminal_dark: terminal(source, variant, &dark, Mode::Dark),
            light,
            dark,
        }
    }

    pub const fn roles(&self, mode: Mode) -> &Roles {
        match mode {
            Mode::Light => &self.light,
            Mode::Dark => &self.dark,
        }
    }

    pub const fn terminal(&self, mode: Mode) -> &[Color; 16] {
        match mode {
            Mode::Light => &self.terminal_light,
            Mode::Dark => &self.terminal_dark,
        }
    }

    /// Looks up a role; panics on unknown names, which are a programming error.
    pub fn role(&self, mode: Mode, name: &str) -> Color {
        self.roles(mode)[name]
    }
}

/// Opens an image, detecting the format from its contents rather than the
/// extension: wallpapers with the wrong extension are common and macOS shows
/// them fine.
pub fn open_image(path: &Path) -> image::ImageResult<DynamicImage> {
    ImageReader::open(path)?.with_guessed_format()?.decode()
}

/// Picks the source color of an image the same way matugen does: downscale,
/// quantize, drop near-gray clusters, score. Falls back to Google blue for
/// images without any usable color.
pub fn source_color(image: &DynamicImage) -> Color {
    source_candidates(image)[0]
}

/// Ranked source color candidates, best first. Never empty.
pub fn source_candidates(image: &DynamicImage) -> Vec<Color> {
    let small = imageops::resize(
        &image.to_rgba8(),
        SAMPLE_SIZE,
        SAMPLE_SIZE,
        FilterType::Triangle,
    );
    let pixels: Vec<Rgb> = small
        .pixels()
        .filter(|p| p.0[3] == u8::MAX)
        .map(|p| Rgb::new(p.0[0], p.0[1], p.0[2]))
        .collect();
    let mut quantized = QuantizerCelebi::quantize(&pixels, MAX_COLORS);
    let most_common = quantized
        .color_to_count
        .iter()
        .max_by_key(|&(_, &count)| count)
        .map(|(&rgb, _)| Color::from(rgb));
    quantized
        .color_to_count
        .retain(|&rgb, _| Hct::new(rgb).get_chroma() >= MIN_CHROMA);
    // Unlike matugen, which falls back to Google blue, a wallpaper without
    // color keeps its own gray (and so gets a monochrome theme).
    if let (true, Some(gray)) = (quantized.color_to_count.is_empty(), most_common) {
        return vec![gray];
    }
    Score::score(&quantized.color_to_count, None, None, None)
        .into_iter()
        .map(Color::from)
        .collect()
}

fn scheme(source: Color, variant: SchemeVariant, is_dark: bool, contrast: f64) -> Roles {
    let dynamic =
        DynamicScheme::by_variant(source.into(), &variant.material(), is_dark, Some(contrast));
    Scheme::from(dynamic)
        .into_iter()
        .map(|(name, rgb)| (name.to_owned(), rgb.into()))
        .collect()
}

/// Canonical HCT hues of the six ANSI accent colors: red, green, yellow, blue,
/// magenta, cyan (in ANSI order 1..=6).
const ANSI_HUES: [f64; 6] = [25.0, 140.0, 90.0, 265.0, 330.0, 200.0];

/// 16 terminal colors: background-ish blacks and foreground-ish whites from the
/// scheme's neutrals, and six accents at their canonical hues pulled towards
/// the source color so they sit in the same family as the wallpaper.
fn terminal(source: Color, variant: SchemeVariant, roles: &Roles, mode: Mode) -> [Color; 16] {
    let role = |name: &str| roles[name];
    // Monochrome schemes still need distinguishable ANSI colors, just quieter.
    let (chroma, bright_chroma) = match variant {
        SchemeVariant::Monochrome | SchemeVariant::Neutral => (24.0, 32.0),
        _ => (48.0, 60.0),
    };
    let (tone, bright_tone) = match mode {
        Mode::Dark => (72.0, 82.0),
        Mode::Light => (42.0, 34.0),
    };
    let accent = |hue: f64, chroma: f64, tone: f64| -> Color {
        let base: Rgb = Hct::from(hue, chroma, tone).into();
        harmonize(base, source.into()).into()
    };

    let mut colors = [Color::new(0, 0, 0); 16];
    colors[0] = role("surface_container_high");
    colors[7] = role("on_surface_variant");
    colors[8] = role("outline");
    colors[15] = role("on_surface");
    for (i, hue) in ANSI_HUES.into_iter().enumerate() {
        colors[i + 1] = accent(hue, chroma, tone);
        colors[i + 9] = accent(hue, bright_chroma, bright_tone);
    }
    colors
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::{Rgba, RgbaImage};

    fn solid(r: u8, g: u8, b: u8) -> DynamicImage {
        DynamicImage::ImageRgba8(RgbaImage::from_pixel(64, 64, Rgba([r, g, b, 255])))
    }

    #[test]
    fn opens_images_with_misleading_extension() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("wallpaper.png");
        solid(10, 20, 200)
            .to_rgb8()
            .save_with_format(&path, image::ImageFormat::Jpeg)
            .unwrap();
        assert!(
            image::open(&path).is_err(),
            "precondition: extension-based open fails"
        );
        let img = open_image(&path).unwrap();
        assert_eq!((img.width(), img.height()), (64, 64));
    }

    #[test]
    fn variant_names_round_trip() {
        for v in SchemeVariant::ALL {
            assert_eq!(v.name().parse::<SchemeVariant>().unwrap(), v);
        }
    }

    #[test]
    fn source_of_solid_image_is_that_color() {
        let c = source_color(&solid(0xc0, 0x20, 0x40));
        assert_eq!(c, Color::new(0xc0, 0x20, 0x40));
    }

    #[test]
    fn colorless_images_keep_their_gray() {
        for (r, g, b) in [(128, 128, 128), (0, 0, 0), (255, 255, 255)] {
            let source = source_color(&solid(r, g, b));
            assert_eq!(source, Color::new(r, g, b));
            let palette = Palette::from_source(source, SchemeVariant::TonalSpot, 0.0);
            assert_eq!(palette.variant, SchemeVariant::Monochrome);
            for mode in [Mode::Light, Mode::Dark] {
                // Errors stay red even in monochrome themes.
                for (role, &c) in palette
                    .roles(mode)
                    .iter()
                    .filter(|(r, _)| !r.contains("error"))
                {
                    assert!(is_achromatic(c), "{mode} {role} {c} has a hue");
                }
            }
        }
    }

    #[test]
    fn fully_transparent_images_fall_back_to_blue() {
        let img = DynamicImage::ImageRgba8(RgbaImage::from_pixel(8, 8, Rgba([9, 9, 9, 0])));
        assert_eq!(source_color(&img), Color::new(66, 133, 244));
    }

    #[test]
    fn transparent_pixels_are_ignored() {
        let mut img = RgbaImage::from_pixel(64, 64, Rgba([0, 200, 0, 0]));
        for x in 0..64 {
            for y in 0..32 {
                img.put_pixel(x, y, Rgba([0x30, 0x40, 0xd0, 255]));
            }
        }
        let c = source_color(&DynamicImage::ImageRgba8(img));
        let hue = Hct::new(c.into()).get_hue();
        assert!(
            (220.0..300.0).contains(&hue),
            "expected blue, got {c} (hue {hue})"
        );
    }

    #[test]
    fn palette_has_all_roles_in_both_modes() {
        let p = Palette::from_source(Color::new(0xff, 0x40, 0x57), SchemeVariant::Content, 0.0);
        assert_eq!(p.light.len(), 49);
        assert_eq!(p.dark.len(), 49);
        assert!(
            p.role(Mode::Dark, "surface").luminance() < p.role(Mode::Light, "surface").luminance()
        );
    }

    #[test]
    fn terminal_colors_are_readable_on_background() {
        for variant in SchemeVariant::ALL {
            for source in [
                Color::new(0xff, 0x40, 0x57),
                Color::new(0x20, 0x60, 0x30),
                Color::new(0x80, 0x80, 0x80),
            ] {
                let p = Palette::from_source(source, variant, 0.0);
                for mode in [Mode::Light, Mode::Dark] {
                    let bg = p.role(mode, "surface");
                    for (i, c) in p.terminal(mode).iter().enumerate().skip(1) {
                        let ratio = c.contrast(bg);
                        assert!(
                            ratio >= 3.0,
                            "{variant} {mode} {source}: color{i} {c} on {bg} = {ratio:.2}"
                        );
                    }
                }
            }
        }
    }

    #[test]
    fn terminal_accents_keep_their_hue_family() {
        let p = Palette::from_source(Color::new(0xff, 0x40, 0x57), SchemeVariant::Content, 0.0);
        for mode in [Mode::Light, Mode::Dark] {
            let t = p.terminal(mode);
            for (i, expected) in ANSI_HUES.iter().enumerate() {
                for c in [t[i + 1], t[i + 9]] {
                    let hue = Hct::new(c.into()).get_hue();
                    let diff = (hue - expected + 540.0).rem_euclid(360.0) - 180.0;
                    assert!(
                        diff.abs() <= 20.0,
                        "{mode} color{} {c}: hue {hue:.0}, expected ~{expected}",
                        i + 1
                    );
                }
            }
        }
    }
}
