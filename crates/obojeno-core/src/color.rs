use std::fmt;
use std::str::FromStr;

use serde::{Deserialize, Deserializer, Serialize, Serializer};

/// An opaque sRGB color. Serialized as `#rrggbb`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Color {
    pub r: u8,
    pub g: u8,
    pub b: u8,
}

impl Color {
    pub const fn new(r: u8, g: u8, b: u8) -> Self {
        Self { r, g, b }
    }

    /// `#rrggbb`
    pub fn hex(self) -> String {
        format!("#{}", self.hex_stripped())
    }

    /// `rrggbb`
    pub fn hex_stripped(self) -> String {
        format!("{:02x}{:02x}{:02x}", self.r, self.g, self.b)
    }

    /// Relative luminance per WCAG 2.x.
    pub fn luminance(self) -> f64 {
        fn channel(c: u8) -> f64 {
            let c = f64::from(c) / 255.0;
            if c <= 0.04045 {
                c / 12.92
            } else {
                ((c + 0.055) / 1.055).powf(2.4)
            }
        }
        0.0722f64.mul_add(
            channel(self.b),
            0.2126f64.mul_add(channel(self.r), 0.7152 * channel(self.g)),
        )
    }

    /// WCAG contrast ratio between two colors, in `1.0..=21.0`.
    pub fn contrast(self, other: Self) -> f64 {
        let (a, b) = (self.luminance(), other.luminance());
        let (hi, lo) = if a > b { (a, b) } else { (b, a) };
        (hi + 0.05) / (lo + 0.05)
    }
}

impl fmt::Display for Color {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.hex())
    }
}

#[derive(Debug, thiserror::Error)]
#[error("invalid color {0:?}, expected #rrggbb or #rgb")]
pub struct ParseColorError(String);

impl FromStr for Color {
    type Err = ParseColorError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let err = || ParseColorError(s.to_owned());
        let hex = s.strip_prefix('#').unwrap_or(s);
        if !hex.is_ascii() {
            return Err(err());
        }
        let expanded: String = match hex.len() {
            3 => hex.chars().flat_map(|c| [c, c]).collect(),
            6 => hex.to_owned(),
            _ => return Err(err()),
        };
        let channel = |i: usize| u8::from_str_radix(&expanded[i..i + 2], 16).map_err(|_| err());
        Ok(Self::new(channel(0)?, channel(2)?, channel(4)?))
    }
}

impl Serialize for Color {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&self.hex())
    }
}

impl<'de> Deserialize<'de> for Color {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let s = String::deserialize(deserializer)?;
        s.parse().map_err(serde::de::Error::custom)
    }
}

impl From<material_colors::color::Rgb> for Color {
    fn from(c: material_colors::color::Rgb) -> Self {
        Self::new(c.red, c.green, c.blue)
    }
}

impl From<Color> for material_colors::color::Rgb {
    fn from(c: Color) -> Self {
        Self::new(c.r, c.g, c.b)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_long_and_short_hex() {
        assert_eq!(
            "#ff4057".parse::<Color>().unwrap(),
            Color::new(0xff, 0x40, 0x57)
        );
        assert_eq!(
            "abc".parse::<Color>().unwrap(),
            Color::new(0xaa, 0xbb, 0xcc)
        );
        assert!("#12345".parse::<Color>().is_err());
        assert!("#gggggg".parse::<Color>().is_err());
        assert!("#ééé".parse::<Color>().is_err());
    }

    #[test]
    fn formats_hex() {
        let c = Color::new(1, 2, 255);
        assert_eq!(c.hex(), "#0102ff");
        assert_eq!(c.hex_stripped(), "0102ff");
    }

    #[test]
    fn contrast_extremes() {
        let black = Color::new(0, 0, 0);
        let white = Color::new(255, 255, 255);
        assert!((black.contrast(white) - 21.0).abs() < 1e-9);
        assert!((white.contrast(white) - 1.0).abs() < 1e-9);
    }
}
