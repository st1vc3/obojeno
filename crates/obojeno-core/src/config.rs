//! `~/.config/obojeno/config.toml`.

use std::borrow::Cow;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::palette::SchemeVariant;
use crate::targets::{self, Output, OutputKind, Reload, Target};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Config {
    pub scheme: SchemeVariant,
    /// -1.0 (reduced) ..= 1.0 (high), 0.0 is the spec.
    pub contrast: f64,
    /// Where images imported in the gallery are copied to. Always shown first.
    pub library: String,
    /// More folders shown in the gallery, by reference.
    pub wallpaper_dirs: Vec<String>,
    pub transition: Transition,
    /// Keep the same wallpaper on every Space. macOS only lets apps change
    /// the Space in view, so the others follow as they are visited.
    pub all_spaces: bool,
    /// Tip the wallpaper back, blur and darken it while the lid closes.
    pub lid_animation: bool,
    pub finish: Finish,
    pub targets: BTreeMap<String, TargetConfig>,
}

/// Touches drawn over the wallpaper.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Finish {
    pub grain: Grain,
    /// Rounds the wallpaper's corners by this many points: the bottom ones
    /// on a MacBook's built-in screen (to match its rounded top), all four
    /// elsewhere. 0 turns it off.
    pub corner_radius: f64,
}

impl Default for Finish {
    fn default() -> Self {
        Self {
            grain: Grain::Off,
            corner_radius: 0.0,
        }
    }
}

