//! `obojeno service`: running the daemon at login with launchd, for installs
//! without a home-manager module (which writes the same agent itself).

use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{Context as _, bail};

pub const LABEL: &str = "io.github.st1vc3.obojeno";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Status {
    NotInstalled,
    /// Installed by `obojeno service install`.
    Installed {
        running: bool,
    },
    /// The agent belongs to home-manager (its program is in the Nix store);
    /// obojeno leaves it alone.
    Managed {
        running: bool,
    },
}

fn plist_path() -> anyhow::Result<PathBuf> {
    let home = std::env::var_os("HOME").context("HOME is not set")?;
    Ok(PathBuf::from(home).join(format!("Library/LaunchAgents/{LABEL}.plist")))
}

fn log_path() -> anyhow::Result<PathBuf> {
    let home = std::env::var_os("HOME").context("HOME is not set")?;
    Ok(crate::log_file::path(Path::new(&home)))
}

fn domain() -> String {
    // SAFETY: getuid cannot fail.
    format!("gui/{}", unsafe { libc::getuid() })
}

pub fn status() -> anyhow::Result<Status> {
    let path = plist_path()?;
    let Ok(meta) = std::fs::symlink_metadata(&path) else {
        return Ok(Status::NotInstalled);
    };
    let running = Command::new("/bin/launchctl")
        .args(["print", &format!("{}/{LABEL}", domain())])
        .output()
        .is_ok_and(|o| {
            o.status.success() && String::from_utf8_lossy(&o.stdout).contains("state = running")
        });
    // home-manager copies its agents (launchd ignores some symlinks), so
    // recognize them by their program living in the Nix store.
    let managed = meta.file_type().is_symlink()
        || std::fs::read_to_string(&path).is_ok_and(|plist| plist.contains("/nix/store/"));
    Ok(if managed {
        Status::Managed { running }
    } else {
        Status::Installed { running }
    })
}

/// Writes the agent for this executable and starts it.
pub fn install() -> anyhow::Result<()> {
    if let Status::Managed { .. } = status()? {
        bail!("the login agent is managed elsewhere (home-manager); change it there");
    }
    let exe = std::env::current_exe()
        .and_then(|p| p.canonicalize())
        .context("finding the obojeno executable")?;
    let path = plist_path()?;
    let log = log_path()?;
    std::fs::create_dir_all(path.parent().expect("plist has a folder"))?;
    if let Some(dir) = log.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let plist = agent_plist(&exe.to_string_lossy(), &log.to_string_lossy());
    // Replace a running agent, so a new executable takes over.
    bootout();
    obojeno_core::output::write_if_changed(&path, plist.as_bytes())
        .with_context(|| format!("writing {}", path.display()))?;
    launchctl(&["bootstrap", &domain(), &path.to_string_lossy()])
}

pub fn uninstall() -> anyhow::Result<()> {
    match status()? {
        Status::NotInstalled => Ok(()),
        Status::Managed { .. } => {
            bail!("the login agent is managed elsewhere (home-manager); change it there")
        }
        Status::Installed { .. } => {
            bootout();
            let path = plist_path()?;
            std::fs::remove_file(&path).with_context(|| format!("removing {}", path.display()))
        }
    }
}

fn bootout() {
    // Fails when the agent is not loaded, which is fine.
    let _ = Command::new("/bin/launchctl")
        .args(["bootout", &format!("{}/{LABEL}", domain())])
        .output();
}

fn launchctl(args: &[&str]) -> anyhow::Result<()> {
    let output = Command::new("/bin/launchctl")
        .args(args)
        .output()
        .context("running launchctl")?;
    if !output.status.success() {
        bail!(
            "launchctl {} failed: {}",
            args.join(" "),
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
    Ok(())
}

fn xml_escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

/// The agent: start at login, restart after crashes but not after a clean
/// exit (another daemon already running), run with interactive priority so
/// theming keeps up with the wallpaper.
fn agent_plist(exe: &str, log: &str) -> String {
    let (exe, log) = (xml_escape(exe), xml_escape(log));
    format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>Label</key>
  <string>{LABEL}</string>
  <key>ProgramArguments</key>
  <array>
    <string>{exe}</string>
    <string>daemon</string>
  </array>
  <key>RunAtLoad</key>
  <true/>
  <key>KeepAlive</key>
  <dict>
    <key>SuccessfulExit</key>
    <false/>
  </dict>
  <key>ProcessType</key>
  <string>Interactive</string>
  <key>StandardOutPath</key>
  <string>{log}</string>
  <key>StandardErrorPath</key>
  <string>{log}</string>
</dict>
</plist>
"#
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plist_is_valid_and_escaped() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("agent.plist");
        std::fs::write(&path, agent_plist("/Apps/A&B.app/obojeno", "/logs/o.log")).unwrap();
        let output = Command::new("/usr/bin/plutil")
            .arg("-lint")
            .arg(&path)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stdout)
        );
        let json = Command::new("/usr/bin/plutil")
            .args(["-convert", "json", "-o", "-"])
            .arg(&path)
            .output()
            .unwrap();
        let value: serde_json::Value = serde_json::from_slice(&json.stdout).unwrap();
        assert_eq!(value["ProgramArguments"][0], "/Apps/A&B.app/obojeno");
        assert_eq!(value["KeepAlive"]["SuccessfulExit"], false);
        assert_eq!(value["Label"], LABEL);
    }
}
