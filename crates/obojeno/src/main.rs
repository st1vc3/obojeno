mod actions;
mod analyze;
mod apply;
mod daemon;
mod desktop;
mod detect;
mod gallery;
mod imageio;
mod large_alloc;
mod layers;
mod lid;
mod lid_view;
mod log_file;
mod macos;
mod paths;
mod programs;
mod reload;
mod screen_images;
mod service;
mod setup;
mod state;
mod transition;
mod watch;

use std::collections::BTreeSet;
use std::path::PathBuf;
use std::process::ExitCode;

use anyhow::Context as _;
use clap::{Parser, Subcommand};
use obojeno_core::config::{Config, TransitionStyle};
use obojeno_core::ipc::{Direction, ModeChange, Outcome, Reply, Report, Request, TargetInfo};
use obojeno_core::targets::{self, OutputKind};
use obojeno_core::{Color, Mode, Palette, SchemeVariant, palette};

use crate::paths::Paths;

#[global_allocator]
static ALLOCATOR: large_alloc::Allocator = large_alloc::Allocator;

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
    /// Set the next image of the gallery as wallpaper
    Next,
    /// Set the previous image of the gallery as wallpaper
    Previous,
    /// Set a random image of the gallery as wallpaper
    Random,
    /// Freeze the lid animation at a percentage (0 open, 100 shut) or `off`,
    /// to see it with the lid open. Needs the lid animation on and a running
    /// daemon.
    #[command(name = "lid-preview")]
    LidPreview { percent: String },
    /// Copy images into the gallery's library
    Import {
        #[arg(required = true)]
        images: Vec<PathBuf>,
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
    /// Run the daemon at login (a launchd agent)
    Service {
        #[command(subcommand)]
        action: ServiceAction,
    },
    /// Run in the background: follow wallpaper and appearance changes, and
    /// serve the menu bar app
    Daemon,
    /// Find an image's source color (and make its gallery thumbnail): the
    /// daemon's helper process, printing JSON
    #[command(hide = true)]
    Analyze {
        image: PathBuf,
        #[arg(long)]
        thumbnail: Option<PathBuf>,
    },
    /// Decode an image's pixels into a file and print their size: the
    /// daemon's helper process
    #[command(name = "decode-pixels", hide = true)]
    DecodePixels {
        image: PathBuf,
        #[arg(long)]
        out: PathBuf,
        #[arg(long)]
        min_side: Option<u32>,
        #[arg(long)]
        bgra: bool,
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
    /// Turn on the targets for the apps installed on this Mac, and show
    /// what is left to set up
    Setup {
        /// Only show what would change
        #[arg(long)]
        dry_run: bool,
    },
}

#[derive(Clone, Copy, Subcommand)]
enum ServiceAction {
    /// Start the daemon now and at every login
    Install,
    /// Stop it and no longer start it at login
    Uninstall,
    /// Whether it is installed and running
    Status,
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
        .with_writer(log_file::writer)
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
        Command::Next => step(Direction::Next),
        Command::Previous => step(Direction::Previous),
        Command::Random => step(Direction::Random),
        Command::LidPreview { percent } => return lid_preview(&paths, &percent),
        Command::Import { images } => return import(&paths, &images),
        Command::Service { action } => return run_service(action),
        Command::Daemon => return daemon::run(paths).map(|()| ExitCode::SUCCESS),
        Command::DecodePixels {
            image,
            out,
            min_side,
            bgra,
        } => {
            let (width, height) =
                imageio::write_pixels(&image, min_side, bgra, &out).map_err(anyhow::Error::msg)?;
            println!("{width} {height}");
            return Ok(ExitCode::SUCCESS);
        }
        Command::Analyze { image, thumbnail } => {
            let analysis =
                analyze::analyze(&image, thumbnail.as_ref()).map_err(anyhow::Error::msg)?;
            println!("{}", serde_json::to_string(&analysis)?);
            return Ok(ExitCode::SUCCESS);
        }
        Command::Gallery => return show_gallery(&paths),
        Command::Preview { image, scheme } => return preview(&paths, image, scheme),
        Command::Targets { name } => return show_targets(name.as_deref(), &paths),
        Command::Setup { dry_run } => return run_setup(&paths, dry_run),
    };
    // A running daemon does the work, so changes animate and never race it.
    let report = match daemon::request(&paths, &request)? {
        Some(Reply::Report(report)) => report,
        Some(_) => anyhow::bail!("unexpected reply from the daemon"),
        None => actions::execute(&paths, &request)?.report,
    };
    Ok(print_report(&report, &paths))
}

const fn step(direction: Direction) -> Request {
    Request::Step { direction }
}

fn lid_preview(paths: &Paths, percent: &str) -> anyhow::Result<ExitCode> {
    let percent = match percent.trim_end_matches('%') {
        "off" => None,
        value => Some(
            value
                .parse::<u16>()
                .ok()
                .filter(|p| *p <= 100)
                .context("the preview is a percentage from 0 to 100, or `off`")?,
        ),
    };
    daemon::request(paths, &Request::LidPreview { percent })?
        .context("the lid preview needs a running daemon")?;
    Ok(ExitCode::SUCCESS)
}

fn import(paths: &Paths, images: &[PathBuf]) -> anyhow::Result<ExitCode> {
    let images = images
        .iter()
        .map(std::path::absolute)
        .collect::<Result<Vec<_>, _>>()?;
    let request = Request::Import {
        paths: images.clone(),
    };
    if daemon::request(paths, &request)?.is_none() {
        gallery::import(paths, &Config::load(&paths.config_file)?, &images)?;
    }
    let count = images.len();
    println!(
        "Imported {count} image{} into the library.",
        if count == 1 { "" } else { "s" }
    );
    Ok(ExitCode::SUCCESS)
}

