//! Built-in theming targets.
//!
//! A target is a list of outputs plus commands that make the app pick them up.
//! Output paths and reload commands are templates themselves, so they can use
//! colors (`borders active_color=0xff{{...}}`) and target options (`{{vault}}`).

use std::borrow::Cow;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MergeFormat {
    Json,
    Toml,
}

#[derive(Debug, Clone, PartialEq)]
pub enum OutputKind {
    /// The rendered template is the file.
    Text { template: Cow<'static, str> },
    /// The rendered template is deep-merged into `base` (a read-only file the
    /// user owns, such as their tracked config) and the result is written to
    /// the output. Without a base, it merges into the existing output file.
    Merge {
        format: MergeFormat,
        template: Cow<'static, str>,
        base: Option<String>,
    },
    /// A downscaled, blurred JPEG of the wallpaper, for bars that fake
    /// translucency. Skipped when there is no wallpaper image.
    BlurredWallpaper { width: u32, sigma: f32, quality: u8 },
}

#[derive(Debug, Clone, PartialEq)]
pub struct Output {
    /// Templated path; `~/` is expanded.
    pub path: String,
    pub kind: OutputKind,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Target {
    pub name: String,
    pub outputs: Vec<Output>,
    /// Commands run after the outputs changed. Each argument is a template.
    pub reload: Vec<Vec<String>>,
    /// Options this target accepts, with defaults (`None` = required).
    pub options: Vec<(&'static str, Option<&'static str>)>,
    /// What the user has to do once so the app reads obojeno's output.
    pub setup: &'static str,
}

impl Target {
    /// The template users override with `template = ...` in the config.
    pub fn primary_template_mut(&mut self) -> Option<&mut Cow<'static, str>> {
        self.outputs.iter_mut().find_map(|o| match &mut o.kind {
            OutputKind::Text { template } | OutputKind::Merge { template, .. } => Some(template),
            OutputKind::BlurredWallpaper { .. } => None,
        })
    }
}

pub const BUILTIN_NAMES: [&str; 8] = [
    "kitty",
    "borders",
    "simple-bar",
    "nvim",
    "starship",
    "herdr",
    "obsidian",
    "zen",
];

fn cmd(args: &[&str]) -> Vec<String> {
    args.iter().map(|&a| a.to_owned()).collect()
}

fn text(path: &str, template: &'static str) -> Output {
    Output {
        path: path.to_owned(),
        kind: OutputKind::Text {
            template: Cow::Borrowed(template),
        },
    }
}

pub fn builtin(name: &str) -> Option<Target> {
    let target = |outputs, reload, options, setup| Target {
        name: name.to_owned(),
        outputs,
        reload,
        options,
        setup,
    };
    Some(match name {
        "kitty" => target(
            vec![text(
                "{{cache}}/kitty.conf",
                include_str!("../templates/kitty.conf"),
            )],
            // kitty reloads its config, including includes, on SIGUSR1.
            vec![cmd(&["pkill", "-USR1", "-x", "kitty"])],
            vec![],
            "Add `include ~/.cache/obojeno/kitty.conf` at the end of kitty.conf.",
        ),
        "borders" => target(
            vec![],
            // Called while borders is running, this updates the live instance.
            vec![cmd(&[
                "borders",
                "active_color=0xff{{colors.primary.default.hex_stripped}}",
            ])],
            vec![],
            "Nothing to do: obojeno updates the running borders instance.",
        ),
        "simple-bar" => target(
            vec![
                Output {
                    path: "~/.simplebarrc".to_owned(),
                    kind: OutputKind::Merge {
                        format: MergeFormat::Json,
                        template: Cow::Borrowed(include_str!("../templates/simple-bar.json")),
                        base: None,
                    },
                },
                Output {
                    path: "{{cache}}/wallpaper-blur.jpg".to_owned(),
                    kind: OutputKind::BlurredWallpaper {
                        width: 900,
                        sigma: 18.0,
                        quality: 82,
                    },
                },
            ],
            vec![cmd(&[
                "osascript",
                "-e",
                "tell application id \"tracesOf.Uebersicht\" to refresh widget id \"simple-bar-index-jsx\"",
            ])],
            vec![],
            "Nothing to do: colors are merged into ~/.simplebarrc. Set `base` to merge from a read-only file instead.",
        ),
        "nvim" => target(
            vec![text(
                "{{cache}}/nvim/colors/obojeno.lua",
                include_str!("../templates/nvim.lua"),
            )],
            vec![],
            vec![],
            "See `obojeno targets nvim` for the snippet that loads and live-reloads the colorscheme.",
        ),
        "starship" => target(
            vec![Output {
                path: "{{cache}}/starship.toml".to_owned(),
                kind: OutputKind::Merge {
                    format: MergeFormat::Toml,
                    template: Cow::Borrowed(include_str!("../templates/starship.toml")),
                    base: Some("{{base}}".to_owned()),
                },
            }],
            vec![],
            vec![("base", Some("~/.config/starship.toml"))],
            "Set STARSHIP_CONFIG=~/.cache/obojeno/starship.toml and use palette colors (`fg:primary`) in your config.",
        ),
        "herdr" => target(
            vec![Output {
                path: "{{cache}}/herdr.toml".to_owned(),
                kind: OutputKind::Merge {
                    format: MergeFormat::Toml,
                    template: Cow::Borrowed(include_str!("../templates/herdr.toml")),
                    base: Some("{{base}}".to_owned()),
                },
            }],
            vec![cmd(&["herdr", "server", "reload-config"])],
            vec![("base", Some("~/.config/herdr/config.toml"))],
            "Set HERDR_CONFIG_PATH=~/.cache/obojeno/herdr.toml for herdr.",
        ),
        "obsidian" => target(
            vec![text(
                "{{vault}}/.obsidian/snippets/obojeno.css",
                include_str!("../templates/obsidian.css"),
            )],
            vec![],
            vec![("vault", None)],
            "Enable the `obojeno` snippet under Settings > Appearance > CSS snippets.",
        ),
        "zen" => target(
            vec![text(
                "{{profile}}/chrome/userChrome.css",
                include_str!("../templates/zen.css"),
            )],
            vec![],
            vec![("profile", None)],
            "Set toolkit.legacyUserProfileCustomizations.stylesheets = true in about:config. Zen applies changes on its next start.",
        ),
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_builtin_name_resolves() {
        for name in BUILTIN_NAMES {
            let t = builtin(name).unwrap_or_else(|| panic!("{name}"));
            assert_eq!(t.name, name);
            assert!(
                !t.outputs.is_empty() || !t.reload.is_empty(),
                "{name} does nothing"
            );
        }
        assert!(builtin("nope").is_none());
    }
}
