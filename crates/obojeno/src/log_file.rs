//! Keeping the daemon's log bounded. Whatever starts the daemon (the menu
//! bar app, launchd) sends its output to `~/Library/Logs/obojeno.log` and
//! never trims it; the daemon moves it aside to `obojeno.log.1` once it
//! grows past [`MAX_SIZE`] and carries on in a fresh file.

use std::fs::{File, OpenOptions};
use std::io::Write;
use std::os::fd::{AsFd as _, AsRawFd as _, BorrowedFd};
use std::os::unix::fs::{MetadataExt as _, OpenOptionsExt as _};
use std::path::{Path, PathBuf};
use std::sync::Mutex;

/// The log is moved aside beyond this size.
pub const MAX_SIZE: u64 = 5 << 20;

/// The log's place, where `obojeno service` and the app point the daemon's
/// output.
pub fn path(home: &Path) -> PathBuf {
    home.join("Library/Logs/obojeno.log")
}

struct Rotation {
    path: PathBuf,
    /// The standard streams writing to the log: stdout, stderr or both.
    fds: Vec<i32>,
    size: u64,
    max_size: u64,
}

/// Set while the output goes to the log; `None` otherwise, as in a terminal.
static ROTATION: Mutex<Option<Rotation>> = Mutex::new(None);

/// Rotates the log at `path` from now on, if stdout or stderr write to it.
pub fn keep_bounded(path: &Path) {
    keep_bounded_at(path, MAX_SIZE);
}

fn keep_bounded_at(path: &Path, max_size: u64) {
    let Ok(log) = std::fs::metadata(path) else {
        return;
    };
    let is_log = |fd: BorrowedFd<'_>| {
        fd.try_clone_to_owned()
            .map(File::from)
            .and_then(|f| f.metadata())
            .is_ok_and(|m| m.dev() == log.dev() && m.ino() == log.ino())
    };
    let fds: Vec<i32> = [std::io::stdout().as_fd(), std::io::stderr().as_fd()]
        .into_iter()
        .filter(|&fd| is_log(fd))
        .map(|fd| fd.as_raw_fd())
        .collect();
    if fds.is_empty() {
        return;
    }
    *ROTATION
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(Rotation {
        path: path.to_owned(),
        fds,
        size: log.len(),
        max_size,
    });
}

/// The writer for log lines: stderr, counting what goes to the log and
/// rotating it after a line that fills it.
pub fn writer() -> Writer {
    Writer { written: 0 }
}

pub struct Writer {
    written: u64,
}

impl Write for Writer {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        let n = std::io::stderr().write(buf)?;
        self.written += n as u64;
        Ok(n)
    }

    fn flush(&mut self) -> std::io::Result<()> {
        std::io::stderr().flush()
    }
}

impl Drop for Writer {
    /// After the whole line, so none is split between files.
    fn drop(&mut self) {
        let mut rotation = ROTATION
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let Some(r) = rotation.as_mut() else {
            return;
        };
        r.size += self.written;
        if r.size < r.max_size {
            return;
        }
        match rotate(r) {
            Ok(()) => r.size = 0,
            Err(e) => {
                // Not again for every line; and stderr is the log itself.
                *rotation = None;
                eprintln!("obojeno: cannot rotate the log, leaving it as it is: {e}");
            }
        }
    }
}

/// Moves the log to `<log>.1` and points the streams at a fresh one.
fn rotate(r: &Rotation) -> std::io::Result<()> {
    let mut old = r.path.clone().into_os_string();
    old.push(".1");
    std::fs::rename(&r.path, old)?;
    let fresh = OpenOptions::new()
        .append(true)
        .create(true)
        .mode(0o644)
        .open(&r.path)?;
    for &fd in &r.fds {
        // SAFETY: both are open descriptors; dup2 replaces `fd` atomically.
        if unsafe { libc::dup2(fresh.as_raw_fd(), fd) } < 0 {
            return Err(std::io::Error::last_os_error());
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Runs the test binary itself with its stderr in a log file: rotating
    /// replaces the process's stderr, which a test must not do to the
    /// runner.
    #[test]
    fn the_log_moves_aside_when_full() {
        if let Some(log) = std::env::var_os("OBOJENO_TEST_LOG") {
            let log = PathBuf::from(log);
            keep_bounded_at(&log, 100);
            for i in 0..30 {
                writeln!(writer(), "line {i}").unwrap();
            }
            return;
        }
        let tmp = tempfile::tempdir().unwrap();
        let log = tmp.path().join("obojeno.log");
        let file = File::create(&log).unwrap();
        let status = std::process::Command::new(std::env::current_exe().unwrap())
            .args(["--exact", "log_file::tests::the_log_moves_aside_when_full"])
            .env("OBOJENO_TEST_LOG", &log)
            .stderr(file)
            .stdout(std::process::Stdio::null())
            .status()
            .unwrap();
        assert!(status.success());

        // Lines of 7 bytes, then 8 from line 10: past 100 bytes after
        // line 13, then again after line 26.
        let old = std::fs::read_to_string(tmp.path().join("obojeno.log.1")).unwrap();
        let new = std::fs::read_to_string(&log).unwrap();
        assert!(
            old.starts_with("line 14\n") && old.ends_with("line 26\n"),
            "{old:?}"
        );
        assert_eq!(new, "line 27\nline 28\nline 29\n");
    }

    #[test]
    fn other_outputs_are_left_alone() {
        let tmp = tempfile::tempdir().unwrap();
        let log = tmp.path().join("obojeno.log");
        std::fs::write(&log, "").unwrap();
        keep_bounded_at(&log, 100);
        assert!(
            ROTATION.lock().unwrap().is_none(),
            "the test's output is not the log"
        );
    }
}
