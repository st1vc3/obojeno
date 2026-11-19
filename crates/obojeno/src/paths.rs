//! Where obojeno keeps its files. Follows the XDG variables when set, like
//! the other command-line tools it themes.

use std::path::{Path, PathBuf};

use anyhow::Context as _;
use obojeno_core::output::Dirs;

#[derive(Debug, Clone)]
pub struct Paths {
    pub dirs: Dirs,
    pub config_file: PathBuf,
    /// `~/.local/state/obojeno`: what was applied last, color cache.
    pub state: PathBuf,
}

impl Paths {
    pub fn from_env(config_override: Option<&Path>) -> anyhow::Result<Self> {
        let home = PathBuf::from(std::env::var_os("HOME").context("HOME is not set")?);
        let xdg = |var: &str, fallback: &str| {
            std::env::var_os(var)
                .map(PathBuf::from)
                .filter(|p| p.is_absolute())
                .unwrap_or_else(|| home.join(fallback))
                .join("obojeno")
        };
        let config = xdg("XDG_CONFIG_HOME", ".config");
        Ok(Self {
            config_file: config_override.map_or_else(|| config.join("config.toml"), Path::to_owned),
            state: xdg("XDG_STATE_HOME", ".local/state"),
            dirs: Dirs {
                cache: xdg("XDG_CACHE_HOME", ".cache"),
                config,
                home,
            },
        })
    }

    pub fn state_file(&self) -> PathBuf {
        self.state.join("state.json")
    }
}
