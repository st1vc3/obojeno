//! Telling apps to pick up their new colors.

use std::ffi::OsString;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::mpsc;
use std::time::Duration;

use obojeno_core::ipc::Outcome;
use obojeno_core::targets::{Reload, Signal};

use crate::macos;

/// Reload commands talk to running apps; anything slower is stuck.
const TIMEOUT: Duration = Duration::from_secs(5);

/// Runs a target's reloads unless its guard process is missing.
pub fn reload_target(reloads: &[Reload], only_if_running: Option<&str>) -> Outcome {
    if let Some(process) = only_if_running {
        match macos::pids_named(process) {
            Ok(pids) if pids.is_empty() => return Outcome::NotRunning,
            Ok(_) => {}
            Err(e) => return Outcome::failed(e.to_string()),
        }
    }
    for reload in reloads {
        let outcome = match reload {
            Reload::Command(args) => command(args),
            Reload::Signal { process, signal } => send_signal(process, *signal),
        };
        if outcome != Outcome::Done {
            return outcome;
        }
    }
    Outcome::Done
}

fn send_signal(process: &str, signal: Signal) -> Outcome {
    let signal = match signal {
        Signal::Usr1 => libc::SIGUSR1,
    };
    let pids = match macos::pids_named(process) {
        Ok(pids) if pids.is_empty() => return Outcome::NotRunning,
        Ok(pids) => pids,
        Err(e) => return Outcome::failed(e.to_string()),
    };
    for pid in pids {
        // SAFETY: plain syscall; a pid that exited in the meantime just fails.
        if unsafe { libc::kill(pid, signal) } != 0 {
            let err = std::io::Error::last_os_error();
            if err.raw_os_error() != Some(libc::ESRCH) {
                return Outcome::failed(format!("signalling {process} ({pid}): {err}"));
            }
        }
    }
    Outcome::Done
}

fn command(args: &[String]) -> Outcome {
    let Some((program, rest)) = args.split_first() else {
        return Outcome::Done;
    };
    let path = search_path();
    // An absolute path lets std use posix_spawn. With a bare name and a
    // changed PATH it forks instead, and forking while other threads are in
    // CoreFoundation (gallery indexing, AppKit) can kill the child before
    // it execs.
    let Some(executable) = resolve(program, &path) else {
        return Outcome::NotInstalled {
            command: program.clone(),
        };
    };
    let mut child = match Command::new(executable)
        .args(rest)
        .env("PATH", path)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
    {
        Ok(child) => child,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            return Outcome::NotInstalled {
                command: program.clone(),
            };
        }
        Err(e) => return Outcome::failed(format!("{program}: {e}")),
    };
    let pid = child.id();
    let mut stderr = child.stderr.take().expect("stderr is piped");
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        let mut output = String::new();
        let _ = stderr.read_to_string(&mut output);
        let _ = tx.send((child.wait(), output));
    });
    match rx.recv_timeout(TIMEOUT) {
        Ok((Ok(status), _)) if status.success() => Outcome::Done,
        Ok((Ok(status), stderr)) => {
            let detail = stderr.trim();
            Outcome::failed(if detail.is_empty() {
                format!("{program} exited with {status}")
            } else {
                format!("{program}: {detail}")
            })
        }
        Ok((Err(e), _)) => Outcome::failed(format!("{program}: {e}")),
        Err(_) => {
            // SAFETY: plain syscall on the child we spawned.
            unsafe { libc::kill(i32::try_from(pid).unwrap_or(0), libc::SIGKILL) };
            Outcome::failed(format!(
                "{program} did not finish within {}s",
                TIMEOUT.as_secs()
            ))
        }
    }
}

