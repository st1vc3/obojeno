//! Snapshots of every built-in target rendered from a fixed palette, so
//! template changes show up in review. Update with `cargo insta review`.

use std::fmt::Write as _;
use std::path::Path;

use obojeno_core::config::Config;
use obojeno_core::output::{self, Dirs, Wallpaper};
use obojeno_core::template::Renderer;
use obojeno_core::{Color, Mode, Palette, SchemeVariant};

#[test]
fn builtin_targets() {
    let dirs = Dirs {
        home: "/home".into(),
        cache: "/home/.cache/obojeno".into(),
        config: "/home/.config/obojeno".into(),
    };
    let palette = Palette::from_source(Color::new(0xe8, 0x26, 0x49), SchemeVariant::TonalSpot, 0.0);
    let wallpaper = Wallpaper::with_image(
        Path::new("/walls/red.jpg"),
        "key",
        image::DynamicImage::new_rgb8(16, 9),
    );
    let config = Config::parse(
        "[targets.kitty]\n[targets.borders]\n[targets.simple-bar]\n[targets.nvim]\n\
         [targets.starship]\nbase = \"/nonexistent\"\n[targets.herdr]\nbase = \"/nonexistent\"\n\
         [targets.obsidian]\nvault = \"~/Notes\"\n[targets.zen]\nprofile = \"/zen\"\n",
    )
    .unwrap();
    let engine = Renderer::new();
    for resolved in config.resolve_targets(|_| unreachable!()).unwrap() {
        for mode in [Mode::Dark, Mode::Light] {
            let rendered =
                output::render_target(&resolved, &palette, mode, Some(&wallpaper), &dirs, &engine)
                    .unwrap();
            let mut snapshot = String::new();
            for file in &rendered.files {
                writeln!(snapshot, "==> {}", file.path.display()).unwrap();
                match std::str::from_utf8(&file.contents) {
                    Ok(text) if file.path.extension().is_none_or(|e| e != "jpg") => {
                        snapshot.push_str(text);
                        if !text.ends_with('\n') {
                            snapshot.push('\n');
                        }
                    }
                    _ => writeln!(snapshot, "<{} bytes>", file.contents.len()).unwrap(),
                }
            }
            for reload in &rendered.reload {
                writeln!(snapshot, "==> reload: {reload:?}").unwrap();
            }
            if let Some(process) = &rendered.only_if_running {
                writeln!(snapshot, "==> only if running: {process}").unwrap();
            }
            insta::assert_snapshot!(format!("{}_{mode}", rendered.name), snapshot);
        }
    }
}
