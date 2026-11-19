//! The wallpaper gallery: recently used wallpapers plus the images in the
//! configured folders, each with a thumbnail and its colors.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::Color;
use crate::output::write_if_changed;

/// Image types macOS can show as wallpaper, by extension (lowercase).
pub const IMAGE_EXTENSIONS: [&str; 12] = [
    "avif", "bmp", "gif", "heic", "heif", "jpeg", "jpg", "jxl", "png", "tif", "tiff", "webp",
];

/// How deep folder scans go, so a stray link to `/` cannot scan everything.
const MAX_DEPTH: usize = 3;

pub fn is_image(path: &Path) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| IMAGE_EXTENSIONS.contains(&e.to_ascii_lowercase().as_str()))
}

/// Images in `dir` and its subfolders, sorted by path. Hidden files and
/// folders are skipped.
pub fn scan(dir: &Path) -> std::io::Result<Vec<PathBuf>> {
    let mut images = Vec::new();
    scan_into(dir, 0, &mut images)?;
    images.sort();
    Ok(images)
}

fn scan_into(dir: &Path, depth: usize, images: &mut Vec<PathBuf>) -> std::io::Result<()> {
    for entry in std::fs::read_dir(dir)? {
        let entry = entry?;
        if entry.file_name().to_string_lossy().starts_with('.') {
            continue;
        }
        let path = entry.path();
        // Follows links, so linked folders and images count.
        let Ok(meta) = std::fs::metadata(&path) else {
            continue;
        };
        if meta.is_dir() {
            if depth < MAX_DEPTH {
                // An unreadable subfolder should not hide the rest.
                let _ = scan_into(&path, depth + 1, images);
            }
        } else if is_image(&path) {
            images.push(path);
        }
    }
    Ok(())
}

/// What the gallery shows.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Gallery {
    /// Most recent first.
    pub recent: Vec<Item>,
    pub folders: Vec<Folder>,
    /// Images still being indexed; their thumbnails and colors follow in
    /// later updates.
    pub pending: usize,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Folder {
    pub path: PathBuf,
    pub items: Vec<Item>,
    /// Why the folder could not be read, if it could not.
    pub error: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Item {
    pub path: PathBuf,
    /// The wallpaper's source color; `None` until indexed.
    pub source: Option<Color>,
    /// A few colors of the theme it would make (current scheme and mode).
    pub swatches: Vec<Color>,
    pub thumbnail: Option<PathBuf>,
    /// Whether this is the wallpaper on screen.
    pub current: bool,
}

/// Size and modification time identify a version of a file well enough to
/// reuse its thumbnail and color without reading it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Fingerprint {
    pub size: u64,
    pub modified_nanos: u128,
}

impl Fingerprint {
    pub fn of(path: &Path) -> std::io::Result<Self> {
        let meta = std::fs::metadata(path)?;
        Ok(Self {
            size: meta.len(),
            modified_nanos: meta
                .modified()?
                .duration_since(UNIX_EPOCH)
                .map_or(0, |d| d.as_nanos()),
        })
    }

    /// A file name for derived files (thumbnails) of this version.
    pub fn file_stem(self, path: &Path) -> String {
        let mut hasher = Sha256::new();
        hasher.update(path.as_os_str().as_encoded_bytes());
        hasher.update(self.size.to_le_bytes());
        hasher.update(self.modified_nanos.to_le_bytes());
        hasher.finalize()[..16]
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect()
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct IndexEntry {
    pub fingerprint: Fingerprint,
    /// SHA-256 of the contents, the key colors are cached under.
    pub key: String,
    pub source: Color,
    pub thumbnail: PathBuf,
}

/// Thumbnails and colors of images seen so far, persisted between runs.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Index {
    /// The extraction version the colors were made with.
    #[serde(default)]
    pub version: u32,
    pub entries: BTreeMap<PathBuf, IndexEntry>,
}

impl Index {
    /// Loads the index; one made by an older extraction starts over.
    pub fn load(path: &Path) -> Self {
        std::fs::read(path)
            .ok()
            .and_then(|bytes| serde_json::from_slice::<Self>(&bytes).ok())
            .filter(|index| index.version == crate::palette::EXTRACTION_VERSION)
            .unwrap_or_else(|| Self {
                version: crate::palette::EXTRACTION_VERSION,
                entries: BTreeMap::new(),
            })
    }

