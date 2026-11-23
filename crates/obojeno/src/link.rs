//! Adding the lines an app needs to read obojeno's output (an `include` in
//! kitty.conf, a variable in the shell profile) to its config, so people
//! do not have to. Each file is copied next to itself first.

use std::collections::BTreeMap;
use std::fs::OpenOptions;
use std::io::Write as _;
use std::path::{Path, PathBuf};

use anyhow::Context as _;
use obojeno_core::targets;

use crate::detect::{self, Places, Variable};

/// Lines to add to the end of a file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Plan {
    /// Created when missing.
    pub file: PathBuf,
    /// The lines, as they are also shown to copy.
    pub text: String,
    /// How a comment starts in the file.
    comment: &'static str,
}

/// Where a target's setup lines go on this Mac, and in which syntax. `None`
/// when obojeno cannot tell for sure or cannot write there, which leaves
/// the setup to the user.
pub fn plan(target: &str, places: &Places, options: &BTreeMap<String, String>) -> Option<Plan> {
    locate(target, places, options).filter(|plan| writable(&plan.file))
}

/// Whether the file, or the folder it would be created in, can be written.
/// Not so for configs home-manager links into the read-only Nix store,
/// which belong in the Nix configuration anyway.
fn writable(file: &Path) -> bool {
    file.ancestors()
        .find_map(|path| std::fs::metadata(path).ok())
        .is_some_and(|metadata| !metadata.permissions().readonly())
}

fn locate(target: &str, places: &Places, options: &BTreeMap<String, String>) -> Option<Plan> {
    let snippet = targets::builtin(target)?.snippet?;
    let plan = |file: PathBuf, comment, text: String| Plan {
        file,
        text,
        comment,
    };
    match target {
        "kitty" => Some(plan(
            places.home(".config/kitty/kitty.conf"),
            "#",
            snippet.to_owned(),
        )),
        "nvim" => {
            let config = places.home(".config/nvim");
            let vim = config.join("init.vim");
            // Neovim refuses to start with both, so Lua goes into init.vim
            // when that is the one there.
            Some(if !config.join("init.lua").exists() && vim.exists() {
                plan(vim, "\"", format!("lua << EOF\n{snippet}\nEOF"))
            } else {
                plan(config.join("init.lua"), "--", snippet.to_owned())
            })
        }
        "starship" | "herdr" => {
            // The snippets are `export NAME=value`.
            let (name, value) = snippet.strip_prefix("export ")?.split_once('=')?;
            // Pointing it elsewhere is a choice obojeno leaves alone.
            if detect::shell_variable(name, places) == Variable::Elsewhere {
                return None;
            }
            let (file, text) = match places.shell_name()? {
                // A ZDOTDIR set in .zshenv moves .zshrc where obojeno cannot
                // follow.
                "zsh"
                    if !std::fs::read_to_string(places.home(".zshenv"))
                        .unwrap_or_default()
                        .contains("ZDOTDIR") =>
                {
                    (places.home(".zshrc"), snippet.to_owned())
                }
                // Terminals on macOS start login shells, which read the first
                // of these.
                "bash" => (
                    [".bash_profile", ".bash_login", ".profile"]
                        .into_iter()
                        .map(|file| places.home(file))
                        .find(|file| file.exists())
                        .unwrap_or_else(|| places.home(".bash_profile")),
                    snippet.to_owned(),
                ),
                "fish" => (
                    places.home(".config/fish/config.fish"),
                    format!("set -gx {name} {value}"),
                ),
                _ => return None,
            };
            Some(plan(file, "#", text))
        }
        "zen" => {
            let profile = places.dirs.expand(options.get("profile")?);
            Some(plan(profile.join("user.js"), "//", snippet.to_owned()))
        }
        _ => None,
    }
}

/// What [`add`] did.
#[derive(Debug, PartialEq, Eq)]
pub enum Added {
    /// The file had the lines already and is left as it is.
    Already,
    /// The lines are in, after copying the file from before (`None` when
    /// there was no file).
    Now { backup: Option<PathBuf> },
}

/// Adds the plan's lines to the end of its file, after copying the file,
/// unless the file has them already.
pub fn add(plan: &Plan) -> anyhow::Result<Added> {
    let file = &plan.file;
    let existing = match std::fs::read(file) {
        Ok(bytes) => Some(bytes),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
        Err(e) => return Err(e).with_context(|| format!("reading {}", file.display())),
    };
    // Whatever told the app the setup was missing, never add it twice.
    if existing
        .as_deref()
        .is_some_and(|bytes| String::from_utf8_lossy(bytes).contains(&plan.text))
    {
        return Ok(Added::Already);
    }
    let backup = existing
        .as_deref()
        .map(|bytes| back_up(file, bytes))
        .transpose()?;
    if existing.is_none()
        && let Some(dir) = file.parent()
    {
        std::fs::create_dir_all(dir).with_context(|| format!("creating {}", dir.display()))?;
    }
    // A blank line between the file's last line and obojeno's.
    let separator = match existing.as_deref() {
        None | Some([]) => "",
        Some(bytes) if bytes.ends_with(b"\n") => "\n",
        Some(_) => "\n\n",
    };
    let text = format!(
        "{separator}{} Added by obojeno\n{}\n",
        plan.comment, plan.text
    );
    // Appending writes through a symlink into a dotfiles repository and
    // keeps the file's permissions.
    OpenOptions::new()
        .append(true)
        .create(true)
        .open(file)
        .and_then(|mut f| f.write_all(text.as_bytes()))
        .with_context(|| format!("adding to {}", file.display()))?;
    Ok(Added::Now { backup })
}