/// Film grain over the wallpaper; softens banding in smooth gradients.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Grain {
    #[default]
    Off,
    Subtle,
    Strong,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            scheme: SchemeVariant::default(),
            contrast: 0.0,
            library: "~/Pictures/obojeno".to_owned(),
            wallpaper_dirs: Vec::new(),
            transition: Transition::default(),
            all_spaces: false,
            lid_animation: false,
            finish: Finish::default(),
            targets: BTreeMap::new(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum TransitionStyle {
    #[default]
    Fade,
    GrowCircle,
    Wipe,
    None,
}

impl std::str::FromStr for TransitionStyle {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "fade" => Ok(Self::Fade),
            "grow-circle" => Ok(Self::GrowCircle),
            "wipe" => Ok(Self::Wipe),
            "none" => Ok(Self::None),
            _ => Err(format!(
                "unknown transition {s:?}, expected fade, grow-circle, wipe or none"
            )),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Transition {
    pub style: TransitionStyle,
    /// Seconds.
    pub duration: f64,
}

impl Default for Transition {
    fn default() -> Self {
        Self {
            style: TransitionStyle::default(),
            duration: 0.6,
        }
    }
}

/// A `[targets.<name>]` table. For built-in targets every field is optional
/// and overrides the default; custom targets need `template` and `output`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct TargetConfig {
    pub enabled: Option<bool>,
    /// Path to a template file replacing the built-in one.
    pub template: Option<String>,
    pub output: Option<String>,
    /// Replaces the reload commands; `[]` disables reloading.
    pub reload: Option<Vec<Vec<String>>>,
    /// Only reload while a process with this executable name runs.
    pub only_if_running: Option<String>,
    /// Target specific options, e.g. `vault` for obsidian.
    #[serde(flatten)]
    pub options: BTreeMap<String, String>,
}

#[derive(Debug, thiserror::Error)]
pub enum ConfigError {
    #[error("reading {path}: {error}")]
    Read {
        path: PathBuf,
        error: std::io::Error,
    },
    #[error("{path}: {error}")]
    Parse {
        path: PathBuf,
        error: Box<toml::de::Error>,
    },
    #[error("target {target}: {message}")]
    Target { target: String, message: String },
}

/// A target ready to render: built-in defaults merged with the user's config.
#[derive(Debug, Clone, PartialEq)]
pub struct ResolvedTarget {
    pub target: Target,
    /// Option values, defaults filled in.
    pub options: BTreeMap<String, String>,
}

impl Config {
    /// Loads the config, or the defaults when the file does not exist.
    pub fn load(path: &Path) -> Result<Self, ConfigError> {
        match std::fs::read_to_string(path) {
            Ok(text) => Self::parse(&text).map_err(|error| ConfigError::Parse {
                path: path.to_owned(),
                error: Box::new(error),
            }),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Self::default()),
            Err(error) => Err(ConfigError::Read {
                path: path.to_owned(),
                error,
            }),
        }
    }

    pub fn parse(text: &str) -> Result<Self, toml::de::Error> {
        toml::from_str(text)
    }

    /// Enabled targets, in config order. `read_template` loads user template
    /// files (paths already `~`-expanded by the caller's choice of reader).
    pub fn resolve_targets(
        &self,
        read_template: impl Fn(&str) -> std::io::Result<String>,
    ) -> Result<Vec<ResolvedTarget>, ConfigError> {
        let mut resolved = Vec::new();
        for (name, cfg) in &self.targets {
            if cfg.enabled == Some(false) {
                continue;
            }
            let err = |message: String| ConfigError::Target {
                target: name.clone(),
                message,
            };
            let user_template = cfg
                .template
                .as_deref()
                .map(|p| read_template(p).map_err(|e| err(format!("reading template {p}: {e}"))))
                .transpose()?;

            let mut target = match targets::builtin(name) {
                Some(mut t) => {
                    if let Some(source) = user_template {
                        let slot = t
                            .primary_template_mut()
                            .ok_or_else(|| err("has no template to replace".to_owned()))?;
                        *slot = Cow::Owned(source);
                    }
                    if let Some(output) = &cfg.output {
                        let first = t.outputs.first_mut().ok_or_else(|| {
                            err("writes no file, `output` does not apply".to_owned())
                        })?;
                        first.path.clone_from(output);
                    }
                    t
                }
                None => {
                    let (Some(template), Some(output)) = (user_template, &cfg.output) else {
                        return Err(err(format!(
                            "not a built-in target ({}), so it needs `template` and `output`",
                            targets::BUILTIN_NAMES.join(", ")
                        )));
                    };
                    Target {
                        outputs: vec![Output {
                            path: output.clone(),
                            kind: OutputKind::Text {
                                template: Cow::Owned(template),
                            },
                        }],
                        ..Target::custom(name)
                    }
                }
            };
            if let Some(reload) = &cfg.reload {
                target.reload = reload.iter().cloned().map(Reload::Command).collect();
            }
            if let Some(process) = &cfg.only_if_running {
                target.only_if_running = Some(process.clone());
            }

            let mut options = BTreeMap::new();
            for key in cfg.options.keys() {
                if !target.options.iter().any(|(k, _)| k == key) {
                    let known: Vec<_> = target.options.iter().map(|(k, _)| *k).collect();
                    return Err(err(format!(
                        "unknown option `{key}` (known: {})",
                        if known.is_empty() {
                            "none".to_owned()
                        } else {
                            known.join(", ")
                        }
                    )));
                }
            }
            for (key, default) in &target.options {
                let value = cfg
                    .options
                    .get(*key)
                    .map(String::as_str)
                    .or(*default)
                    .ok_or_else(|| err(format!("missing required option `{key}`")))?;
                options.insert((*key).to_owned(), value.to_owned());
            }
            resolved.push(ResolvedTarget { target, options });
        }
        Ok(resolved)
    }
}

/// Settings the menu bar app changes. `None` leaves a setting alone.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct ConfigChange {
    pub scheme: Option<SchemeVariant>,
    pub contrast: Option<f64>,
    pub transition: Option<TransitionStyle>,
    pub duration: Option<f64>,
    pub wallpaper_dirs: Option<Vec<String>>,
    pub all_spaces: Option<bool>,
    pub lid_animation: Option<bool>,
    pub grain: Option<Grain>,
    pub corner_radius: Option<f64>,
}

