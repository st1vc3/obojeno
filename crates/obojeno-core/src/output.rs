//! Rendering targets and writing their files.

use std::cell::OnceCell;
use std::io::{Cursor, Read, Write};
use std::path::{Path, PathBuf};

use image::DynamicImage;
use image::codecs::jpeg::JpegEncoder;
use image::imageops::FilterType;
use sha2::{Digest, Sha256};

use crate::config::ResolvedTarget;
use crate::palette::{Mode, Palette};
use crate::targets::{MergeFormat, OutputKind, Reload};
use crate::template::{Context, Renderer, TemplateError};

/// Where `~`, `{{home}}`, `{{cache}}` and `{{config}}` point.
#[derive(Debug, Clone)]
pub struct Dirs {
    pub home: PathBuf,
    /// Generated files apps read, `~/.cache/obojeno`.
    pub cache: PathBuf,
    /// `~/.config/obojeno`.
    pub config: PathBuf,
}

impl Dirs {
    /// Expands a leading `~` to the home directory.
    pub fn expand(&self, path: &str) -> PathBuf {
        match path.strip_prefix('~') {
            Some("") => self.home.clone(),
            Some(rest) if rest.starts_with('/') => self.home.join(&rest[1..]),
            _ => PathBuf::from(path),
        }
    }
}

/// The current wallpaper. Its pixels are decoded on first use only, since
/// most applies never need them.
pub struct Wallpaper {
    pub path: PathBuf,
    /// SHA-256 of the file contents. Outputs made from the pixels record it
    /// and are skipped while it stays the same.
    pub key: String,
    decode: Decoder,
    image: OnceCell<Result<DynamicImage, String>>,
}

/// Turns an image file into pixels. Platforms can plug in their own codecs.
pub type Decoder = fn(&Path) -> Result<DynamicImage, String>;

fn decode_with_image_crate(path: &Path) -> Result<DynamicImage, String> {
    crate::palette::open_image(path).map_err(|e| e.to_string())
}

impl Wallpaper {
    /// Hashes the file; decoding happens lazily in [`Self::image`].
    pub fn open(path: &Path) -> std::io::Result<Self> {
        Self::open_with(path, decode_with_image_crate)
    }

    /// Like [`Self::open`], decoding with `decode`.
    pub fn open_with(path: &Path, decode: Decoder) -> std::io::Result<Self> {
        let mut file = std::fs::File::open(path)?;
        let mut hasher = Sha256::new();
        let mut buf = vec![0; 1 << 16];
        loop {
            match file.read(&mut buf)? {
                0 => break,
                n => hasher.update(&buf[..n]),
            }
        }
        Ok(Self {
            path: path.to_owned(),
            key: hex(&hasher.finalize()),
            decode,
            image: OnceCell::new(),
        })
    }

    /// A wallpaper whose image is already decoded.
    pub fn with_image(path: &Path, key: &str, image: DynamicImage) -> Self {
        Self {
            path: path.to_owned(),
            key: key.to_owned(),
            decode: decode_with_image_crate,
            image: OnceCell::from(Ok(image)),
        }
    }