/// Writes `bytes` to `<file>.obojeno-backup`, or `-2`, `-3`, ... when an
/// earlier copy is there, with the file's permissions.
fn back_up(file: &Path, bytes: &[u8]) -> anyhow::Result<PathBuf> {
    let permissions = std::fs::metadata(file)
        .with_context(|| format!("reading {}", file.display()))?
        .permissions();
    for n in 1.. {
        let mut name = file.as_os_str().to_owned();
        name.push(".obojeno-backup");
        if n > 1 {
            name.push(format!("-{n}"));
        }
        let backup = PathBuf::from(name);
        match OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&backup)
        {
            Ok(mut f) => {
                f.write_all(bytes)
                    .and_then(|()| f.set_permissions(permissions))
                    .with_context(|| format!("writing {}", backup.display()))?;
                return Ok(backup);
            }
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {}
            Err(e) => return Err(e).with_context(|| format!("creating {}", backup.display())),
        }
    }
    unreachable!("there is always a free name")
}

#[cfg(test)]
mod tests {
    use super::*;
    use obojeno_core::output::Dirs;

    fn places(home: &Path, shell: &str) -> Places {
        Places {
            dirs: Dirs {
                home: home.to_owned(),
                cache: home.join(".cache/obojeno"),
                config: home.join(".config/obojeno"),
            },
            bin_dirs: vec![],
            shell: Some(PathBuf::from(shell)),
            nix_profiles: vec![],
        }
    }

