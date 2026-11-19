mod actions;
mod apply;
mod daemon;
mod gallery;
mod imageio;
mod macos;
mod paths;
mod reload;
mod state;
mod transition;
mod watch;

use std::path::PathBuf;
use std::process::ExitCode;

use anyhow::Context as _;
use clap::{Parser, Subcommand};
use obojeno_core::config::{Config, TransitionStyle};
use obojeno_core::ipc::{ModeChange, Outcome, Reply, Report, Request};
use obojeno_core::targets::{self, OutputKind};
use obojeno_core::{Color, Mode, Palette, SchemeVariant, palette};

use crate::paths::Paths;

/// Wallpaper-driven theming for macOS.
#[derive(Parser)]
#[command(version, about)]
struct Cli {
    /// Config file [default: ~/.config/obojeno/config.toml]
    #[arg(long, global = true, value_name = "FILE")]
    config: Option<PathBuf>,

    /// More output; repeat for even more
    #[arg(short, long, global = true, action = clap::ArgAction::Count)]
    verbose: u8,

    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Theme everything from the current wallpaper (or the fixed color)
    Apply {
        /// Theme for this mode instead of the system appearance
        #[arg(long)]
        mode: Option<Mode>,
    },
    /// Set the wallpaper on all screens and theme everything from it
    Set {
        image: PathBuf,
        /// fade, grow-circle, wipe or none [default: from the config]
        #[arg(long)]
        transition: Option<TransitionStyle>,
        /// Transition length in seconds [default: from the config]
        #[arg(long)]
        duration: Option<f64>,
    },
    /// Switch macOS to light or dark and theme everything for it
    Mode {
        #[arg(value_enum)]
        mode: ModeChoice,
    },
    /// Theme from a fixed color instead of the wallpaper
    Color {
        /// A hex color such as #ff4057
        #[arg(required_unless_present = "wallpaper")]
        color: Option<Color>,
        /// Go back to following the wallpaper
        #[arg(long, conflicts_with = "color")]
        wallpaper: bool,
    },
    /// List recent wallpapers and the images in the wallpaper folders
    Gallery,
    /// Run in the background: follow wallpaper and appearance changes, and
    /// serve the menu bar app
    Daemon,
    /// Show the palette of an image (default: the current wallpaper)
    Preview {
        image: Option<PathBuf>,
        /// Scheme variant [default: from the config]
        #[arg(long)]
        scheme: Option<SchemeVariant>,
    },
    /// List the built-in targets, or show how to set one up
    Targets { name: Option<String> },
}

#[derive(Clone, Copy, clap::ValueEnum)]
enum ModeChoice {
    Light,
    Dark,
    Toggle,
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    let level = match cli.verbose {
        0 => "warn",
        1 => "info",
        _ => "debug",
    };
    // In a terminal: colors, no timestamps. In a log file (the daemon under
    // launchd): plain text with timestamps.
    let terminal = std::io::IsTerminal::is_terminal(&std::io::stderr());
    let logger = tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_env("OBOJENO_LOG")
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new(level)),
        )
        .with_writer(std::io::stderr)
        .with_ansi(terminal)
        .with_target(false);
    if terminal {
        logger.without_time().init();
    } else {
        logger.init();
    }

    match run(cli) {
        Ok(code) => code,
        Err(e) => {
            eprintln!("obojeno: {e:#}");
            ExitCode::FAILURE
        }
    }
}

