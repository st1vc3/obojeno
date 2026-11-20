//! Finding an image's source color, and making its gallery thumbnail, in a
//! short-lived helper process (`obojeno analyze`).
//!
//! Colors come from the image at full size, like matugen's: a 6K wallpaper
//! takes over 100 MB to decode, which the allocator then keeps for reuse.
//! In a helper the memory goes back to the system when it exits, and an
//! image that crashes a decoder takes down the helper, not the daemon.

use std::path::{Path, PathBuf};
use std::process::Command;

use image::codecs::jpeg::JpegEncoder;
use obojeno_core::output::{Wallpaper, write_if_changed};
use obojeno_core::{Color, palette};
use serde::{Deserialize, Serialize};

use crate::imageio;

/// Longest side of gallery thumbnails: sharp in a grid at 2x.
pub const THUMBNAIL_SIDE: u32 = 512;
const THUMBNAIL_QUALITY: u8 = 85;

/// What the helper finds out about an image.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Analysis {
    pub source: Color,
    /// SHA-256 of the file, which colors are cached by.
    pub key: String,
}

/// Analyzes `path` in a helper process, writing its thumbnail to
/// `thumbnail` if given.
pub fn in_helper(path: &Path, thumbnail: Option<&Path>) -> Result<Analysis, String> {
    // An absolute program path lets std spawn without forking, which is not
    // safe while other threads are inside CoreFoundation.
    let program = std::env::current_exe().map_err(|e| format!("finding obojeno: {e}"))?;
    let mut command = Command::new(program);
    command.arg("analyze").arg(path);
    if let Some(thumbnail) = thumbnail {
        command.arg("--thumbnail").arg(thumbnail);
    }
    let output = command
        .output()
        .map_err(|e| format!("starting the analysis: {e}"))?;
    if !output.status.success() {
        let message = String::from_utf8_lossy(&output.stderr);
        let message = message.trim().trim_start_matches("obojeno: ");
        return Err(if message.is_empty() {
            format!("the analysis ended with {}", output.status)
        } else {
            message.to_owned()
        });
    }
    serde_json::from_slice(&output.stdout).map_err(|e| format!("reading the analysis: {e}"))
}

/// The analysis itself, run by `obojeno analyze`.
pub fn analyze(path: &Path, thumbnail: Option<&PathBuf>) -> Result<Analysis, String> {
    let wallpaper = Wallpaper::open_with(path, imageio::decode).map_err(|e| e.to_string())?;
    let image = wallpaper.image().map_err(str::to_owned)?;
    let source = palette::source_color(image);
    if let Some(thumbnail) = thumbnail {
        // From the image decoded for the color: one decode per image. Only
        // ever smaller; `thumbnail` would also enlarge small images.
        let small = if image.width().max(image.height()) > THUMBNAIL_SIDE {
            image.thumbnail(THUMBNAIL_SIDE, THUMBNAIL_SIDE).to_rgb8()
        } else {
            image.to_rgb8()
        };
        let mut jpeg = Vec::new();
        JpegEncoder::new_with_quality(&mut jpeg, THUMBNAIL_QUALITY)
            .encode_image(&small)
            .map_err(|e| e.to_string())?;
        write_if_changed(thumbnail, &jpeg)
            .map_err(|e| format!("writing {}: {e}", thumbnail.display()))?;
    }
    Ok(Analysis {
        source,
        key: wallpaper.key,
    })
}
