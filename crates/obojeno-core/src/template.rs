//! Template rendering with matugen-compatible variables.
//!
//! Every color is available per mode and for the current mode (`default`), in
//! several formats: `{{colors.primary.default.hex}}`, `{{colors.surface.dark.rgb}}`,
//! `{{term.color4.light.hex_stripped}}`. Plain values: `{{mode}}`,
//! `{{wallpaper}}`, `{{source_color.hex}}`, plus per-target options and paths.

use std::collections::BTreeMap;

use serde::Serialize;

use crate::color::Color;
use crate::palette::{Mode, Palette};

#[derive(Debug, thiserror::Error)]
#[error("{0:#}")]
pub struct TemplateError(Box<upon::Error>);

impl From<upon::Error> for TemplateError {
    fn from(e: upon::Error) -> Self {
        Self(Box::new(e))
    }
}

/// One color in every format templates can ask for.
#[derive(Debug, Clone, Serialize)]
struct Formats {
    hex: String,
    hex_stripped: String,
    rgb: String,
    rgba: String,
    hsl: String,
    red: u8,
    green: u8,
    blue: u8,
}

impl From<Color> for Formats {
    fn from(c: Color) -> Self {
        let (h, s, l) = hsl(c);
        Self {
            hex: c.hex(),
            hex_stripped: c.hex_stripped(),
            rgb: format!("rgb({}, {}, {})", c.r, c.g, c.b),
            rgba: format!("rgba({}, {}, {}, 1)", c.r, c.g, c.b),
            hsl: format!("hsl({h:.0}, {s:.0}%, {l:.0}%)"),
            red: c.r,
            green: c.g,
            blue: c.b,
        }
    }
}

/// HSL with hue in degrees and saturation/lightness in percent.
fn hsl(c: Color) -> (f64, f64, f64) {
    let [r, g, b] = [c.r, c.g, c.b].map(|v| f64::from(v) / 255.0);
    let max = r.max(g).max(b);
    let min = r.min(g).min(b);
    let l = f64::midpoint(max, min);
    let d = max - min;
    if d == 0.0 {
        return (0.0, 0.0, l * 100.0);
    }
    let s = d / (1.0 - 2.0f64.mul_add(l, -1.0).abs());
    let h = if max == r {
        ((g - b) / d).rem_euclid(6.0)
    } else if max == g {
        (b - r) / d + 2.0
    } else {
        (r - g) / d + 4.0
    };
    (h * 60.0, s * 100.0, l * 100.0)
}

#[derive(Debug, Clone, Serialize)]
struct PerMode {
    light: Formats,
    dark: Formats,
    default: Formats,
}

impl PerMode {
    fn new(light: Color, dark: Color, mode: Mode) -> Self {
        Self {
            light: light.into(),
            dark: dark.into(),
            default: match mode {
                Mode::Light => light,
                Mode::Dark => dark,
            }
            .into(),
        }
    }
}

/// The variables available to every template.
#[derive(Debug, Clone, Serialize)]
pub struct Context {
    colors: BTreeMap<String, PerMode>,
    term: BTreeMap<String, PerMode>,
    source_color: Formats,
    mode: Mode,
    is_dark: bool,
    wallpaper: String,
    /// Paths and per-target options (`home`, `cache`, `vault`, ...).
    #[serde(flatten)]
    vars: BTreeMap<String, String>,
}

impl Context {
    pub fn new(palette: &Palette, mode: Mode, wallpaper: Option<&std::path::Path>) -> Self {
        let mut colors: BTreeMap<String, PerMode> = palette
            .light
            .iter()
            .map(|(name, &light)| (name.clone(), PerMode::new(light, palette.dark[name], mode)))
            .collect();
        // matugen exposes the source color as a role too.
        colors.insert(
            "source_color".to_owned(),
            PerMode::new(palette.source, palette.source, mode),
        );
        let term = (0..16)
            .map(|i| {
                let entry = PerMode::new(palette.terminal_light[i], palette.terminal_dark[i], mode);
                (format!("color{i}"), entry)
            })
            .collect();
        Self {
            colors,
            term,
            source_color: palette.source.into(),
            mode,
            is_dark: mode.is_dark(),
            wallpaper: wallpaper
                .map(|p| p.display().to_string())
                .unwrap_or_default(),
            vars: BTreeMap::new(),
        }
    }

