//! Building the gallery and indexing its images.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use anyhow::Context as _;
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
    let library = library_dir(paths, config);
    let library = library.canonicalize().unwrap_or(library);
    let folders: Vec<Folder> = folder_paths(paths, config)
        .into_iter()
        .map(|path| {
            let is_library = path == library;
            match scan(&path) {
                Ok(images) => Folder {
                    items: images.iter().filter_map(|p| item(p)).collect(),
                    path,
                    library: is_library,
                    error: None,
                },
                // An empty library just has not been imported into yet.
                Err(e) if is_library && e.kind() == std::io::ErrorKind::NotFound => Folder {
                    path,
                    library: true,
                    items: Vec::new(),
                    error: None,
                },
                Err(e) => Folder {
                    path,
                    library: is_library,
                    items: Vec::new(),
                    error: Some(folder_problem(&e)),
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

pub fn library_dir(paths: &Paths, config: &Config) -> PathBuf {
    paths.dirs.expand(&config.library)
}

/// The library, then the configured folders, without duplicates.
/// Canonical, like the wallpaper paths macOS reports and history records
/// (`/tmp` is really `/private/tmp`), so images compare equal to them.
fn folder_paths(paths: &Paths, config: &Config) -> Vec<PathBuf> {
    let canonical = |dir: PathBuf| dir.canonicalize().unwrap_or(dir);
    let mut folders = vec![canonical(library_dir(paths, config))];
    for dir in &config.wallpaper_dirs {
        let dir = canonical(paths.dirs.expand(dir));
        if !folders.contains(&dir) {
            folders.push(dir);
        }
    }
    folders
}

/// Every gallery image in display order, for next/previous/random.
pub fn ordered_images(paths: &Paths, config: &Config) -> Vec<PathBuf> {
    let mut images: Vec<PathBuf> = Vec::new();
    for dir in folder_paths(paths, config) {
        for image in scan(&dir).unwrap_or_default() {
            if !images.contains(&image) {
                images.push(image);
            }
        }
    }
    images
}

/// Copies images into the library, renaming on clashes ("name 2.jpg").
/// Returns the new paths.
pub fn import(paths: &Paths, config: &Config, images: &[PathBuf]) -> anyhow::Result<Vec<PathBuf>> {
    let library = library_dir(paths, config);
    std::fs::create_dir_all(&library).with_context(|| format!("creating {}", library.display()))?;
    let mut imported = Vec::new();
    for image in images {
        imageio::cg_image(image)
            .map_err(anyhow::Error::msg)
            .with_context(|| format!("{} is not an image macOS can show", image.display()))?;
        let target = free_name(&library, image)?;
        std::fs::copy(image, &target).with_context(|| format!("copying {}", image.display()))?;
        imported.push(target);
    }
    Ok(imported)
}

fn free_name(dir: &Path, image: &Path) -> anyhow::Result<PathBuf> {
    let stem = image
        .file_stem()
        .context("image has no name")?
        .to_string_lossy();
    let ext = image
        .extension()
        .map(|e| format!(".{}", e.to_string_lossy()))
        .unwrap_or_default();
    // A bound, so a folder full of copies fails rather than hangs.
    (1..=10_000)
        .map(|n| match n {
            1 => dir.join(format!("{stem}{ext}")),
            n => dir.join(format!("{stem} {n}{ext}")),
        })
        .find(|p| !p.exists())
        .context("no free name")
}

/// Moves a library image to the Trash. Images elsewhere are only shown by
/// reference and are never touched.
pub fn remove(paths: &Paths, config: &Config, image: &Path) -> anyhow::Result<()> {
    let library = library_dir(paths, config)
        .canonicalize()
        .context("the library does not exist")?;
    let image = image
        .canonicalize()
        .with_context(|| format!("{} does not exist", image.display()))?;
    anyhow::ensure!(
        image.parent() == Some(library.as_path()),
        "{} is not in the library ({}); obojeno only removes images it imported",
        image.display(),
        library.display()
    );
    crate::macos::trash(&image)
}

/// Why a folder cannot be shown, phrased to follow "This folder".
fn folder_problem(e: &std::io::Error) -> String {
    match e.kind() {
        std::io::ErrorKind::NotFound => "doesn't exist".to_owned(),
        std::io::ErrorKind::PermissionDenied => "can't be read without permission".to_owned(),
        std::io::ErrorKind::NotADirectory => "isn't a folder".to_owned(),
        _ => format!("can't be read ({e})"),
    }
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn gallery_paths_are_canonical() {
        let dir = tempfile::tempdir().unwrap();
        let real = dir.path().join("real");
        std::fs::create_dir(&real).unwrap();
        std::fs::write(real.join("a.png"), "").unwrap();
        let link = dir.path().join("link");
        std::os::unix::fs::symlink(&real, &link).unwrap();
        let paths = Paths {
            dirs: obojeno_core::output::Dirs {
                home: dir.path().to_owned(),
                cache: dir.path().join("cache"),
                config: dir.path().join("config"),
            },
            config_file: dir.path().join("config.toml"),
            state: dir.path().join("state"),
        };
        let config = Config {
            library: link.display().to_string(),
            ..Config::default()
        };
        let images = ordered_images(&paths, &config);
        assert_eq!(images, [real.canonicalize().unwrap().join("a.png")]);
    }

    #[test]
    fn free_names_never_overwrite() {
        let dir = tempfile::tempdir().unwrap();
        let image = Path::new("/elsewhere/Marble.jpg");
        assert_eq!(
            free_name(dir.path(), image).unwrap(),
            dir.path().join("Marble.jpg")
        );
        std::fs::write(dir.path().join("Marble.jpg"), "").unwrap();
        assert_eq!(
            free_name(dir.path(), image).unwrap(),
            dir.path().join("Marble 2.jpg")
        );
        std::fs::write(dir.path().join("Marble 2.jpg"), "").unwrap();
        assert_eq!(
            free_name(dir.path(), image).unwrap(),
            dir.path().join("Marble 3.jpg")
        );
    }
}
