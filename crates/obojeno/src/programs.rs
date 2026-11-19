//! Finding commands the way a login shell would, also under launchd and
//! when started by the app, where `PATH` only has the system folders and
//! Homebrew and Nix tools would be missing.

use std::ffi::OsString;
use std::path::{Path, PathBuf};

/// `PATH`, then the places package managers install commands to.
pub fn search_dirs(home: &Path) -> Vec<PathBuf> {
    let user = std::env::var("USER").unwrap_or_default();
    let path: Vec<PathBuf> = std::env::var_os("PATH")
        .map(|path| std::env::split_paths(&path).collect())
        .unwrap_or_default();
    let mut dirs: Vec<PathBuf> = Vec::new();
    for dir in path.into_iter().chain([
        PathBuf::from("/opt/homebrew/bin"),
        PathBuf::from("/usr/local/bin"),
        home.join(".nix-profile/bin"),
        PathBuf::from(format!("/etc/profiles/per-user/{user}/bin")),
        PathBuf::from("/run/current-system/sw/bin"),
        PathBuf::from("/nix/var/nix/profiles/default/bin"),
        home.join(".local/bin"),
        home.join(".cargo/bin"),
        PathBuf::from("/usr/bin"),
        PathBuf::from("/bin"),
        PathBuf::from("/usr/sbin"),
        PathBuf::from("/sbin"),
    ]) {
        // Shells search the first copy of a folder; later ones only cost.
        if !dirs.contains(&dir) {
            dirs.push(dir);
        }
    }
    dirs
}

/// The folders as a `PATH` for child processes.
pub fn path_variable(dirs: &[PathBuf]) -> OsString {
    std::env::join_paths(dirs).unwrap_or_default()
}

/// Finds the executable `program` in `dirs`, like a shell would; a program
/// with a slash is only checked.
pub fn find(program: &str, dirs: &[PathBuf]) -> Option<PathBuf> {
    if program.contains('/') {
        let path = PathBuf::from(program);
        return is_executable(&path).then_some(path);
    }
    dirs.iter()
        .map(|dir| dir.join(program))
        .find(|path| is_executable(path))
}

fn is_executable(path: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    std::fs::metadata(path).is_ok_and(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_programs_like_a_shell() {
        let dirs = search_dirs(Path::new("/nowhere"));
        let sh = find("sh", &dirs).unwrap();
        assert!(sh.is_absolute() && sh.ends_with("sh"), "{}", sh.display());
        assert_eq!(find("/bin/sh", &dirs), Some(PathBuf::from("/bin/sh")));
        assert_eq!(find("obojeno-no-such-command", &dirs), None);
        // Folders and files that cannot run are not programs.
        assert_eq!(find("/tmp", &dirs), None);
        assert_eq!(find("/etc/hosts", &dirs), None);
    }

    #[test]
    fn searches_where_package_managers_install() {
        let dirs = search_dirs(Path::new("/Users/me"));
        assert!(dirs.contains(&PathBuf::from("/opt/homebrew/bin")));
        assert!(dirs.contains(&PathBuf::from("/run/current-system/sw/bin")));
        assert!(dirs.contains(&PathBuf::from("/Users/me/.nix-profile/bin")));
        let unique: std::collections::BTreeSet<_> = dirs.iter().collect();
        assert_eq!(unique.len(), dirs.len(), "no folder twice");
    }
}
