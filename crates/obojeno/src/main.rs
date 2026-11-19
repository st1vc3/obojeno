mod apply;
mod imageio;
mod macos;
mod paths;
mod reload;
mod state;

use std::path::PathBuf;
use std::process::ExitCode;

use anyhow::Context as _;
use clap::{Parser, Subcommand};
use obojeno_core::config::Config;
use obojeno_core::targets::{self, OutputKind};
use obojeno_core::{Color, Mode, Palette, SchemeVariant, palette};

use crate::apply::Report;
use crate::paths::Paths;
use crate::reload::Outcome;
use crate::state::{Source, State};

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
        /// Theme for this mode instead of the system appearance
        #[arg(long)]
        mode: Option<Mode>,
    },
    /// Theme from a fixed color instead of the wallpaper
    Color {
        /// A hex color such as #ff4057
        #[arg(required_unless_present = "wallpaper")]
        color: Option<Color>,
        /// Go back to following the wallpaper
        #[arg(long, conflicts_with = "color")]
        wallpaper: bool,
        #[arg(long)]
        mode: Option<Mode>,
    },
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

fn main() -> ExitCode {
    let cli = Cli::parse();
    let level = match cli.verbose {
        0 => "warn",
        1 => "info",
        _ => "debug",
    };
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_env("OBOJENO_LOG")
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new(level)),
        )
        .with_writer(std::io::stderr)
        .without_time()
        .with_target(false)
        .init();

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
    match cli.command {
        Command::Apply { mode } => Ok(print_report(&apply::apply(&paths, mode, None)?, &paths)),
        Command::Set { image, mode } => {
            let image = std::fs::canonicalize(&image)
                .with_context(|| format!("{} does not exist", image.display()))?;
            // Fail before touching the desktop if it is not an image.
            imageio::decode(&image)
                .map_err(anyhow::Error::msg)
                .with_context(|| format!("{} is not a readable image", image.display()))?;
            macos::set_wallpaper(&image)?;
            set_source(&paths, Source::Wallpaper)?;
            Ok(print_report(
                &apply::apply(&paths, mode, Some(&image))?,
                &paths,
            ))
        }
        Command::Color { color, mode, .. } => {
            let source = color.map_or(Source::Wallpaper, |color| Source::Color { color });
            set_source(&paths, source)?;
            Ok(print_report(&apply::apply(&paths, mode, None)?, &paths))
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
            Ok(ExitCode::SUCCESS)
        }
        Command::Targets { name } => {
            print_targets(name.as_deref(), &paths)?;
            Ok(ExitCode::SUCCESS)
        }
    }
}

fn set_source(paths: &Paths, source: Source) -> anyhow::Result<()> {
    let mut state = State::load(&paths.state_file());
    state.source = source;
    state.save(&paths.state_file())
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
                Some(Outcome::NotInstalled(cmd)) => Some(format!("{cmd} not installed")),
                Some(Outcome::Failed(e)) => {
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
