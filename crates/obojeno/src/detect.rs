//! Finding the apps obojeno can theme, so first-run setup can offer the ones
//! installed, and whether their one-time setup is done.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use obojeno_core::output::Dirs;

use crate::programs;

/// What was found for one target.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Found {
    pub installed: bool,
    /// Whether the app reads obojeno's output, when that can be checked.
    pub set_up: Option<bool>,
    /// Values for options the target needs (an Obsidian vault, a Zen profile).
    pub options: BTreeMap<String, String>,
}

/// Where to look: the home folder and the folders with commands.
pub struct Places {
    pub dirs: Dirs,
    pub bin_dirs: Vec<PathBuf>,
}

impl Places {
    pub fn of_this_user(dirs: &Dirs) -> Self {
        Self {
            dirs: dirs.clone(),
            bin_dirs: programs::search_dirs(&dirs.home),
        }
    }

    fn command(&self, name: &str) -> bool {
        programs::find(name, &self.bin_dirs).is_some()
    }

    /// An app bundle in /Applications or ~/Applications.
    fn app(&self, names: &[&str]) -> bool {
        names.iter().any(|name| {
            Path::new("/Applications").join(name).exists()
                || self.dirs.home.join("Applications").join(name).exists()
        })
    }

    fn home(&self, path: &str) -> PathBuf {
        self.dirs.home.join(path)
    }
}

pub fn find(target: &str, places: &Places) -> Found {
    let mut found = Found::default();
    match target {
        "kitty" => {
            found.installed = places.app(&["kitty.app"])
                || places.command("kitty")
                || places.home(".config/kitty").is_dir();
            let conf = places.home(".config/kitty/kitty.conf");
            found.set_up =
                Some(std::fs::read_to_string(conf).is_ok_and(|text| includes_obojeno(&text)));
        }
        "borders" => found.installed = places.command("borders"),
        "simple-bar" => {
            found.installed = places
                .home("Library/Application Support/Übersicht/widgets/simple-bar")
                .exists();
        }
        "nvim" => {
            let config = places.home(".config/nvim");
            found.installed = places.command("nvim") || config.is_dir();
            found.set_up = Some(mentions_obojeno(&config, 4));
        }
        "starship" => {
            found.installed =
                places.command("starship") || places.home(".config/starship.toml").exists();
        }
        "herdr" => {
            found.installed = places.command("herdr") || places.home(".config/herdr").is_dir();
        }
        "obsidian" => {
            let vaults = places.home("Library/Application Support/obsidian/obsidian.json");
            found.installed = places.app(&["Obsidian.app"]) || vaults.exists();
            if let Some(vault) = std::fs::read_to_string(vaults)
                .ok()
                .and_then(|json| obsidian_vault(&json))
            {
                found
                    .options
                    .insert("vault".to_owned(), places.dirs.abbreviate(&vault));
            }
        }
        "zen" => {
            let root = places.home("Library/Application Support/zen");
            found.installed = places.app(&["Zen.app", "Zen Browser.app"]) || root.is_dir();
            if let Some(profile) = std::fs::read_to_string(root.join("profiles.ini"))
                .ok()
                .and_then(|ini| zen_profile(&ini, &root))
            {
                found
                    .options
                    .insert("profile".to_owned(), places.dirs.abbreviate(&profile));
                let prefs = std::fs::read_to_string(profile.join("prefs.js")).unwrap_or_default()
                    + &std::fs::read_to_string(profile.join("user.js")).unwrap_or_default();
                found.set_up = Some(
                    prefs
                        .contains(r#""toolkit.legacyUserProfileCustomizations.stylesheets", true"#),
                );
            }
        }
        _ => {}
    }
    found
}

/// An uncommented `include` of obojeno's kitty.conf.
fn includes_obojeno(kitty_conf: &str) -> bool {
    kitty_conf.lines().any(|line| {
        let line = line.trim();
        line.starts_with("include") && line.contains("obojeno/kitty.conf")
    })
}

/// Whether any Lua file in the Neovim config loads the obojeno colorscheme.
fn mentions_obojeno(dir: &Path, depth: usize) -> bool {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return false;
    };
    entries.flatten().any(|entry| {
        let path = entry.path();
        if path.is_dir() {
            depth > 0 && mentions_obojeno(&path, depth - 1)
        } else {
            path.extension().is_some_and(|e| e == "lua" || e == "vim")
                && std::fs::read_to_string(&path).is_ok_and(|text| text.contains("obojeno"))
        }
    })
}

/// The open vault, or else the one used last, from Obsidian's own list.
fn obsidian_vault(json: &str) -> Option<PathBuf> {
    let value: serde_json::Value = serde_json::from_str(json).ok()?;
    let vaults = value.get("vaults")?.as_object()?;
    let vault = vaults.values().max_by_key(|v| {
        (
            v.get("open")
                .and_then(serde_json::Value::as_bool)
                .unwrap_or(false),
            v.get("ts").and_then(serde_json::Value::as_u64).unwrap_or(0),
        )
    })?;
    vault.get("path")?.as_str().map(PathBuf::from)
}