fn run(cli: Cli) -> anyhow::Result<ExitCode> {
    let paths = Paths::from_env(cli.config.as_deref())?;
    let request = match cli.command {
        Command::Apply { mode } => Request::Apply { mode },
        Command::Set {
            image,
            transition,
            duration,
        } => Request::SetWallpaper {
            // The daemon has its own working directory.
            path: std::path::absolute(&image)?,
            transition,
            duration,
            origin: None,
        },
        Command::Mode { mode } => Request::SetMode {
            mode: match mode {
                ModeChoice::Light => ModeChange::Light,
                ModeChoice::Dark => ModeChange::Dark,
                ModeChoice::Toggle => ModeChange::Toggle,
            },
        },
        Command::Color { color, .. } => Request::SetColor { color },
        Command::Daemon => {
            daemon::run(paths)?;
            return Ok(ExitCode::SUCCESS);
        }
        Command::Gallery => {
            let gallery = match daemon::request(&paths, &Request::Gallery)? {
                Some(Reply::Gallery(gallery)) => gallery,
                Some(Reply::Error { message }) => anyhow::bail!(message),
                Some(_) => anyhow::bail!("unexpected reply from the daemon"),
                None => gallery::build_blocking(
                    &paths,
                    macos::current_wallpaper().ok().as_deref(),
                    macos::system_mode(),
                )?,
            };
            print_gallery(&gallery);
            return Ok(ExitCode::SUCCESS);
        }
        Command::Preview { image, scheme } => {
            let config = Config::load(&paths.config_file)?;
            let image = match image {
                Some(image) => image,
                None => macos::current_wallpaper()?,
            };
            let decoded = imageio::decode(&image)
                .map_err(anyhow::Error::msg)
                .with_context(|| format!("reading {}", image.display()))?;
            let candidates = palette::source_candidates(&decoded);
            let palette = Palette::from_source(
                candidates[0],
                scheme.unwrap_or(config.scheme),
                config.contrast,
            );
            print_preview(&palette, &candidates);
            return Ok(ExitCode::SUCCESS);
        }
        Command::Targets { name } => {
            print_targets(name.as_deref(), &paths)?;
            return Ok(ExitCode::SUCCESS);
        }
    };
    // A running daemon does the work, so changes animate and never race it.
    let report = match daemon::request(&paths, &request)? {
        Some(Reply::Report(report)) => report,
        Some(Reply::Error { message }) => anyhow::bail!(message),
        Some(Reply::State(_) | Reply::Gallery(_)) => {
            anyhow::bail!("unexpected reply from the daemon")
        }
        None => actions::execute(&paths, &request)?.report,
    };
    Ok(print_report(&report, &paths))
}

fn swatch(c: Color) -> String {
    format!("\x1b[48;2;{};{};{}m    \x1b[0m", c.r, c.g, c.b)
}

fn print_report(report: &Report, paths: &Paths) -> ExitCode {
    let from = report
        .wallpaper
        .as_ref()
        .map_or_else(String::new, |w| format!(" from {}", w.display()));
    println!(
        "{} {} {}{from}",
        swatch(report.source),
        report.source,
        report.mode
    );
    if report.targets.is_empty() {
        println!(
            "No targets enabled yet. Add some to {} (see `obojeno targets`).",
            paths.config_file.display()
        );
        return ExitCode::SUCCESS;
    }
    let width = report
        .targets
        .iter()
        .map(|t| t.name.len())
        .max()
        .unwrap_or(0);
    let mut failed = false;
    for t in &report.targets {
        let status = if let Some(error) = &t.error {
            failed = true;
            format!("error: {error}")
        } else {
            let files = match t.written.len() {
                _ if !t.has_files => None,
                0 => Some("unchanged".to_owned()),
                1 => Some("1 file written".to_owned()),
                n => Some(format!("{n} files written")),
            };
            let reload = match &t.reload {
                None => None,
                Some(Outcome::Done) => Some("reloaded".to_owned()),
                Some(Outcome::NotRunning) => Some("not running".to_owned()),
                Some(Outcome::NotInstalled { command }) => Some(format!("{command} not installed")),
                Some(Outcome::Failed { message: e }) => {
                    failed = true;
                    Some(format!("reload failed: {e}"))
                }
            };
            [files, reload]
                .into_iter()
                .flatten()
                .collect::<Vec<_>>()
                .join(", ")
        };
        println!("  {:width$}  {status}", t.name);
    }
    if failed {
        ExitCode::FAILURE
    } else {
        ExitCode::SUCCESS
    }
}

