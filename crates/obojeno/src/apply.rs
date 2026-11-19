//! Generating the palette and theming every target.

use std::path::{Path, PathBuf};

use anyhow::Context as _;
use obojeno_core::config::Config;
use obojeno_core::output::{self, Wallpaper, write_if_changed};
use obojeno_core::template::Renderer;
use obojeno_core::{Color, Mode, Palette, palette};
use serde::Serialize;

use crate::paths::Paths;
use crate::reload::{self, Outcome};
use crate::state::{Source, State};

/// What happened to one target.
#[derive(Debug)]
pub struct TargetReport {
    pub name: String,
    /// Whether the target has files at all (borders only reloads).
    pub has_files: bool,
    pub written: Vec<PathBuf>,
    pub reload: Option<Outcome>,
    pub error: Option<String>,
}

#[derive(Debug)]
pub struct Report {
    pub source: Color,
    pub mode: Mode,
    pub wallpaper: Option<PathBuf>,
    pub targets: Vec<TargetReport>,
}

/// The palette and everything it was made from, for other tools and the GUI.
#[derive(Serialize)]
struct PaletteFile<'a> {
    mode: Mode,
    wallpaper: Option<&'a Path>,
    #[serde(flatten)]
    palette: &'a Palette,
}

/// Themes everything from the current source: the wallpaper, or the color
/// fixed with `obojeno color`. Pass `wallpaper` when it was just set: macOS
/// keeps reporting the previous one for a moment.
pub fn apply(
    paths: &Paths,
    mode: Option<Mode>,
    wallpaper: Option<&Path>,
) -> anyhow::Result<Report> {
    let config = Config::load(&paths.config_file)?;
    let mut state = State::load(&paths.state_file());
    let mode = mode.unwrap_or_else(crate::macos::system_mode);

    // A fixed color still has a wallpaper on screen (simple-bar blurs it), but
    // a wallpaper that cannot be read must not stop it.
    let wallpaper_path =
        wallpaper.map_or_else(crate::macos::current_wallpaper, |p| Ok(p.to_owned()));
    let wallpaper = match (&state.source, wallpaper_path) {
        (_, Ok(path)) => match Wallpaper::open_with(&path, crate::imageio::decode) {
            Ok(w) => Some(w),
            Err(e) if state.source == Source::Wallpaper => {
                return Err(e).with_context(|| format!("reading wallpaper {}", path.display()));
            }
            Err(e) => {
                tracing::warn!("reading wallpaper {}: {e}", path.display());
                None
            }
        },
        (Source::Wallpaper, Err(e)) => return Err(e),
        (Source::Color { .. }, Err(e)) => {
            tracing::warn!("{e:#}");
            None
        }
    };

    let source = match (&state.source, &wallpaper) {
        (Source::Color { color }, _) => *color,
        (Source::Wallpaper, Some(w)) => source_color(w, &mut state)?,
        (Source::Wallpaper, None) => unreachable!("handled above"),
    };
    let palette = Palette::from_source(source, config.scheme, config.contrast);

    let palette_json = serde_json::to_vec_pretty(&PaletteFile {
        mode,
        wallpaper: wallpaper.as_ref().map(|w| w.path.as_path()),
        palette: &palette,
    })?;
    let palette_path = paths.dirs.cache.join("palette.json");
    write_if_changed(&palette_path, &palette_json)
        .with_context(|| format!("writing {}", palette_path.display()))?;

    let targets = render_and_write(&config, paths, &palette, mode, wallpaper.as_ref())?;
    state.save(&paths.state_file())?;

    Ok(Report {
        source,
        mode,
        wallpaper: wallpaper.map(|w| w.path),
        targets,
    })
}

fn source_color(wallpaper: &Wallpaper, state: &mut State) -> anyhow::Result<Color> {
    if let Some(color) = state.cached_color(&wallpaper.key) {
        return Ok(color);
    }
    let image = wallpaper
        .image()
        .map_err(|e| anyhow::anyhow!("decoding {}: {e}", wallpaper.path.display()))?;
    let color = palette::source_color(image);
    state.cache_color(&wallpaper.key, color);
    Ok(color)
}

fn render_and_write(
    config: &Config,
    paths: &Paths,
    palette: &Palette,
    mode: Mode,
    wallpaper: Option<&Wallpaper>,
) -> anyhow::Result<Vec<TargetReport>> {
    let dirs = &paths.dirs;
    let targets = config.resolve_targets(|p| std::fs::read_to_string(dirs.expand(p)))?;
    let renderer = Renderer::new();

    let mut reports = Vec::new();
    let mut reloads = Vec::new();
    for resolved in &targets {
        let mut report = TargetReport {
            name: resolved.target.name.clone(),
            has_files: !resolved.target.outputs.is_empty(),
            written: Vec::new(),
            reload: None,
            error: None,
        };
        match output::render_target(resolved, palette, mode, wallpaper, dirs, &renderer) {
            Ok(rendered) => {
                for file in &rendered.files {
                    match write_if_changed(&file.path, &file.contents) {
                        Ok(true) => report.written.push(file.path.clone()),
                        Ok(false) => {}
                        Err(e) => {
                            report.error = Some(format!("writing {}: {e}", file.path.display()));
                            break;
                        }
                    }
                }
                // Targets without files (borders) have nothing to compare, so
                // they always reload; the rest only when something changed.
                let changed = !report.written.is_empty() || rendered.files.is_empty();
                if report.error.is_none() && changed && !rendered.reload.is_empty() {
                    reloads.push((reports.len(), rendered));
                }
            }
            Err(e) => report.error = Some(e.to_string()),
        }
        reports.push(report);
    }

    // Reloads talk to separate apps; run them side by side so the whole
    // desktop changes at once.
    let outcomes: Vec<(usize, Outcome)> = std::thread::scope(|scope| {
        let handles: Vec<_> = reloads
            .iter()
            .map(|(i, rendered)| {
                scope.spawn(move || {
                    (
                        *i,
                        reload::reload_target(
                            &rendered.reload,
                            rendered.only_if_running.as_deref(),
                        ),
                    )
                })
            })
            .collect();
        handles
            .into_iter()
            .map(|h| h.join().expect("reload thread panicked"))
            .collect()
    });
    for (i, outcome) in outcomes {
        reports[i].reload = Some(outcome);
    }
    Ok(reports)
}
