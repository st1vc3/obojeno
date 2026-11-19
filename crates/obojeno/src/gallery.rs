//! Building the gallery and indexing its images.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use image::codecs::jpeg::JpegEncoder;
use obojeno_core::config::Config;
use obojeno_core::gallery::{Fingerprint, Folder, Gallery, Index, IndexEntry, Item, scan};
use obojeno_core::output::{Wallpaper, write_if_changed};
use obojeno_core::{Mode, Palette, palette};

use crate::imageio;
use crate::paths::Paths;
use crate::state::State;

/// Longest side of gallery thumbnails: sharp in a grid at 2x.
const THUMBNAIL_SIDE: u32 = 512;
const THUMBNAIL_QUALITY: u8 = 85;

/// Roles shown as a swatch strip under each thumbnail.
const SWATCH_ROLES: [&str; 4] = ["primary", "secondary", "tertiary", "surface_container_high"];

pub fn index_file(paths: &Paths) -> PathBuf {
    paths.dirs.cache.join("gallery/index.json")
}

fn thumbnail_dir(paths: &Paths) -> PathBuf {
    paths.dirs.cache.join("gallery")
}

/// Builds the gallery from what is indexed. Returns it with the images that
/// still need indexing, and prunes index entries for images that are gone.
pub fn build(
    paths: &Paths,
    config: &Config,
    state: &State,
    index: &mut Index,
    current: Option<&Path>,
    mode: Mode,
) -> (Gallery, Vec<PathBuf>) {
    let mut missing = BTreeSet::new();
    let mut item = |path: &Path| -> Option<Item> {
        // Gone or unreadable files drop out of the gallery.
        let fingerprint = Fingerprint::of(path).ok()?;
        let entry = index.get(path, fingerprint);
        if entry.is_none() {
            missing.insert(path.to_owned());
        }
        let swatches = entry.map_or_else(Vec::new, |e| {
            let palette = Palette::from_source(e.source, config.scheme, config.contrast);
            SWATCH_ROLES
                .iter()
                .map(|role| palette.role(mode, role))
                .collect()
        });
        Some(Item {
            path: path.to_owned(),
            source: entry.map(|e| e.source),
            swatches,
            thumbnail: entry.map(|e| e.thumbnail.clone()),
            current: current == Some(path),
        })
    };

    let recent: Vec<Item> = state.history.iter().filter_map(|h| item(&h.path)).collect();
    let folders: Vec<Folder> = config
        .wallpaper_dirs
        .iter()
        .map(|dir| {
            let path = paths.dirs.expand(dir);
            match scan(&path) {
                Ok(images) => Folder {
                    items: images.iter().filter_map(|p| item(p)).collect(),
                    path,
                    error: None,
                },
                Err(e) => Folder {
                    path,
                    items: Vec::new(),
                    error: Some(e.to_string()),
                },
            }
        })
        .collect();

    let shown: BTreeSet<&Path> = recent
        .iter()
        .chain(folders.iter().flat_map(|f| &f.items))
        .map(|i| i.path.as_path())
        .collect();
    index.retain(|p| shown.contains(p));

    let missing: Vec<PathBuf> = missing.into_iter().collect();
    let gallery = Gallery {
        recent,
        folders,
        pending: missing.len(),
    };
    (gallery, missing)
}

/// Makes the thumbnail and finds the source color of one image. Slow (a full
/// decode); run it off the main thread.
pub fn index_image(paths: &Paths, path: &Path) -> Result<IndexEntry, String> {
    let fingerprint = Fingerprint::of(path).map_err(|e| e.to_string())?;
    let wallpaper = Wallpaper::open_with(path, imageio::decode).map_err(|e| e.to_string())?;
    let source = palette::source_color(wallpaper.image().map_err(str::to_owned)?);

    let thumbnail = thumbnail_dir(paths).join(format!("{}.jpg", fingerprint.file_stem(path)));
    let small = imageio::thumbnail(path, THUMBNAIL_SIDE)?;
    let mut jpeg = Vec::new();
    JpegEncoder::new_with_quality(&mut jpeg, THUMBNAIL_QUALITY)
        .encode_image(&small.to_rgb8())
        .map_err(|e| e.to_string())?;
    write_if_changed(&thumbnail, &jpeg)
        .map_err(|e| format!("writing {}: {e}", thumbnail.display()))?;

    Ok(IndexEntry {
        fingerprint,
        key: wallpaper.key,
        source,
        thumbnail,
    })
}

/// Indexes everything missing right away, for the CLI without a daemon.
pub fn build_blocking(
    paths: &Paths,
    current: Option<&Path>,
    mode: Mode,
) -> anyhow::Result<Gallery> {
    let config = Config::load(&paths.config_file)?;
    let mut state = State::load(&paths.state_file());
    let file = index_file(paths);
    let mut index = Index::load(&file);
    let (_, missing) = build(paths, &config, &state, &mut index, current, mode);
    let results: Vec<_> = std::thread::scope(|scope| {
        let workers = parallelism();
        let chunks: Vec<_> = missing
            .chunks(missing.len().div_ceil(workers).max(1))
            .collect();
        let handles: Vec<_> = chunks
            .into_iter()
            .map(|chunk| {
                scope.spawn(move || {
                    chunk
                        .iter()
                        .map(|p| (p.clone(), index_image(paths, p)))
                        .collect::<Vec<_>>()
                })
            })
            .collect();
        handles
            .into_iter()
            .flat_map(|h| h.join().expect("indexing thread panicked"))
            .collect()
    });
    for (path, result) in results {
        match result {
            Ok(entry) => {
                state.cache_color(&entry.key, entry.source);
                index.entries.insert(path, entry);
            }
            Err(e) => tracing::warn!("indexing {}: {e}", path.display()),
        }
    }
    let (gallery, _) = build(paths, &config, &state, &mut index, current, mode);
    index.save(&file)?;
    state.save(&paths.state_file())?;
    Ok(gallery)
}

/// Half the cores: indexing runs in the background and should not make the
/// machine sluggish.
pub fn parallelism() -> usize {
    std::thread::available_parallelism().map_or(2, |n| (n.get() / 2).max(1))
}