    pub fn save(&self, path: &Path) -> std::io::Result<()> {
        let mut json = serde_json::to_vec(self).map_err(std::io::Error::other)?;
        json.push(b'\n');
        write_if_changed(path, &json).map(drop)
    }

    /// The entry for `path` if it is up to date with the file on disk and its
    /// thumbnail still exists.
    pub fn get(&self, path: &Path, fingerprint: Fingerprint) -> Option<&IndexEntry> {
        self.entries
            .get(path)
            .filter(|e| e.fingerprint == fingerprint && e.thumbnail.exists())
    }

    /// Drops entries for paths not in `keep`, deleting their thumbnails.
    pub fn retain(&mut self, keep: impl Fn(&Path) -> bool) {
        self.entries.retain(|path, entry| {
            let kept = keep(path);
            if !kept {
                let _ = std::fs::remove_file(&entry.thumbnail);
            }
            kept
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recognizes_images_case_insensitively() {
        assert!(is_image(Path::new("/a/b.HEIC")));
        assert!(is_image(Path::new("b.jpeg")));
        assert!(!is_image(Path::new("b.txt")));
        assert!(!is_image(Path::new("jpg")));
    }

    #[test]
    fn scans_nested_folders_and_skips_hidden_ones() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        for path in [
            "b.png",
            "a.JPG",
            "notes.txt",
            ".hidden.png",
            "sub/c.heic",
            ".git/d.png",
            "1/2/3/4/deep.png",
        ] {
            let path = root.join(path);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, b"").unwrap();
        }
        let found: Vec<_> = scan(root)
            .unwrap()
            .into_iter()
            .map(|p| p.strip_prefix(root).unwrap().to_string_lossy().into_owned())
            .collect();
        assert_eq!(found, ["a.JPG", "b.png", "sub/c.heic"]);
    }

    #[test]
    fn missing_folder_is_an_error() {
        assert!(scan(Path::new("/definitely/not/here")).is_err());
    }

    #[test]
    fn index_entries_go_stale_when_the_file_changes() {
        let dir = tempfile::tempdir().unwrap();
        let image = dir.path().join("w.png");
        let thumb = dir.path().join("t.jpg");
        std::fs::write(&image, b"one").unwrap();
        std::fs::write(&thumb, b"").unwrap();
        let fingerprint = Fingerprint::of(&image).unwrap();
        let mut index = Index::load(&dir.path().join("none.json"));
        index.entries.insert(
            image.clone(),
            IndexEntry {
                fingerprint,
                key: "k".into(),
                source: Color::new(1, 2, 3),
                thumbnail: thumb.clone(),
            },
        );
        assert!(index.get(&image, fingerprint).is_some());

        std::fs::write(&image, b"longer").unwrap();
        assert!(
            index
                .get(&image, Fingerprint::of(&image).unwrap())
                .is_none()
        );

        let path = dir.path().join("index.json");
        index.save(&path).unwrap();
        assert_eq!(Index::load(&path), index);

        index.retain(|_| false);
        assert!(index.entries.is_empty());
        assert!(!thumb.exists(), "thumbnail deleted with its entry");
    }

    #[test]
    fn thumbnail_names_differ_per_version() {
        let a = Fingerprint {
            size: 1,
            modified_nanos: 1,
        };
        let b = Fingerprint {
            size: 1,
            modified_nanos: 2,
        };
        let p = Path::new("/w.png");
        assert_ne!(a.file_stem(p), b.file_stem(p));
        assert_ne!(a.file_stem(p), a.file_stem(Path::new("/x.png")));
        assert_eq!(a.file_stem(p).len(), 32);
    }
}
