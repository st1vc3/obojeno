//! What obojeno remembers between runs.

use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::Context as _;
use obojeno_core::Color;
use obojeno_core::output::write_if_changed;
use serde::{Deserialize, Serialize};

/// Source colors of recently used wallpapers, by content hash, so re-applying
/// never decodes an image twice. Bounded; oldest entries are dropped.
const MAX_CACHED_COLORS: usize = 256;

/// Wallpapers remembered for the gallery's recent row.
const MAX_HISTORY: usize = 30;

/// Bumped when source colors change: core's extraction version, plus one
/// for decoding through ImageIO.
const EXTRACTION_VERSION: u32 = obojeno_core::palette::EXTRACTION_VERSION * 100 + 1;

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "lowercase")]
pub enum Source {
    /// Follow the desktop wallpaper.
    #[default]
    Wallpaper,
    /// A fixed color chosen with `obojeno color`.
    Color { color: Color },
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct State {
    pub source: Source,
    extraction_version: u32,
    /// Most recent first.
    source_colors: Vec<(String, Color)>,
    /// Wallpapers themed from, most recent first.
    pub history: Vec<HistoryEntry>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HistoryEntry {
    pub path: PathBuf,
    /// Seconds since the Unix epoch.
    pub used_at: u64,
}

impl State {
    /// Loads the state; a missing or unreadable file starts fresh, since
    /// everything in it can be recomputed.
    pub fn load(path: &Path) -> Self {
        let state: Self = match std::fs::read(path) {
            Ok(bytes) => serde_json::from_slice(&bytes).unwrap_or_else(|e| {
                tracing::warn!("ignoring unreadable {}: {e}", path.display());
                Self::default()
            }),
            Err(_) => Self::default(),
        };
        if state.extraction_version == EXTRACTION_VERSION {
            state
        } else {
            Self {
                extraction_version: EXTRACTION_VERSION,
                source_colors: Vec::new(),
                ..state
            }
        }
    }

    pub fn save(&self, path: &Path) -> anyhow::Result<()> {
        let mut json = serde_json::to_vec_pretty(self)?;
        json.push(b'\n');
        write_if_changed(path, &json).with_context(|| format!("writing {}", path.display()))?;
        Ok(())
    }

    pub fn cached_color(&self, key: &str) -> Option<Color> {
        self.source_colors
            .iter()
            .find(|(k, _)| k == key)
            .map(|(_, c)| *c)
    }

    /// Moves `path` to the front of the history.
    pub fn record_wallpaper(&mut self, path: &Path) {
        self.history.retain(|e| e.path != path);
        let used_at = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |d| d.as_secs());
        self.history.insert(
            0,
            HistoryEntry {
                path: path.to_owned(),
                used_at,
            },
        );
        self.history.truncate(MAX_HISTORY);
    }

    pub fn cache_color(&mut self, key: &str, color: Color) {
        self.source_colors.retain(|(k, _)| k != key);
        self.source_colors.insert(0, (key.to_owned(), color));
        self.source_colors.truncate(MAX_CACHED_COLORS);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips_and_caches() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("state.json");
        let mut state = State::load(&path);
        assert_eq!(state.source, Source::Wallpaper);
        state.source = Source::Color {
            color: Color::new(1, 2, 3),
        };
        state.cache_color("a", Color::new(9, 9, 9));
        state.save(&path).unwrap();

        let loaded = State::load(&path);
        assert_eq!(loaded.source, state.source);
        assert_eq!(loaded.cached_color("a"), Some(Color::new(9, 9, 9)));
        assert_eq!(loaded.cached_color("b"), None);
    }

    #[test]
    fn cache_is_bounded_and_most_recent_first() {
        let mut state = State::default();
        for i in 0..MAX_CACHED_COLORS + 10 {
            state.cache_color(&i.to_string(), Color::new(0, 0, 0));
        }
        assert_eq!(state.source_colors.len(), MAX_CACHED_COLORS);
        assert!(state.cached_color("0").is_none());
        state.cache_color("100", Color::new(1, 1, 1));
        assert_eq!(state.source_colors[0].0, "100");
        assert_eq!(
            state
                .source_colors
                .iter()
                .filter(|(k, _)| k == "100")
                .count(),
            1
        );
    }

    #[test]
    fn outdated_extraction_drops_cached_colors_only() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("state.json");
        std::fs::write(
            &path,
            r##"{"source":{"kind":"color","color":"#010203"},"extraction_version":1,"source_colors":[["a","#ffffff"]]}"##,
        )
        .unwrap();
        let state = State::load(&path);
        assert!(state.cached_color("a").is_none());
        assert!(matches!(state.source, Source::Color { .. }));
    }

    #[test]
    fn history_is_most_recent_first_without_duplicates() {
        let mut state = State::default();
        for p in ["/a", "/b", "/a"] {
            state.record_wallpaper(Path::new(p));
        }
        let paths: Vec<_> = state
            .history
            .iter()
            .map(|e| e.path.to_str().unwrap())
            .collect();
        assert_eq!(paths, ["/a", "/b"]);
        for i in 0..MAX_HISTORY + 5 {
            state.record_wallpaper(Path::new(&format!("/{i}")));
        }
        assert_eq!(state.history.len(), MAX_HISTORY);
    }

    #[test]
    fn corrupt_state_starts_fresh() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("state.json");
        std::fs::write(&path, "{not json").unwrap();
        assert_eq!(State::load(&path).source, Source::Wallpaper);
    }
}