    fn write(path: &Path, text: &str) {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, text).unwrap();
    }

    fn read(path: &Path) -> String {
        std::fs::read_to_string(path).unwrap()
    }

    #[test]
    fn adds_to_the_end_after_a_backup() {
        let home = tempfile::tempdir().unwrap();
        let conf = home.path().join(".config/kitty/kitty.conf");
        write(&conf, "font_size 13");
        let plan = plan("kitty", &places(home.path(), "/bin/zsh"), &BTreeMap::new()).unwrap();
        assert_eq!(plan.file, conf);

        let Added::Now {
            backup: Some(backup),
        } = add(&plan).unwrap()
        else {
            panic!("not added");
        };
        assert_eq!(
            backup,
            home.path().join(".config/kitty/kitty.conf.obojeno-backup")
        );
        assert_eq!(read(&backup), "font_size 13");
        assert_eq!(
            read(&conf),
            "font_size 13\n\n# Added by obojeno\ninclude ~/.cache/obojeno/kitty.conf\n"
        );
        assert!(detect::includes_obojeno(&read(&conf)));

        // Never twice, and the backup stays.
        let added = read(&conf);
        assert_eq!(add(&plan).unwrap(), Added::Already);
        assert_eq!(read(&conf), added);
        assert_eq!(read(&backup), "font_size 13");

        // With the lines gone again, an earlier backup is kept.
        write(&conf, "font_size 14\n");
        let Added::Now {
            backup: Some(again),
        } = add(&plan).unwrap()
        else {
            panic!("not added");
        };
        assert_eq!(again.file_name().unwrap(), "kitty.conf.obojeno-backup-2");
        assert_eq!(read(&backup), "font_size 13");
    }

    #[test]
    fn creates_a_missing_file_without_a_backup() {
        let home = tempfile::tempdir().unwrap();
        let plan = plan("nvim", &places(home.path(), "/bin/zsh"), &BTreeMap::new()).unwrap();
        assert_eq!(plan.file, home.path().join(".config/nvim/init.lua"));
        assert_eq!(add(&plan).unwrap(), Added::Now { backup: None });
        assert_eq!(
            read(&plan.file),
            format!("-- Added by obojeno\n{}\n", targets::NVIM_SNIPPET)
        );
    }

    #[test]
    fn keeps_permissions_and_writes_through_symlinks() {
        use std::os::unix::fs::PermissionsExt as _;
        let home = tempfile::tempdir().unwrap();
        let real = home.path().join("dotfiles/zshrc");
        write(&real, "alias g=git\n");
        std::fs::set_permissions(&real, std::fs::Permissions::from_mode(0o600)).unwrap();
        std::os::unix::fs::symlink(&real, home.path().join(".zshrc")).unwrap();

        let plan = plan(
            "starship",
            &places(home.path(), "/bin/zsh"),
            &BTreeMap::new(),
        )
        .unwrap();
        let Added::Now {
            backup: Some(backup),
        } = add(&plan).unwrap()
        else {
            panic!("not added");
        };
        let link = std::fs::symlink_metadata(home.path().join(".zshrc")).unwrap();
        assert!(link.file_type().is_symlink(), "still a link");
        assert_eq!(
            read(&real),
            "alias g=git\n\n# Added by obojeno\nexport STARSHIP_CONFIG=~/.cache/obojeno/starship.toml\n"
        );
        let mode = |path: &Path| std::fs::metadata(path).unwrap().permissions().mode() & 0o777;
        assert_eq!((mode(&real), mode(&backup)), (0o600, 0o600));
    }

    #[test]
    fn shell_variables_in_each_shell() {
        let home = tempfile::tempdir().unwrap();
        let file_and_text = |shell: &str| {
            plan("herdr", &places(home.path(), shell), &BTreeMap::new()).map(|plan| {
                (
                    plan.file.strip_prefix(home.path()).unwrap().to_owned(),
                    plan.text,
                )
            })
        };
        assert_eq!(
            file_and_text("/bin/zsh"),
            Some((
                PathBuf::from(".zshrc"),
                "export HERDR_CONFIG_PATH=~/.cache/obojeno/herdr.toml".to_owned()
            ))
        );
        assert_eq!(
            file_and_text("/opt/homebrew/bin/fish"),
            Some((
                PathBuf::from(".config/fish/config.fish"),
                "set -gx HERDR_CONFIG_PATH ~/.cache/obojeno/herdr.toml".to_owned()
            ))
        );
        assert_eq!(
            file_and_text("/bin/bash").unwrap().0,
            PathBuf::from(".bash_profile")
        );
        write(&home.path().join(".profile"), "");
        assert_eq!(
            file_and_text("/bin/bash").unwrap().0,
            PathBuf::from(".profile")
        );
        assert_eq!(file_and_text("/usr/local/bin/nu"), None, "unknown shell");

        write(
            &home.path().join(".zshenv"),
            "export ZDOTDIR=~/.config/zsh\n",
        );
        assert_eq!(file_and_text("/bin/zsh"), None, ".zshrc is elsewhere");
    }

    #[test]
    fn leaves_read_only_configs_alone() {
        use std::os::unix::fs::PermissionsExt as _;
        let home = tempfile::tempdir().unwrap();
        // Like home-manager: links into a read-only store.
        let store = home.path().join("store");
        write(&store.join("zshrc"), "alias g=git\n");
        std::fs::create_dir_all(store.join("nvim")).unwrap();
        std::fs::set_permissions(store.join("zshrc"), std::fs::Permissions::from_mode(0o444))
            .unwrap();
        std::fs::set_permissions(store.join("nvim"), std::fs::Permissions::from_mode(0o555))
            .unwrap();
        std::os::unix::fs::symlink(store.join("zshrc"), home.path().join(".zshrc")).unwrap();
        std::fs::create_dir_all(home.path().join(".config")).unwrap();
        std::os::unix::fs::symlink(store.join("nvim"), home.path().join(".config/nvim")).unwrap();

        let places = places(home.path(), "/bin/zsh");
        assert_eq!(plan("starship", &places, &BTreeMap::new()), None, "file");
        assert_eq!(plan("nvim", &places, &BTreeMap::new()), None, "folder");
        assert!(
            plan("kitty", &places, &BTreeMap::new()).is_some(),
            "writable"
        );
    }

    #[test]
    fn leaves_a_variable_pointing_elsewhere_alone() {
        let home = tempfile::tempdir().unwrap();
        write(
            &home.path().join(".zshrc"),
            "export STARSHIP_CONFIG=~/my/starship.toml\n",
        );
        assert_eq!(
            plan(
                "starship",
                &places(home.path(), "/bin/zsh"),
                &BTreeMap::new()
            ),
            None
        );
    }

    #[test]
    fn lua_goes_into_an_existing_init_vim() {
        let home = tempfile::tempdir().unwrap();
        write(&home.path().join(".config/nvim/init.vim"), "set number\n");
        let plan = plan("nvim", &places(home.path(), "/bin/zsh"), &BTreeMap::new()).unwrap();
        add(&plan).unwrap();
        let text = read(&home.path().join(".config/nvim/init.vim"));
        assert!(
            text.starts_with("set number\n\n\" Added by obojeno\nlua << EOF\nlocal colors"),
            "{text}"
        );
        assert!(text.ends_with("end))\nEOF\n"), "{text}");
    }

    #[test]
    fn zen_needs_its_profile() {
        let home = tempfile::tempdir().unwrap();
        let places = places(home.path(), "/bin/zsh");
        assert_eq!(plan("zen", &places, &BTreeMap::new()), None);
        let options = BTreeMap::from([("profile".to_owned(), "~/zen/p".to_owned())]);
        let plan = plan("zen", &places, &options).unwrap();
        assert_eq!(plan.file, home.path().join("zen/p/user.js"));
        assert!(plan.text.starts_with("user_pref("));
        assert_eq!(super::plan("obsidian", &places, &options), None);
    }
}