    /// Adds plain string variables such as paths and target options.
    #[must_use]
    pub fn with_vars<'a>(mut self, vars: impl IntoIterator<Item = (&'a str, &'a str)>) -> Self {
        self.vars
            .extend(vars.into_iter().map(|(k, v)| (k.to_owned(), v.to_owned())));
        self
    }
}

/// Renders templates. Each render gets a fresh engine so errors can name the
/// template they come from; engines are cheap to make.
#[derive(Debug, Default)]
pub struct Renderer;

impl Renderer {
    pub const fn new() -> Self {
        Self
    }

    /// `name` labels the template in error messages, e.g. its file path.
    pub fn render(&self, name: &str, source: &str, ctx: &Context) -> Result<String, TemplateError> {
        let mut engine = upon::Engine::new();
        engine.add_function("upper", |s: &str| s.to_uppercase());
        engine.add_function("lower", |s: &str| s.to_lowercase());
        engine.add_function("replace", |s: &str, from: &str, to: &str| {
            s.replace(from, to)
        });
        engine.add_template(name.to_owned(), source.to_owned())?;
        Ok(engine.template(name).render(ctx).to_string()?)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::SchemeVariant;

    fn ctx(mode: Mode) -> Context {
        let palette =
            Palette::from_source(Color::new(0xff, 0x40, 0x57), SchemeVariant::TonalSpot, 0.0);
        Context::new(&palette, mode, Some("/tmp/wall.jpg".as_ref()))
    }

    #[test]
    fn renders_matugen_style_paths() {
        let r = Renderer::new();
        let p = Palette::from_source(Color::new(0xff, 0x40, 0x57), SchemeVariant::TonalSpot, 0.0);
        let out = r
            .render(
                "test",
                "{{colors.primary.default.hex}} {{colors.primary.light.hex_stripped}} {{mode}} {{wallpaper}}",
                &ctx(Mode::Dark),
            )
            .unwrap();
        let expected = format!(
            "{} {} dark /tmp/wall.jpg",
            p.role(Mode::Dark, "primary"),
            p.role(Mode::Light, "primary").hex_stripped()
        );
        assert_eq!(out, expected);
    }

    #[test]
    fn default_follows_mode() {
        let r = Renderer::new();
        let src = "{{colors.surface.default.hex}}={{colors.surface.light.hex}}";
        let light = r.render("test", src, &ctx(Mode::Light)).unwrap();
        let (a, b) = light.split_once('=').unwrap();
        assert_eq!(a, b);
        let dark = r.render("test", src, &ctx(Mode::Dark)).unwrap();
        let (a, b) = dark.split_once('=').unwrap();
        assert_ne!(a, b);
    }

    #[test]
    fn formats() {
        let f = Formats::from(Color::new(255, 0, 0));
        assert_eq!(f.rgb, "rgb(255, 0, 0)");
        assert_eq!(f.rgba, "rgba(255, 0, 0, 1)");
        assert_eq!(f.hsl, "hsl(0, 100%, 50%)");
        assert_eq!(
            Formats::from(Color::new(0, 0, 255)).hsl,
            "hsl(240, 100%, 50%)"
        );
        assert_eq!(
            Formats::from(Color::new(128, 128, 128)).hsl,
            "hsl(0, 0%, 50%)"
        );
    }

    #[test]
    fn terminal_colors_and_vars() {
        let r = Renderer::new();
        let c = ctx(Mode::Dark).with_vars([("vault", "/notes")]);
        let out = r
            .render("test", "{{term.color1.default.hex}} {{vault}}", &c)
            .unwrap();
        assert!(out.starts_with('#') && out.ends_with(" /notes"), "{out}");
    }

    #[test]
    fn functions() {
        let r = Renderer::new();
        let out = r
            .render(
                "test",
                "{{ colors.primary.default.hex | upper | replace: \"#\", \"0xff\" }}",
                &ctx(Mode::Dark),
            )
            .unwrap();
        assert!(
            out.starts_with("0xff") && out[4..].chars().all(|c| !c.is_ascii_lowercase()),
            "{out}"
        );
    }

    #[test]
    fn unknown_variable_is_an_error_with_location() {
        let err = Renderer::new()
            .render("test", "x {{colors.primray.default.hex}}", &ctx(Mode::Dark))
            .unwrap_err();
        assert!(err.to_string().contains("primray"), "{err}");
        assert!(
            err.to_string().contains("test"),
            "names the template: {err}"
        );
    }
}