    pub fn image(&self) -> Result<&DynamicImage, &str> {
        self.image
            .get_or_init(|| (self.decode)(&self.path))
            .as_ref()
            .map_err(String::as_str)
    }
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// The file next to a pixel-derived output that records which wallpaper it
/// was made from.
fn source_marker(path: &Path) -> PathBuf {
    let mut name = path.file_name().unwrap_or_default().to_owned();
    name.push(".source");
    path.with_file_name(name)
}

#[derive(Debug, thiserror::Error)]
pub enum OutputError {
    #[error("{target}: template: {error}")]
    Template {
        target: String,
        error: TemplateError,
    },
    #[error("{target}: {path}: {message}")]
    File {
        target: String,
        path: PathBuf,
        message: String,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RenderedFile {
    pub path: PathBuf,
    pub contents: Vec<u8>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RenderedTarget {
    pub name: String,
    pub files: Vec<RenderedFile>,
    /// Commands with their arguments rendered.
    pub reload: Vec<Reload>,
    pub only_if_running: Option<String>,
}

pub fn render_target(
    resolved: &ResolvedTarget,
    palette: &Palette,
    mode: Mode,
    wallpaper: Option<&Wallpaper>,
    dirs: &Dirs,
    renderer: &Renderer,
) -> Result<RenderedTarget, OutputError> {
    let name = &resolved.target.name;
    let template_err = |error| OutputError::Template {
        target: name.clone(),
        error,
    };
    let home = dirs.home.display().to_string();
    let cache = dirs.cache.display().to_string();
    let config = dirs.config.display().to_string();
    // Options may be paths with `~`; expand them so templates get usable paths.
    let options: Vec<(String, String)> = resolved
        .options
        .iter()
        .map(|(k, v)| (k.clone(), dirs.expand(v).display().to_string()))
        .collect();
    let ctx = Context::new(palette, mode, wallpaper.map(|w| w.path.as_path())).with_vars(
        [
            ("home", home.as_str()),
            ("cache", cache.as_str()),
            ("config", config.as_str()),
        ]
        .into_iter()
        .chain(options.iter().map(|(k, v)| (k.as_str(), v.as_str()))),
    );
    let render = |what: &str, source: &str| {
        renderer
            .render(&format!("{name} {what}"), source, &ctx)
            .map_err(template_err)
    };

    let mut files = Vec::new();
    for output in &resolved.target.outputs {
        let path = dirs.expand(&render("output path", &output.path)?);
        let file_err = |message: String| OutputError::File {
            target: name.clone(),
            path: path.clone(),
            message,
        };
        let contents = match &output.kind {
            OutputKind::Text { template } => render("template", template)?.into_bytes(),
            OutputKind::Merge {
                format,
                template,
                base,
            } => {
                let overlay = render("template", template)?;
                let base_path = match base {
                    Some(b) => dirs.expand(&render("base path", b)?),
                    None => path.clone(),
                };
                let base_text = match std::fs::read_to_string(&base_path) {
                    Ok(t) => t,
                    Err(e) if e.kind() == std::io::ErrorKind::NotFound => String::new(),
                    Err(e) => {
                        return Err(file_err(format!("reading {}: {e}", base_path.display())));
                    }
                };
                merge(*format, &base_text, &overlay)
                    .map_err(|e| file_err(format!("merging into {}: {e}", base_path.display())))?
                    .into_bytes()
            }
            OutputKind::BlurredWallpaper {
                width,
                sigma,
                quality,
            } => {
                let Some(wallpaper) = wallpaper else { continue };
                let marker = source_marker(&path);
                if path.exists()
                    && std::fs::read_to_string(&marker).is_ok_and(|k| k == wallpaper.key)
                {
                    continue;
                }
                let image = wallpaper
                    .image()
                    .map_err(|e| file_err(format!("decoding {}: {e}", wallpaper.path.display())))?;
                let jpeg = blurred_jpeg(image, *width, *sigma, *quality)
                    .map_err(|e| file_err(e.to_string()))?;
                files.push(RenderedFile {
                    path,
                    contents: jpeg,
                });
                // Written after the image, so a crash in between re-renders.
                files.push(RenderedFile {
                    path: marker,
                    contents: wallpaper.key.clone().into_bytes(),
                });
                continue;
            }
        };
        files.push(RenderedFile { path, contents });
    }

    let reload = resolved
        .target
        .reload
        .iter()
        .map(|reload| match reload {
            Reload::Command(args) => args
                .iter()
                .map(|a| render("reload command", a))
                .collect::<Result<Vec<_>, _>>()
                .map(Reload::Command),
            Reload::Signal { .. } => Ok(reload.clone()),
        })
        .collect::<Result<_, _>>()?;

    Ok(RenderedTarget {
        name: name.clone(),
        files,
        reload,
        only_if_running: resolved.target.only_if_running.clone(),
    })
}

/// Deep-merges `overlay` into `base`: tables merge key by key, everything
/// else is replaced. TOML keeps the base's comments and formatting.
pub fn merge(format: MergeFormat, base: &str, overlay: &str) -> Result<String, String> {
    match format {
        MergeFormat::Json => {
            let mut base: serde_json::Value = if base.trim().is_empty() {
                serde_json::Value::Object(serde_json::Map::new())
            } else {
                serde_json::from_str(base).map_err(|e| format!("base is not valid JSON: {e}"))?
            };
            let overlay: serde_json::Value = serde_json::from_str(overlay)
                .map_err(|e| format!("template is not valid JSON: {e}"))?;
            merge_json(&mut base, overlay);
            let mut text = serde_json::to_string_pretty(&base).map_err(|e| e.to_string())?;
            text.push('\n');
            Ok(text)
        }
        MergeFormat::Toml => {
            let mut base: toml_edit::DocumentMut = base
                .parse()
                .map_err(|e| format!("base is not valid TOML: {e}"))?;
            let overlay: toml_edit::DocumentMut = overlay
                .parse()
                .map_err(|e| format!("template is not valid TOML: {e}"))?;
            merge_table_like(base.as_table_mut(), overlay.as_table());
            Ok(base.to_string())
        }
    }
}

fn merge_json(base: &mut serde_json::Value, overlay: serde_json::Value) {
    match (base, overlay) {
        (serde_json::Value::Object(base), serde_json::Value::Object(overlay)) => {
            for (key, value) in overlay {
                match base.get_mut(&key) {
                    Some(existing) => merge_json(existing, value),
                    None => {
                        base.insert(key, value);
                    }
                }
            }
        }
        (base, overlay) => *base = overlay,
    }
}

fn merge_table_like(base: &mut dyn toml_edit::TableLike, overlay: &dyn toml_edit::TableLike) {
    for (key, item) in overlay.iter() {
        match (base.get_mut(key), item.as_table_like()) {
            (Some(existing), Some(overlay_table)) if existing.is_table_like() => {
                let existing = existing.as_table_like_mut().expect("checked table-like");
                merge_table_like(existing, overlay_table);
            }
            _ => {
                base.insert(key, item.clone());
            }
        }
    }
}

fn blurred_jpeg(
    image: &DynamicImage,
    width: u32,
    sigma: f32,
    quality: u8,
) -> image::ImageResult<Vec<u8>> {
    let height =
        (u64::from(image.height()) * u64::from(width) / u64::from(image.width().max(1))).max(1);
    let small = image.resize_exact(
        width,
        u32::try_from(height).unwrap_or(u32::MAX),
        FilterType::Triangle,
    );
    let blurred = image::imageops::blur(&small.to_rgb8(), sigma);
    let mut bytes = Cursor::new(Vec::new());
    JpegEncoder::new_with_quality(&mut bytes, quality).encode_image(&blurred)?;
    Ok(bytes.into_inner())
}

/// Writes a file atomically (temp file + rename) and creates parent folders.
/// Returns `false` without touching the file when it already has `contents`,
/// so file watchers in apps do not fire for nothing.
pub fn write_if_changed(path: &Path, contents: &[u8]) -> std::io::Result<bool> {
    if std::fs::read(path).is_ok_and(|existing| existing == contents) {
        return Ok(false);
    }
    let dir = path.parent().unwrap_or_else(|| Path::new("."));
    std::fs::create_dir_all(dir)?;
    let mut tmp = tempfile::NamedTempFile::new_in(dir)?;
    tmp.write_all(contents)?;
    // Keep the permissions of a file being replaced, else use 0644. Not those
    // of a symlink's target: replacing a link into the read-only Nix store
    // must not make the new file read-only.
    let mode = std::fs::symlink_metadata(path)
        .ok()
        .filter(std::fs::Metadata::is_file)
        .map_or(0o644, |m| {
            use std::os::unix::fs::PermissionsExt;
            m.permissions().mode()
        });
    {
        use std::os::unix::fs::PermissionsExt;
        tmp.as_file()
            .set_permissions(std::fs::Permissions::from_mode(mode))?;
    }
    tmp.persist(path).map_err(|e| e.error)?;
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Config;
    use crate::{Color, SchemeVariant};

    fn setup() -> (tempfile::TempDir, Dirs, Palette) {
        let tmp = tempfile::tempdir().unwrap();
        let dirs = Dirs {
            home: tmp.path().to_owned(),
            cache: tmp.path().join(".cache/obojeno"),
            config: tmp.path().join(".config/obojeno"),
        };
        let palette =
            Palette::from_source(Color::new(0xe8, 0x26, 0x49), SchemeVariant::TonalSpot, 0.0);
        (tmp, dirs, palette)
    }

    fn render(
        config: &str,
        dirs: &Dirs,
        palette: &Palette,
        wallpaper: Option<&Wallpaper>,
    ) -> Vec<RenderedTarget> {
        Config::parse(config)
            .unwrap()
            .resolve_targets(|p| std::fs::read_to_string(p))
            .unwrap()
            .iter()
            .map(|t| {
                render_target(t, palette, Mode::Dark, wallpaper, dirs, &Renderer::new()).unwrap()
            })
            .collect()
    }

    #[test]
    fn expands_home() {
        let (_tmp, dirs, _) = setup();
        assert_eq!(dirs.expand("~"), dirs.home);
        assert_eq!(dirs.expand("~/a/b"), dirs.home.join("a/b"));
        assert_eq!(dirs.expand("/abs/~x"), PathBuf::from("/abs/~x"));
        assert_eq!(dirs.expand("~other/x"), PathBuf::from("~other/x"));
    }

    #[test]
    fn every_builtin_renders() {
        let (tmp, dirs, palette) = setup();
        let wallpaper =
            Wallpaper::with_image(Path::new("/w.jpg"), "k1", DynamicImage::new_rgb8(64, 40));
        let config = format!(
            "[targets.kitty]\n[targets.borders]\n[targets.simple-bar]\n[targets.nvim]\n[targets.starship]\n[targets.herdr]\n\
             [targets.obsidian]\nvault = \"~/Notes\"\n[targets.zen]\nprofile = \"{}/zen\"\n",
            tmp.path().display()
        );
        let rendered = render(&config, &dirs, &palette, Some(&wallpaper));
        assert_eq!(rendered.len(), 8);
        for t in &rendered {
            for f in &t.files {
                assert!(
                    f.path.starts_with(tmp.path()),
                    "{} writes outside home: {}",
                    t.name,
                    f.path.display()
                );
                if f.path.extension().is_some_and(|e| e != "jpg") {
                    let text = std::str::from_utf8(&f.contents).unwrap();
                    assert!(
                        !text.contains("{{") && !text.contains("}}"),
                        "{}: unrendered template",
                        t.name
                    );
                }
            }
        }
        let borders = rendered.iter().find(|t| t.name == "borders").unwrap();
        let Reload::Command(args) = &borders.reload[0] else {
            panic!("borders reloads with a command")
        };
        assert_eq!(
            args[1],
            format!("active_color=0xff{}", palette.source.hex_stripped())
        );
        let obsidian = rendered.iter().find(|t| t.name == "obsidian").unwrap();
        assert_eq!(
            obsidian.files[0].path,
            tmp.path().join("Notes/.obsidian/snippets/obojeno.css")
        );
    }

    #[test]
    fn builtin_merge_templates_are_valid() {
        let (_tmp, dirs, palette) = setup();
        let rendered = render(
            "[targets.simple-bar]\n[targets.starship]\n[targets.herdr]",
            &dirs,
            &palette,
            None,
        );
        for t in rendered {
            for f in t.files {
                let text = String::from_utf8(f.contents).unwrap();
                if f.path.extension().is_some_and(|e| e == "toml") {
                    text.parse::<toml_edit::DocumentMut>().unwrap();
                } else {
                    serde_json::from_str::<serde_json::Value>(&text).unwrap();
                }
            }
        }
    }

    #[test]
    fn blurred_wallpaper_is_skipped_without_wallpaper() {
        let (_tmp, dirs, palette) = setup();
        let rendered = render("[targets.simple-bar]", &dirs, &palette, None);
        assert_eq!(rendered[0].files.len(), 1);
    }

    #[test]
    fn blurred_wallpaper_is_only_rendered_when_the_wallpaper_changes() {
        let (_tmp, dirs, palette) = setup();
        let write_all = |rendered: &[RenderedTarget]| {
            for f in &rendered[0].files {
                write_if_changed(&f.path, &f.contents).unwrap();
            }
        };
        let blur_files = |rendered: &[RenderedTarget]| {
            rendered[0]
                .files
                .iter()
                .filter(|f| !f.path.ends_with(".simplebarrc"))
                .count()
        };
        let first =
            Wallpaper::with_image(Path::new("/a.jpg"), "aaa", DynamicImage::new_rgb8(32, 18));
        let rendered = render("[targets.simple-bar]", &dirs, &palette, Some(&first));
        assert_eq!(blur_files(&rendered), 2, "image and marker");
        write_all(&rendered);

        // Same wallpaper: nothing to decode or write.
        // Decoding this one would fail, proving it is never decoded.
        let same = Wallpaper {
            path: "/nonexistent.jpg".into(),
            key: "aaa".to_owned(),
            decode: |_| panic!("must not decode"),
            image: OnceCell::new(),
        };
        let rendered = render("[targets.simple-bar]", &dirs, &palette, Some(&same));
        assert_eq!(blur_files(&rendered), 0);

        let second =
            Wallpaper::with_image(Path::new("/b.jpg"), "bbb", DynamicImage::new_rgb8(32, 18));
        assert_eq!(
            blur_files(&render(
                "[targets.simple-bar]",
                &dirs,
                &palette,
                Some(&second)
            )),
            2
        );
    }

    #[test]
    fn wallpaper_key_is_the_content_hash() {
        let (tmp, ..) = setup();
        let path = tmp.path().join("w.bin");
        std::fs::write(&path, b"abc").unwrap();
        let w = Wallpaper::open(&path).unwrap();
        assert_eq!(
            w.key,
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
        assert!(w.image().is_err(), "not an image");
    }

    #[test]
    fn blurred_wallpaper_keeps_aspect_ratio() {
        let bytes = blurred_jpeg(&DynamicImage::new_rgb8(1600, 900), 900, 4.0, 82).unwrap();
        let img = image::load_from_memory(&bytes).unwrap();
        assert_eq!((img.width(), img.height()), (900, 506));
    }

    #[test]
    fn json_merge_keeps_unrelated_keys_and_order() {
        let base = r#"{"global": {"theme": "dark", "font": "Hack"}, "widgets": {"a": true}}"#;
        let out = merge(
            MergeFormat::Json,
            base,
            r##"{"global": {"theme": "light"}, "themes": {"x": "#fff"}}"##,
        )
        .unwrap();
        let v: serde_json::Value = serde_json::from_str(&out).unwrap();
        assert_eq!(v["global"]["theme"], "light");
        assert_eq!(v["global"]["font"], "Hack");
        assert_eq!(v["widgets"]["a"], true);
        let keys: Vec<_> = v.as_object().unwrap().keys().collect();
        assert_eq!(keys, ["global", "widgets", "themes"]);
    }

    #[test]
    fn toml_merge_preserves_comments() {
        let base = "# my config\n[ui]\nagent_panel_sort = \"spaces\" # keep\n\n[theme]\nname = \"catppuccin\"\n";
        let out = merge(
            MergeFormat::Toml,
            base,
            "[theme.custom]\naccent = \"#ff0000\"\n",
        )
        .unwrap();
        assert!(
            out.contains("# my config") && out.contains("# keep"),
            "{out}"
        );
        let doc: toml::Table = out.parse().unwrap();
        assert_eq!(doc["theme"]["name"].as_str(), Some("catppuccin"));
        assert_eq!(doc["theme"]["custom"]["accent"].as_str(), Some("#ff0000"));
    }

    #[test]
    fn toml_merge_without_base() {
        let out = merge(
            MergeFormat::Toml,
            "",
            "palette = \"obojeno\"\n[palettes.obojeno]\nred = \"#f00\"\n",
        )
        .unwrap();
        let doc: toml::Table = out.parse().unwrap();
        assert_eq!(doc["palette"].as_str(), Some("obojeno"));
    }

    #[test]
    fn toml_merge_puts_top_level_keys_before_tables() {
        let base = "add_newline = false\n[character]\nsuccess_symbol = \">\"\n";
        let out = merge(
            MergeFormat::Toml,
            base,
            "palette = \"obojeno\"\n[palettes.obojeno]\nred = \"#f00\"\n",
        )
        .unwrap();
        // A top-level key after a table header would belong to that table.
        let doc: toml::Table = out.parse().unwrap();
        assert_eq!(doc["palette"].as_str(), Some("obojeno"), "{out}");
        assert!(doc["character"].get("palette").is_none(), "{out}");
    }

    #[test]
    fn merge_reports_invalid_base() {
        assert!(
            merge(MergeFormat::Json, "{oops", "{}")
                .unwrap_err()
                .contains("base")
        );
        assert!(
            merge(MergeFormat::Toml, "[[", "")
                .unwrap_err()
                .contains("base")
        );
    }

    #[test]
    fn write_if_changed_is_atomic_and_idempotent() {
        let (tmp, ..) = setup();
        let path = tmp.path().join("deep/dir/file.txt");
        assert!(write_if_changed(&path, b"one").unwrap());
        assert!(!write_if_changed(&path, b"one").unwrap());
        assert!(write_if_changed(&path, b"two").unwrap());
        assert_eq!(std::fs::read(&path).unwrap(), b"two");
        let leftovers: Vec<_> = std::fs::read_dir(path.parent().unwrap()).unwrap().collect();
        assert_eq!(leftovers.len(), 1, "temp files left behind");
    }

    #[test]
    fn write_if_changed_replaces_symlinks_with_writable_files() {
        use std::os::unix::fs::PermissionsExt;
        let (tmp, ..) = setup();
        let target = tmp.path().join("store-file");
        std::fs::write(&target, "old").unwrap();
        std::fs::set_permissions(&target, std::fs::Permissions::from_mode(0o444)).unwrap();
        let link = tmp.path().join("config");
        std::os::unix::fs::symlink(&target, &link).unwrap();

        write_if_changed(&link, b"new").unwrap();
        let meta = std::fs::symlink_metadata(&link).unwrap();
        assert!(meta.is_file(), "the link became a file");
        assert_eq!(meta.permissions().mode() & 0o777, 0o644);
        assert_eq!(
            std::fs::read(&target).unwrap(),
            b"old",
            "the link target is untouched"
        );
    }

    #[test]
    fn write_if_changed_keeps_permissions() {
        use std::os::unix::fs::PermissionsExt;
        let (tmp, ..) = setup();
        let path = tmp.path().join("f");
        std::fs::write(&path, "a").unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).unwrap();
        write_if_changed(&path, b"b").unwrap();
        assert_eq!(
            std::fs::metadata(&path).unwrap().permissions().mode() & 0o777,
            0o600
        );
    }
}