fn show_gallery(paths: &Paths) -> anyhow::Result<ExitCode> {
    let gallery = match daemon::request(paths, &Request::Gallery)? {
        Some(Reply::Gallery(gallery)) => gallery,
        Some(_) => anyhow::bail!("unexpected reply from the daemon"),
        None => gallery::build_blocking(
            paths,
            macos::current_wallpaper().ok().as_deref(),
            macos::system_mode(),
        )?,
    };
    print_gallery(&gallery);
    Ok(ExitCode::SUCCESS)
}

fn preview(
    paths: &Paths,
    image: Option<PathBuf>,
    scheme: Option<SchemeVariant>,
) -> anyhow::Result<ExitCode> {
    let config = Config::load(&paths.config_file)?;
    let image = match image {
        Some(image) => image,
        None => macos::current_wallpaper()?,
    };
    let decoded = imageio::decode(&image, None)
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

fn run_service(action: ServiceAction) -> anyhow::Result<ExitCode> {
    let describe = |running: bool| if running { "running" } else { "not running" };
    match action {
        ServiceAction::Install => {
            service::install()?;
            println!("obojeno now runs at login (log: ~/Library/Logs/obojeno.log).");
        }
        ServiceAction::Uninstall => {
            service::uninstall()?;
            println!("obojeno no longer runs at login.");
        }
        ServiceAction::Status => match service::status()? {
            service::Status::NotInstalled => println!("not installed"),
            service::Status::Installed { running } => println!("installed, {}", describe(running)),
            service::Status::Managed { running } => {
                println!("managed by home-manager, {}", describe(running));
            }
        },
    }
    Ok(ExitCode::SUCCESS)
}

/// A block of `c` in the terminal, `width` characters wide.
fn swatch(c: Color, width: usize) -> String {
    format!("\x1b[48;2;{};{};{}m{:width$}\x1b[0m", c.r, c.g, c.b, "")
}

fn print_report(report: &Report, paths: &Paths) -> ExitCode {
    let from = report
        .wallpaper
        .as_ref()
        .map_or_else(String::new, |w| format!(" from {}", w.display()));
    println!(
        "{} {} {}{from}",
        swatch(report.source, 4),
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
            item.swatches.iter().map(|&c| swatch(c, 2)).collect()
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
        .map(|&c| format!("{} {c}", swatch(c, 4)))
        .collect::<Vec<_>>()
        .join("  ");
    let others = if others.is_empty() {
        String::new()
    } else {
        format!("; other candidates: {others}")
    };
    println!(
        "source  {}  {}   (scheme {}{others})",
        swatch(palette.source, 4),
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
            println!("  {}  {c}  {role}", swatch(c, 4));
        }
        let term = palette.terminal(mode);
        let row = |range: std::ops::Range<usize>| {
            term[range]
                .iter()
                .map(|&c| swatch(c, 4))
                .collect::<String>()
        };
        println!("  terminal  {}\n            {}", row(0..8), row(8..16));
    }
}

fn show_targets(name: Option<&str>, paths: &Paths) -> anyhow::Result<ExitCode> {
    let Some(name) = name else {
        print_status(&setup::targets(paths)?, &BTreeSet::new(), false);
        println!(
            "\nTurn them on in the menu bar app, with `obojeno setup`, or with \
             [targets.<name>] in {}.\n`obojeno targets <name>` shows what a target \
             writes and how to set it up.",
            paths.config_file.display()
        );
        return Ok(ExitCode::SUCCESS);
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
    if let Some(snippet) = target.snippet {
        println!("\n{snippet}");
    }
    Ok(ExitCode::SUCCESS)
}

fn run_setup(paths: &Paths, dry_run: bool) -> anyhow::Result<ExitCode> {
    let infos = setup::targets(paths)?;
    let change = setup::enable_found(&infos, &Config::load(&paths.config_file)?);
    let turning_on: BTreeSet<String> = change
        .targets
        .as_ref()
        .map(|t| t.keys().cloned().collect())
        .unwrap_or_default();
    print_status(&infos, &turning_on, dry_run);
    if !dry_run && !turning_on.is_empty() {
        let request = Request::Configure { change };
        if daemon::request(paths, &request)?.is_none() {
            actions::execute(paths, &request)?;
        }
    }
    // Steps for targets turned on now, unless already done, and for ones
    // known to need them; not for what obojeno cannot check.
    let todo = infos.iter().filter(|t| {
        t.snippet.is_some()
            && match t.set_up {
                Some(set_up) => !set_up && (t.enabled || turning_on.contains(&t.name)),
                None => turning_on.contains(&t.name),
            }
    });
    for t in todo {
        println!("\n{}: {}", t.title, t.setup);
        if let Some(snippet) = &t.snippet {
            println!("\n{snippet}");
        }
    }
    Ok(ExitCode::SUCCESS)
}

/// One line per target: on or off, and what was found.
fn print_status(infos: &[TargetInfo], turning_on: &BTreeSet<String>, dry_run: bool) {
    let width = infos.iter().map(|t| t.title.len()).max().unwrap_or(0);
    for t in infos {
        let now_on = turning_on.contains(&t.name);
        let status = if now_on {
            if dry_run {
                "found, would turn on".to_owned()
            } else {
                "found, turned on".to_owned()
            }
        } else if t.enabled {
            "on".to_owned()
        } else if !t.installed {
            "not found".to_owned()
        } else if let Some(option) = &t.missing {
            format!("found, needs `{option}` in the config")
        } else {
            "found, off".to_owned()
        };
        let mark = if t.enabled || now_on { "●" } else { "○" };
        println!("  {mark} {:width$}  {status}", t.title);
    }
}
