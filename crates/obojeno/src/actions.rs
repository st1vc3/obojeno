//! The things obojeno does, shared by the daemon and the CLI (which runs them
//! itself when no daemon is running).

use std::path::Path;
use std::time::Duration;

use anyhow::Context as _;
use objc2_core_foundation::CGPoint;
use obojeno_core::Mode;
use obojeno_core::config::{Config, ConfigChange, TransitionStyle};
use obojeno_core::ipc::{Direction, ModeChange, Request};
use obojeno_core::output::write_if_changed;

use crate::apply::{self, Applied};
use crate::paths::Paths;
use crate::state::{Source, State};
use crate::{gallery, imageio, macos, transition};

/// Runs a request that changes the theme. `Subscribe` and `State` only make
/// sense for the daemon.
pub fn execute(paths: &Paths, request: &Request) -> anyhow::Result<Applied> {
    match request {
        Request::Step { direction } => step(paths, *direction),
        Request::Subscribe
        | Request::State
        | Request::Gallery
        | Request::Import { .. }
        | Request::Remove { .. }
        | Request::LidPreview { .. }
        | Request::Targets
        | Request::SetUp { .. } => {
            anyhow::bail!("only the daemon answers {request:?}")
        }
        Request::Apply { mode } => apply::apply(paths, *mode, None),
        Request::SetWallpaper {
            path,
            transition,
            duration,
            origin,
        } => set_wallpaper(paths, path, *transition, *duration, *origin),
        Request::SetMode { mode } => set_mode(paths, *mode),
        Request::Configure { change } => {
            configure(paths, change)?;
            apply::apply(paths, None, None)
        }
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
    imageio::size(&path)
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

/// The next, previous or a random image of the gallery, from the wallpaper
/// last chosen.
fn step(paths: &Paths, direction: Direction) -> anyhow::Result<Applied> {
    let config = Config::load(&paths.config_file)?;
    let images = gallery::ordered_images(paths, &config);
    let n = images.len();
    anyhow::ensure!(
        n > 0,
        "the gallery is empty; import images or add a folder first"
    );
    let last = State::load(&paths.state_file())
        .history
        .first()
        .map(|h| h.path.clone());
    let current = last.and_then(|last| images.iter().position(|p| *p == last));
    let index = match direction {
        Direction::Next => current.map_or(0, |i| (i + 1) % n),
        Direction::Previous => current.map_or(n - 1, |i| (i + n - 1) % n),
        Direction::Random => random_other(n, current),
    };
    set_wallpaper(paths, &images[index], None, None, None)
}

/// A random index below `n`, avoiding `except` when there is a choice.
fn random_other(n: usize, except: Option<usize>) -> usize {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.subsec_nanos());
    match except {
        Some(skip) if n > 1 => {
            // Pick among the n - 1 others and step over the current one.
            let pick = usize::try_from(nanos).unwrap_or(0) % (n - 1);
            if pick >= skip { pick + 1 } else { pick }
        }
        _ => usize::try_from(nanos).unwrap_or(0) % n,
    }
}

fn configure(paths: &Paths, change: &ConfigChange) -> anyhow::Result<()> {
    let file = &paths.config_file;
    let text = match std::fs::read_to_string(file) {
        Ok(text) => text,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => String::new(),
        Err(e) => return Err(e).with_context(|| format!("reading {}", file.display())),
    };
    let edited = obojeno_core::config::edit(&text, change)
        .with_context(|| format!("editing {}", file.display()))?;
    write_if_changed(file, edited.as_bytes())
        .with_context(|| format!("writing {}", file.display()))?;
    Ok(())
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn random_never_repeats_the_current_image() {
        for _ in 0..200 {
            let i = random_other(3, Some(1));
            assert!(i < 3 && i != 1, "{i}");
        }
        assert_eq!(random_other(1, Some(0)), 0, "nothing else to pick");
        assert!(random_other(5, None) < 5);
    }
}