#[derive(Debug, thiserror::Error)]
pub enum EditError {
    #[error("the config is not valid TOML: {0}")]
    Parse(#[from] toml_edit::TomlError),
    #[error("{0}")]
    Invalid(String),
}

/// Applies `change` to the text of a config file, keeping its comments and
/// formatting. The result is validated by parsing it.
pub fn edit(text: &str, change: &ConfigChange) -> Result<String, EditError> {
    use toml_edit::{Array, DocumentMut, Item, Table};

    let mut doc: DocumentMut = text.parse()?;
    if let Some(scheme) = change.scheme {
        set(&mut doc["scheme"], scheme.name().into());
    }
    if let Some(contrast) = change.contrast {
        if !(-1.0..=1.0).contains(&contrast) {
            return Err(EditError::Invalid(format!(
                "contrast {contrast} is outside -1..=1"
            )));
        }
        set(&mut doc["contrast"], contrast.into());
    }
    if let Some(dirs) = &change.wallpaper_dirs {
        set(
            &mut doc["wallpaper_dirs"],
            dirs.iter().map(String::as_str).collect::<Array>().into(),
        );
    }
    if let Some(all) = change.all_spaces {
        set(&mut doc["all_spaces"], all.into());
    }
    if let Some(lid) = change.lid_animation {
        set(&mut doc["lid_animation"], lid.into());
    }
    if change.grain.is_some() || change.corner_radius.is_some() {
        if !doc.contains_table("finish") {
            doc["finish"] = Item::Table(Table::new());
        }
        if let Some(grain) = change.grain {
            let name = match grain {
                Grain::Off => "off",
                Grain::Subtle => "subtle",
                Grain::Strong => "strong",
            };
            set(&mut doc["finish"]["grain"], name.into());
        }
        if let Some(radius) = change.corner_radius {
            if !(0.0..=40.0).contains(&radius) {
                return Err(EditError::Invalid(format!(
                    "corner radius {radius} is outside 0..=40"
                )));
            }
            set(&mut doc["finish"]["corner_radius"], radius.into());
        }
    }
    if change.transition.is_some() || change.duration.is_some() {
        if !doc.contains_table("transition") {
            doc["transition"] = Item::Table(Table::new());
        }
        if let Some(style) = change.transition {
            let name = serde_json::to_value(style)
                .ok()
                .and_then(|v| v.as_str().map(str::to_owned))
                .unwrap_or_default();
            set(&mut doc["transition"]["style"], name.into());
        }
        if let Some(duration) = change.duration {
            if !(0.0..=10.0).contains(&duration) {
                return Err(EditError::Invalid(format!(
                    "duration {duration}s is outside 0..=10"
                )));
            }
            set(&mut doc["transition"]["duration"], duration.into());
        }
    }
    let edited = doc.to_string();
    Config::parse(&edited).map_err(|e| EditError::Invalid(e.to_string()))?;
    Ok(edited)
}

/// Replaces a value, keeping the comments and spacing around it.
fn set(item: &mut toml_edit::Item, mut new: toml_edit::Value) {
    if let Some(old) = item.as_value() {
        *new.decor_mut() = old.decor().clone();
    }
    *item = toml_edit::Item::Value(new);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn no_files(path: &str) -> std::io::Result<String> {
        Err(std::io::Error::new(
            std::io::ErrorKind::NotFound,
            path.to_owned(),
        ))
    }

    #[test]
    fn edits_keep_comments_and_other_settings() {
        let text = "# my setup\nscheme = \"vibrant\" # bold\n\n[targets.kitty]\n";
        let edited = edit(
            text,
            &ConfigChange {
                scheme: Some(SchemeVariant::Content),
                transition: Some(TransitionStyle::GrowCircle),
                duration: Some(1.2),
                wallpaper_dirs: Some(vec!["~/Walls".into()]),
                ..ConfigChange::default()
            },
        )
        .unwrap();
        assert!(
            edited.contains("# my setup") && edited.contains("# bold"),
            "{edited}"
        );
        let config = Config::parse(&edited).unwrap();
        assert_eq!(config.scheme, SchemeVariant::Content);
        assert_eq!(config.transition.style, TransitionStyle::GrowCircle);
        assert!((config.transition.duration - 1.2).abs() < f64::EPSILON);
        assert_eq!(config.wallpaper_dirs, ["~/Walls"]);
        assert!(config.targets.contains_key("kitty"));
    }

    #[test]
    fn edits_finish_and_toggles() {
        let edited = edit(
            "",
            &ConfigChange {
                all_spaces: Some(true),
                lid_animation: Some(true),
                grain: Some(Grain::Subtle),
                corner_radius: Some(18.0),
                ..ConfigChange::default()
            },
        )
        .unwrap();
        let config = Config::parse(&edited).unwrap();
        assert!(config.all_spaces && config.lid_animation);
        assert_eq!(config.finish.grain, Grain::Subtle);
        assert!((config.finish.corner_radius - 18.0).abs() < f64::EPSILON);
        assert!(
            edit(
                "",
                &ConfigChange {
                    corner_radius: Some(99.0),
                    ..ConfigChange::default()
                }
            )
            .is_err()
        );
    }

    #[test]
    fn edits_start_from_an_empty_file() {
        let edited = edit(
            "",
            &ConfigChange {
                duration: Some(0.3),
                ..ConfigChange::default()
            },
        )
        .unwrap();
        assert!((Config::parse(&edited).unwrap().transition.duration - 0.3).abs() < f64::EPSILON);
        assert_eq!(edit("", &ConfigChange::default()).unwrap(), "");
    }

    #[test]
    fn edits_reject_bad_values_and_broken_files() {
        assert!(
            edit(
                "",
                &ConfigChange {
                    contrast: Some(2.0),
                    ..ConfigChange::default()
                }
            )
            .is_err()
        );
        assert!(
            edit(
                "",
                &ConfigChange {
                    duration: Some(-1.0),
                    ..ConfigChange::default()
                }
            )
            .is_err()
        );
        assert!(edit("[[", &ConfigChange::default()).is_err());
    }

    #[test]
    fn empty_config_is_default() {
        assert_eq!(Config::parse("").unwrap(), Config::default());
    }

    #[test]
    fn parses_full_config() {
        let c = Config::parse(
            r#"
            scheme = "vibrant"
            contrast = 0.5
            wallpaper_dirs = ["~/Walls"]
            [transition]
            style = "grow-circle"
            duration = 1.2
            [targets.kitty]
            [targets.obsidian]
            vault = "~/Notes"
            "#,
        )
        .unwrap();
        assert_eq!(c.scheme, SchemeVariant::Vibrant);
        assert_eq!(c.transition.style, TransitionStyle::GrowCircle);
        assert_eq!(c.targets["obsidian"].options["vault"], "~/Notes");
    }

    #[test]
    fn rejects_unknown_top_level_keys() {
        assert!(Config::parse("shceme = \"vibrant\"").is_err());
        assert!(Config::parse("scheme = \"neon\"").is_err());
    }

    #[test]
    fn resolves_builtins_with_defaults() {
        let c = Config::parse("[targets.kitty]\n[targets.starship]").unwrap();
        let targets = c.resolve_targets(no_files).unwrap();
        assert_eq!(targets.len(), 2);
        let starship = targets
            .iter()
            .find(|t| t.target.name == "starship")
            .unwrap();
        assert_eq!(starship.options["base"], "~/.config/starship.toml");
    }

    #[test]
    fn disabled_targets_are_skipped() {
        let c = Config::parse("[targets.kitty]\nenabled = false").unwrap();
        assert!(c.resolve_targets(no_files).unwrap().is_empty());
    }

    #[test]
    fn missing_required_option_is_an_error() {
        let c = Config::parse("[targets.obsidian]").unwrap();
        let err = c.resolve_targets(no_files).unwrap_err().to_string();
        assert!(err.contains("vault"), "{err}");
    }

    #[test]
    fn unknown_option_is_an_error() {
        let c = Config::parse("[targets.kitty]\nvalut = \"x\"").unwrap();
        let err = c.resolve_targets(no_files).unwrap_err().to_string();
        assert!(err.contains("valut"), "{err}");
    }

    #[test]
    fn custom_target_needs_template_and_output() {
        let c = Config::parse("[targets.mine]\noutput = \"/tmp/x\"").unwrap();
        assert!(c.resolve_targets(no_files).is_err());

        let c = Config::parse(
            "[targets.mine]\ntemplate = \"t\"\noutput = \"/tmp/x\"\nreload = [[\"true\"]]",
        )
        .unwrap();
        let t = c.resolve_targets(|_| Ok("{{mode}}".to_owned())).unwrap();
        assert_eq!(t[0].target.outputs[0].path, "/tmp/x");
        assert_eq!(
            t[0].target.reload,
            vec![Reload::Command(vec!["true".to_owned()])]
        );
    }

    #[test]
    fn template_override_replaces_builtin_template() {
        let c = Config::parse("[targets.kitty]\ntemplate = \"mine\"").unwrap();
        let mut t = c.resolve_targets(|_| Ok("custom".to_owned())).unwrap();
        assert_eq!(
            t[0].target.primary_template_mut().unwrap().as_ref(),
            "custom"
        );
    }
}
