//! `~/.config/obojeno/config.toml`.

use std::borrow::Cow;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::palette::SchemeVariant;
use crate::targets::{self, Output, OutputKind, Target};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Config {
    pub scheme: SchemeVariant,
    /// -1.0 (reduced) ..= 1.0 (high), 0.0 is the spec.
    pub contrast: f64,
    /// Folders shown in the gallery.
    pub wallpaper_dirs: Vec<String>,
    pub transition: Transition,
    pub targets: BTreeMap<String, TargetConfig>,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            scheme: SchemeVariant::default(),
            contrast: 0.0,
            wallpaper_dirs: vec!["~/Pictures/Wallpapers".to_owned()],
            transition: Transition::default(),
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
    /// Target specific options, e.g. `vault` for obsidian.
    #[serde(flatten)]
    pub options: BTreeMap<String, String>,
}

#[derive(Debug, thiserror::Error)]
pub enum ConfigError {
    #[error("reading {path}: {source}")]
    Read {
        path: PathBuf,
        source: std::io::Error,
    },
    #[error("{path}: {source}")]
    Parse {
        path: PathBuf,
        source: Box<toml::de::Error>,
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
            Ok(text) => Self::parse(&text).map_err(|source| ConfigError::Parse {
                path: path.to_owned(),
                source: Box::new(source),
            }),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Self::default()),
            Err(source) => Err(ConfigError::Read {
                path: path.to_owned(),
                source,
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
                        name: name.clone(),
                        outputs: vec![Output {
                            path: output.clone(),
                            kind: OutputKind::Text {
                                template: Cow::Owned(template),
                            },
                        }],
                        reload: vec![],
                        options: vec![],
                        setup: "",
                    }
                }
            };
            if let Some(reload) = &cfg.reload {
                target.reload.clone_from(reload);
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
        assert_eq!(t[0].target.reload, vec![vec!["true".to_owned()]]);
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
