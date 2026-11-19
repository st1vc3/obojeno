//! Messages between the daemon and its clients (the CLI and the menu bar app).
//!
//! One connection per request: the client writes a [`Request`] as one line of
//! JSON and reads one [`Reply`] line back. A `subscribe` connection stays open
//! and receives a `state` reply now and after every change.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::config::TransitionStyle;
use crate::{Color, Mode, Palette};

pub const SOCKET_NAME: &str = "obojeno.sock";

pub fn socket_path(state_dir: &Path) -> PathBuf {
    state_dir.join(SOCKET_NAME)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ModeChange {
    Light,
    Dark,
    Toggle,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "cmd", rename_all = "snake_case")]
pub enum Request {
    /// The current state now and after every change, on this connection.
    Subscribe,
    State,
    /// Re-theme from the current source.
    Apply {
        #[serde(default)]
        mode: Option<Mode>,
    },
    SetWallpaper {
        path: PathBuf,
        #[serde(default)]
        transition: Option<TransitionStyle>,
        /// Seconds.
        #[serde(default)]
        duration: Option<f64>,
        /// Where grow-circle starts, in global screen points with the origin
        /// at the bottom left. Defaults to the mouse pointer.
        #[serde(default)]
        origin: Option<[f64; 2]>,
    },
    SetMode {
        mode: ModeChange,
    },
    /// Theme from a fixed color, or follow the wallpaper again with `null`.
    SetColor {
        color: Option<Color>,
    },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "reply", rename_all = "snake_case")]
pub enum Reply {
    Report(Report),
    State(State),
    Error { message: String },
}

/// What the daemon shows: the theme on screen right now.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct State {
    pub wallpaper: Option<PathBuf>,
    /// A color fixed with `obojeno color`, or `None` when following the
    /// wallpaper.
    pub fixed_color: Option<Color>,
    pub mode: Mode,
    pub palette: Palette,
    pub transition: TransitionStyle,
    /// Seconds.
    pub duration: f64,
}

/// What happened when theming.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Report {
    pub source: Color,
    pub mode: Mode,
    pub wallpaper: Option<PathBuf>,
    pub targets: Vec<TargetReport>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TargetReport {
    pub name: String,
    /// Whether the target has files at all (borders only reloads).
    pub has_files: bool,
    pub written: Vec<PathBuf>,
    pub reload: Option<Outcome>,
    pub error: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "outcome", rename_all = "snake_case")]
pub enum Outcome {
    Done,
    /// The app is not running, so there was nothing to reload.
    NotRunning,
    /// The command does not exist on this machine.
    NotInstalled {
        command: String,
    },
    Failed {
        message: String,
    },
}

impl Outcome {
    pub fn failed(message: impl Into<String>) -> Self {
        Self::Failed {
            message: message.into(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn requests_are_flat_json() {
        let r: Request =
            serde_json::from_str(r#"{"cmd":"set_wallpaper","path":"/w.heic"}"#).unwrap();
        assert_eq!(
            r,
            Request::SetWallpaper {
                path: "/w.heic".into(),
                transition: None,
                duration: None,
                origin: None
            }
        );
        let r: Request = serde_json::from_str(r#"{"cmd":"set_mode","mode":"toggle"}"#).unwrap();
        assert_eq!(
            r,
            Request::SetMode {
                mode: ModeChange::Toggle
            }
        );
        let r: Request = serde_json::from_str(r#"{"cmd":"set_color","color":null}"#).unwrap();
        assert_eq!(r, Request::SetColor { color: None });
        assert_eq!(
            serde_json::to_string(&Request::Subscribe).unwrap(),
            r#"{"cmd":"subscribe"}"#
        );
    }

    #[test]
    fn replies_round_trip() {
        let reply = Reply::Report(Report {
            source: Color::new(1, 2, 3),
            mode: Mode::Dark,
            wallpaper: None,
            targets: vec![TargetReport {
                name: "kitty".into(),
                has_files: true,
                written: vec![],
                reload: Some(Outcome::NotInstalled {
                    command: "kitty".into(),
                }),
                error: None,
            }],
        });
        let json = serde_json::to_string(&reply).unwrap();
        assert!(json.starts_with(r#"{"reply":"report""#), "{json}");
        assert_eq!(serde_json::from_str::<Reply>(&json).unwrap(), reply);
    }
}