/// Finds `program` like a shell would, in `path`.
fn resolve(program: &str, path: &OsString) -> Option<PathBuf> {
    use std::os::unix::fs::PermissionsExt;
    let executable = |p: &Path| {
        std::fs::metadata(p).is_ok_and(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
    };
    if program.contains('/') {
        let p = PathBuf::from(program);
        return executable(&p).then_some(p);
    }
    std::env::split_paths(path)
        .map(|dir| dir.join(program))
        .find(|p| executable(p))
}

/// `PATH` plus the places package managers install to. Under launchd, `PATH`
/// is only the system folders, so Homebrew and Nix tools would be missing.
fn search_path() -> OsString {
    let home = std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_default();
    let user = std::env::var("USER").unwrap_or_default();
    let mut dirs: Vec<PathBuf> = std::env::var_os("PATH")
        .map(|p| std::env::split_paths(&p).collect())
        .unwrap_or_default();
    for extra in [
        PathBuf::from("/opt/homebrew/bin"),
        PathBuf::from("/usr/local/bin"),
        home.join(".nix-profile/bin"),
        PathBuf::from(format!("/etc/profiles/per-user/{user}/bin")),
        PathBuf::from("/run/current-system/sw/bin"),
        PathBuf::from("/nix/var/nix/profiles/default/bin"),
        PathBuf::from("/usr/bin"),
        PathBuf::from("/bin"),
        PathBuf::from("/usr/sbin"),
        PathBuf::from("/sbin"),
    ] {
        if !dirs.contains(&extra) {
            dirs.push(extra);
        }
    }
    std::env::join_paths(dirs).unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cmd(args: &[&str]) -> Reload {
        Reload::Command(args.iter().map(|&s| s.to_owned()).collect())
    }

    #[test]
    fn command_outcomes() {
        assert_eq!(reload_target(&[cmd(&["true"])], None), Outcome::Done);
        assert_eq!(
            reload_target(&[cmd(&["obojeno-no-such-command"])], None),
            Outcome::NotInstalled {
                command: "obojeno-no-such-command".to_owned()
            }
        );
        let Outcome::Failed { message: msg } =
            reload_target(&[cmd(&["sh", "-c", "echo oops >&2; exit 3"])], None)
        else {
            panic!("expected failure");
        };
        assert_eq!(msg, "sh: oops");
    }

    #[test]
    fn stops_at_first_failure() {
        let dir = tempfile::tempdir().unwrap();
        let marker = dir.path().join("ran");
        let outcome = reload_target(
            &[cmd(&["false"]), cmd(&["touch", marker.to_str().unwrap()])],
            None,
        );
        assert!(matches!(outcome, Outcome::Failed { .. }));
        assert!(!marker.exists());
    }

    #[test]
    fn guard_skips_when_process_is_missing() {
        let outcome = reload_target(&[cmd(&["false"])], Some("surely-no-process-has-this-name"));
        assert_eq!(outcome, Outcome::NotRunning);
    }

    #[test]
    fn signal_to_missing_process_is_not_running() {
        let outcome = reload_target(
            &[Reload::Signal {
                process: "surely-no-process-has-this-name".to_owned(),
                signal: Signal::Usr1,
            }],
            None,
        );
        assert_eq!(outcome, Outcome::NotRunning);
    }

    #[test]
    fn resolves_programs_on_the_path() {
        let path = search_path();
        let sh = resolve("sh", &path).unwrap();
        assert!(sh.is_absolute() && sh.ends_with("sh"), "{}", sh.display());
        assert_eq!(resolve("/bin/sh", &path), Some(PathBuf::from("/bin/sh")));
        assert_eq!(resolve("obojeno-no-such-command", &path), None);
        // Folders and non-executable files are not programs.
        assert_eq!(resolve("/tmp", &path), None);
        assert_eq!(resolve("/etc/hosts", &path), None);
    }

    #[test]
    fn search_path_includes_package_managers() {
        let path = search_path();
        let dirs: Vec<_> = std::env::split_paths(&path).collect();
        assert!(dirs.contains(&PathBuf::from("/opt/homebrew/bin")));
        assert!(dirs.contains(&PathBuf::from("/run/current-system/sw/bin")));
    }
}