/// The default profile from Zen's profiles.ini: the one an install section
/// names, or else the one marked `Default=1`.
fn zen_profile(ini: &str, root: &Path) -> Option<PathBuf> {
    let mut sections: Vec<(String, BTreeMap<String, String>)> = Vec::new();
    for line in ini.lines().map(str::trim) {
        if let Some(name) = line.strip_prefix('[').and_then(|l| l.strip_suffix(']')) {
            sections.push((name.to_owned(), BTreeMap::new()));
        } else if let (Some((key, value)), Some((_, fields))) =
            (line.split_once('='), sections.last_mut())
        {
            fields.insert(key.trim().to_owned(), value.trim().to_owned());
        }
    }
    let resolve = |fields: &BTreeMap<String, String>, path: &str| {
        let relative = fields.get("IsRelative").is_none_or(|r| r == "1");
        if relative {
            root.join(path)
        } else {
            PathBuf::from(path)
        }
    };
    let installed = sections
        .iter()
        .filter(|(name, _)| name.starts_with("Install"))
        .find_map(|(_, fields)| fields.get("Default"));
    if let Some(path) = installed {
        // Install sections hold relative paths.
        return Some(root.join(path));
    }
    sections
        .iter()
        .filter(|(name, _)| name.starts_with("Profile"))
        .find(|(_, fields)| fields.get("Default").is_some_and(|d| d == "1"))
        .and_then(|(_, fields)| fields.get("Path").map(|path| resolve(fields, path)))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn places(home: &Path) -> Places {
        Places {
            dirs: Dirs {
                home: home.to_owned(),
                cache: home.join(".cache/obojeno"),
                config: home.join(".config/obojeno"),
            },
            bin_dirs: vec![home.join("bin")],
        }
    }

    fn write(path: &Path, text: &str) {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, text).unwrap();
    }

    #[test]
    fn finds_commands_and_config_folders() {
        let home = tempfile::tempdir().unwrap();
        let places = places(home.path());
        assert!(!find("borders", &places).installed);
        let borders = home.path().join("bin/borders");
        write(&borders, "");
        assert!(!find("borders", &places).installed, "not executable");
        let executable = std::os::unix::fs::PermissionsExt::from_mode(0o755);
        std::fs::set_permissions(&borders, executable).unwrap();
        assert!(find("borders", &places).installed);
        write(&home.path().join(".config/herdr/config.toml"), "");
        assert!(find("herdr", &places).installed);
    }

    #[test]
    fn kitty_is_set_up_with_an_uncommented_include() {
        assert!(includes_obojeno(
            "font_size 13\ninclude ~/.cache/obojeno/kitty.conf\n"
        ));
        assert!(!includes_obojeno("# include ~/.cache/obojeno/kitty.conf\n"));
        assert!(!includes_obojeno("include theme.conf\n"));

        let home = tempfile::tempdir().unwrap();
        write(
            &home.path().join(".config/kitty/kitty.conf"),
            "include ~/.cache/obojeno/kitty.conf\n",
        );
        let found = find("kitty", &places(home.path()));
        assert_eq!((found.installed, found.set_up), (true, Some(true)));
    }

    #[test]
    fn nvim_is_set_up_when_a_lua_file_loads_obojeno() {
        let home = tempfile::tempdir().unwrap();
        let lua = home.path().join(".config/nvim/lua/plugins/colors.lua");
        write(&lua, "vim.cmd.colorscheme('habamax')");
        assert_eq!(find("nvim", &places(home.path())).set_up, Some(false));
        write(&lua, "vim.cmd.colorscheme('obojeno')");
        assert_eq!(find("nvim", &places(home.path())).set_up, Some(true));
    }

    #[test]
    fn obsidian_prefers_the_open_vault_then_the_latest() {
        let json = r#"{"vaults":{
            "a":{"path":"/Users/me/Old","ts":300},
            "b":{"path":"/Users/me/Notes","ts":100,"open":true},
            "c":{"path":"/Users/me/Work","ts":200}}}"#;
        assert_eq!(obsidian_vault(json), Some(PathBuf::from("/Users/me/Notes")));
        let closed = r#"{"vaults":{"a":{"path":"/Old","ts":1},"c":{"path":"/Work","ts":2}}}"#;
        assert_eq!(obsidian_vault(closed), Some(PathBuf::from("/Work")));
        assert_eq!(obsidian_vault("{}"), None);
    }

    #[test]
    fn zen_profile_from_the_install_section_or_the_default() {
        let root = Path::new("/zen");
        let install = "[Profile0]\nName=default\nIsRelative=1\nPath=Profiles/old.default\nDefault=1\n\n\
                       [Install6ED35B3CA1B2BB2A]\nDefault=Profiles/abc.Default (release)\nLocked=1\n";
        assert_eq!(
            zen_profile(install, root),
            Some(PathBuf::from("/zen/Profiles/abc.Default (release)"))
        );
        let plain = "[Profile0]\nName=a\nIsRelative=1\nPath=Profiles/a\n\n\
                     [Profile1]\nName=b\nIsRelative=0\nPath=/elsewhere/b\nDefault=1\n";
        assert_eq!(
            zen_profile(plain, root),
            Some(PathBuf::from("/elsewhere/b"))
        );
        assert_eq!(zen_profile("[General]\nVersion=2\n", root), None);
    }

    #[test]
    fn zen_is_found_with_its_profile_and_setting() {
        let home = tempfile::tempdir().unwrap();
        let root = home.path().join("Library/Application Support/zen");
        write(
            &root.join("profiles.ini"),
            "[Profile0]\nPath=Profiles/x\nDefault=1\n",
        );
        write(
            &root.join("Profiles/x/prefs.js"),
            r#"user_pref("toolkit.legacyUserProfileCustomizations.stylesheets", true);"#,
        );
        let found = find("zen", &places(home.path()));
        assert!(found.installed);
        assert_eq!(
            found.options["profile"],
            "~/Library/Application Support/zen/Profiles/x"
        );
        assert_eq!(found.set_up, Some(true));
    }
}
