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

/// How an app is told to pick up new colors.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Reload {
    /// Run a command. Each argument is a template.
    Command(Vec<String>),
    /// Send a signal to every process with this executable name.
    Signal { process: String, signal: Signal },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Signal {
    Usr1,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Target {
    pub name: String,
    pub outputs: Vec<Output>,
    /// Run after the outputs are written.
    pub reload: Vec<Reload>,
    /// Skip reloading unless a process with this executable name runs.
    /// Some reload commands would otherwise start the app.
    pub only_if_running: Option<String>,
    /// Options this target accepts, with defaults (`None` = required).
    pub options: Vec<(&'static str, Option<&'static str>)>,
    /// What the user has to do once so the app reads obojeno's output.
    pub setup: &'static str,
}

impl Target {
    /// A target that does nothing yet; built-ins and custom targets fill it.
    pub fn custom(name: &str) -> Self {
        Self {
            name: name.to_owned(),
            outputs: vec![],
            reload: vec![],
            only_if_running: None,
            options: vec![],
            setup: "",
        }
    }

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

fn cmd(args: &[&str]) -> Reload {
    Reload::Command(args.iter().map(|&a| a.to_owned()).collect())
}

fn text(path: &str, template: &'static str) -> Output {
    Output {
        path: path.to_owned(),
        kind: OutputKind::Text {
            template: Cow::Borrowed(template),
        },
    }
}

fn toml_merge(path: &str, template: &'static str) -> Output {
    Output {
        path: path.to_owned(),
        kind: OutputKind::Merge {
            format: MergeFormat::Toml,
            template: Cow::Borrowed(template),
            base: Some("{{base}}".to_owned()),
        },
    }
}

pub fn builtin(name: &str) -> Option<Target> {
    let base = Target::custom(name);
    Some(match name {
        "kitty" => Target {
            outputs: vec![text(
                "{{cache}}/kitty.conf",
                include_str!("../templates/kitty.conf"),
            )],
            // kitty reloads its config, including includes, on SIGUSR1.
            reload: vec![Reload::Signal {
                process: "kitty".to_owned(),
                signal: Signal::Usr1,
            }],
            setup: "Add `include ~/.cache/obojeno/kitty.conf` at the end of kitty.conf.",
            ..base
        },
        "borders" => Target {
            // Called while borders runs, this updates the live instance; when
            // it does not run, it would start a new one.
            // The vivid source color, not `primary`: Material's dark-mode
            // primary is a pastel, and a border only needs to stand out.
            reload: vec![cmd(&[
                "borders",
                "active_color=0xff{{source_color.hex_stripped}}",
            ])],
            only_if_running: Some("borders".to_owned()),
            setup: "Nothing to do: obojeno updates the running borders instance.",
            ..base
        },
        "simple-bar" => Target {
            outputs: vec![
                Output {
                    path: "~/.simplebarrc".to_owned(),
                    kind: OutputKind::Merge {
                        format: MergeFormat::Json,
                        template: Cow::Borrowed(include_str!("../templates/simple-bar.json")),
                        base: Some("{{base}}".to_owned()),
                    },
                },
                Output {
                    path: "{{blur}}".to_owned(),
                    kind: OutputKind::BlurredWallpaper {
                        width: 900,
                        sigma: 18.0,
                        quality: 82,
                    },
                },
            ],
            // `refresh` only re-runs the widget; `reload` re-reads ~/.simplebarrc.
            // simple-bar renders from its localStorage copy before that read
            // finishes, so the new colors show from the second reload on.
            // AppleScript would launch Übersicht if it is not running.
            reload: vec![cmd(&[
                "osascript",
                "-e",
                "tell application id \"tracesOf.Uebersicht\" to reload widget id \"simple-bar-index-jsx\"",
                "-e",
                "delay 1",
                "-e",
                "tell application id \"tracesOf.Uebersicht\" to reload widget id \"simple-bar-index-jsx\"",
            ])],
            only_if_running: Some("Übersicht".to_owned()),
            // By default the colors merge into the file itself. Point `base`
            // at a tracked copy to keep your settings there instead.
            // `blur` is for custom styles with a blurred wallpaper behind the
            // bar, e.g. "~/Library/Application Support/Übersicht/widgets/
            // simple-bar/wallpaper-blur.jpg" for `url("simple-bar/wallpaper-blur.jpg")`.
            options: vec![
                ("base", Some("~/.simplebarrc")),
                ("blur", Some("{{cache}}/wallpaper-blur.jpg")),
            ],
            setup: "Nothing to do: colors are merged into ~/.simplebarrc (or into `base`, written to ~/.simplebarrc). \
                    Custom styles can show a blurred wallpaper behind the bar from `blur`.",
            ..base
        },
        "nvim" => Target {
            outputs: vec![text(
                "{{cache}}/nvim/colors/obojeno.lua",
                include_str!("../templates/nvim.lua"),
            )],
            setup: "See `obojeno targets nvim` for the snippet that loads and live-reloads the colorscheme.",
            ..base
        },
        "starship" => Target {
            outputs: vec![toml_merge(
                "{{cache}}/starship.toml",
                include_str!("../templates/starship.toml"),
            )],
            options: vec![("base", Some("~/.config/starship.toml"))],
            setup: "Set STARSHIP_CONFIG=~/.cache/obojeno/starship.toml and use palette colors (`fg:primary`) in your config.",
            ..base
        },
        "herdr" => Target {
            outputs: vec![toml_merge(
                "{{cache}}/herdr.toml",
                include_str!("../templates/herdr.toml"),
            )],
            reload: vec![cmd(&["herdr", "server", "reload-config"])],
            only_if_running: Some("herdr".to_owned()),
            options: vec![("base", Some("~/.config/herdr/config.toml"))],
            setup: "Set HERDR_CONFIG_PATH=~/.cache/obojeno/herdr.toml for herdr.",
            ..base
        },
        "obsidian" => Target {
            outputs: vec![text(
                "{{vault}}/.obsidian/snippets/obojeno.css",
                include_str!("../templates/obsidian.css"),
            )],
            options: vec![("vault", None)],
            setup: "Enable the `obojeno` snippet under Settings > Appearance > CSS snippets.",
            ..base
        },
        "zen" => Target {
            outputs: vec![text(
                "{{profile}}/chrome/userChrome.css",
                include_str!("../templates/zen.css"),
            )],
            options: vec![("profile", None)],
            setup: "Set toolkit.legacyUserProfileCustomizations.stylesheets = true in about:config. Zen applies changes on its next start.",
            ..base
        },
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
