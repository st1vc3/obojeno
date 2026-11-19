//! The things obojeno does, shared by the daemon and the CLI (which runs them
//! itself when no daemon is running).

use std::path::Path;
use std::time::Duration;

use anyhow::Context as _;
use objc2_core_foundation::CGPoint;
use obojeno_core::Mode;
use obojeno_core::config::{Config, TransitionStyle};
use obojeno_core::ipc::{ModeChange, Request};

use crate::apply::{self, Applied};
use crate::paths::Paths;
use crate::state::{Source, State};
use crate::{imageio, macos, transition};

/// Runs a request that changes the theme. `Subscribe` and `State` only make
/// sense for the daemon.
pub fn execute(paths: &Paths, request: &Request) -> anyhow::Result<Applied> {
    match request {
        Request::Subscribe | Request::State => anyhow::bail!("only the daemon answers {request:?}"),
        Request::Apply { mode } => apply::apply(paths, *mode, None),
        Request::SetWallpaper {
            path,
            transition,
            duration,
            origin,
        } => set_wallpaper(paths, path, *transition, *duration, *origin),
        Request::SetMode { mode } => set_mode(paths, *mode),
        Request::SetColor { color } => {
            set_source(
                paths,
                color.map_or(Source::Wallpaper, |color| Source::Color { color }),
            )?;
            apply::apply(paths, None, None)
        }
    }
}

fn set_wallpaper(
    paths: &Paths,
    path: &Path,
    style: Option<TransitionStyle>,
    duration: Option<f64>,
    origin: Option<[f64; 2]>,
) -> anyhow::Result<Applied> {
    let path = std::fs::canonicalize(path)
        .with_context(|| format!("{} does not exist", path.display()))?;
    // Fail before touching the desktop if it is not an image.
    imageio::cg_image(&path)
        .map_err(anyhow::Error::msg)
        .with_context(|| format!("{} is not a readable image", path.display()))?;
    let config = Config::load(&paths.config_file)?;
    let duration = duration.unwrap_or(config.transition.duration);
    let options = transition::Options {
        style: style.unwrap_or(config.transition.style),
        duration: Duration::try_from_secs_f64(duration)
            .with_context(|| format!("invalid duration {duration}"))?,
        origin: origin.map(|[x, y]| CGPoint::new(x, y)),
    };
    set_source(paths, Source::Wallpaper)?;
    transition::set_wallpaper(&path, &options, || apply::apply(paths, None, Some(&path)))?
}

fn set_mode(paths: &Paths, change: ModeChange) -> anyhow::Result<Applied> {
    let mode = match change {
        ModeChange::Light => Mode::Light,
        ModeChange::Dark => Mode::Dark,
        ModeChange::Toggle => match macos::system_mode() {
            Mode::Light => Mode::Dark,
            Mode::Dark => Mode::Light,
        },
    };
    macos::set_system_mode(mode)?;
    // Pass the mode on: macOS reports the old one for a moment.
    apply::apply(paths, Some(mode), None)
}

fn set_source(paths: &Paths, source: Source) -> anyhow::Result<()> {
    let file = paths.state_file();
    let mut state = State::load(&file);
    state.source = source;
    state.save(&file)
}