fn print_gallery(gallery: &obojeno_core::gallery::Gallery) {
    let line = |item: &obojeno_core::gallery::Item| {
        let swatches: String = if item.swatches.is_empty() {
            "········".to_owned()
        } else {
            item.swatches
                .iter()
                .map(|&c| format!("\x1b[48;2;{};{};{}m  \x1b[0m", c.r, c.g, c.b))
                .collect()
        };
        let marker = if item.current { "▶" } else { " " };
        println!("  {marker} {swatches}  {}", item.path.display());
    };
    println!("Recent");
    if gallery.recent.is_empty() {
        println!("  (nothing yet)");
    }
    gallery.recent.iter().for_each(line);
    for folder in &gallery.folders {
        println!("\n{}", folder.path.display());
        match &folder.error {
            Some(e) => println!("  This folder {e}."),
            None if folder.items.is_empty() => println!("  (no images)"),
            None => folder.items.iter().for_each(line),
        }
    }
    if gallery.pending > 0 {
        println!("\n{} images are still being indexed.", gallery.pending);
    }
}

fn print_preview(palette: &Palette, candidates: &[Color]) {
    let others = candidates[1..]
        .iter()
        .map(|&c| format!("{} {c}", swatch(c)))
        .collect::<Vec<_>>()
        .join("  ");
    let others = if others.is_empty() {
        String::new()
    } else {
        format!("; other candidates: {others}")
    };
    println!(
        "source  {}  {}   (scheme {}{others})",
        swatch(palette.source),
        palette.source,
        palette.variant,
    );
    let roles = [
        "primary",
        "secondary",
        "tertiary",
        "primary_container",
        "surface",
        "surface_container_high",
        "on_surface",
        "outline",
        "error",
    ];
    for mode in [Mode::Dark, Mode::Light] {
        println!("\n{mode}");
        for role in roles {
            let c = palette.role(mode, role);
            println!("  {}  {c}  {role}", swatch(c));
        }
        let term = palette.terminal(mode);
        let row = |range: std::ops::Range<usize>| {
            term[range].iter().map(|&c| swatch(c)).collect::<String>()
        };
        println!("  terminal  {}\n            {}", row(0..8), row(8..16));
    }
}

const NVIM_SNIPPET: &str = r#"-- In init.lua: load the obojeno colorscheme and follow it live.
local colors = vim.fn.expand("~/.cache/obojeno/nvim")
vim.opt.rtp:prepend(colors)
if not pcall(vim.cmd.colorscheme, "obojeno") then
  vim.cmd.colorscheme("default")
end
local watcher = vim.uv.new_fs_event()
watcher:start(colors .. "/colors", {}, vim.schedule_wrap(function()
  pcall(vim.cmd.colorscheme, "obojeno")
end))"#;

fn print_targets(name: Option<&str>, paths: &Paths) -> anyhow::Result<()> {
    let config = Config::load(&paths.config_file)?;
    let Some(name) = name else {
        println!(
            "Built-in targets (enable with [targets.<name>] in {}):\n",
            paths.config_file.display()
        );
        for name in targets::BUILTIN_NAMES {
            let enabled = config
                .targets
                .get(name)
                .is_some_and(|t| t.enabled != Some(false));
            println!("  {} {name}", if enabled { "●" } else { "○" });
        }
        println!("\n`obojeno targets <name>` shows what a target writes and how to set it up.");
        return Ok(());
    };
    let target = targets::builtin(name).with_context(|| {
        format!(
            "no built-in target {name:?} ({})",
            targets::BUILTIN_NAMES.join(", ")
        )
    })?;
    println!("{name}\n");
    for output in &target.outputs {
        let what = match &output.kind {
            OutputKind::Text { .. } => "writes",
            OutputKind::Merge { base: Some(_), .. } => "merges `base` with colors into",
            OutputKind::Merge { base: None, .. } => "merges colors into",
            OutputKind::BlurredWallpaper { .. } => "renders a blurred wallpaper to",
        };
        println!(
            "  {what} {}",
            output.path.replace("{{cache}}", "~/.cache/obojeno")
        );
    }
    for (option, default) in &target.options {
        match default {
            Some(d) => println!("  option `{option}` (default {d})"),
            None => println!("  option `{option}` (required)"),
        }
    }
    println!("\nSetup: {}", target.setup);
    if name == "nvim" {
        println!("\n{NVIM_SNIPPET}");
    }
    Ok(())
}
