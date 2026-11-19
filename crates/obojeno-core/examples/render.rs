//! Renders targets into a scratch directory without touching real configs:
//! `cargo run --example render -- <image> <config.toml> <out-dir> [light|dark]`
//! `{{cache}}` and `~` both point into `<out-dir>`.

use std::path::Path;

use obojeno_core::config::Config;
use obojeno_core::output::{self, Dirs, Wallpaper};
use obojeno_core::palette;
use obojeno_core::template::Renderer;
use obojeno_core::{Mode, Palette};

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let [image_path, config_path, out, rest @ ..] = args.as_slice() else {
        panic!("usage: render <image> <config.toml> <out-dir> [light|dark]");
    };
    let mode = match rest.first().map(String::as_str) {
        Some("light") => Mode::Light,
        _ => Mode::Dark,
    };
    let out = Path::new(out);
    let dirs = Dirs {
        home: out.to_owned(),
        cache: out.join(".cache/obojeno"),
        config: out.join(".config/obojeno"),
    };
    let config = Config::load(Path::new(config_path)).unwrap();
    let image = palette::open_image(Path::new(image_path)).unwrap();
    let palette = Palette::from_source(
        palette::source_color(&image),
        config.scheme,
        config.contrast,
    );
    let wallpaper = Wallpaper {
        path: Path::new(image_path),
        image: &image,
    };
    let renderer = Renderer::new();
    for resolved in config
        .resolve_targets(|p| std::fs::read_to_string(dirs.expand(p)))
        .unwrap()
    {
        let rendered = output::render_target(
            &resolved,
            &palette,
            mode,
            Some(&wallpaper),
            &dirs,
            &renderer,
        )
        .unwrap();
        for file in &rendered.files {
            let changed = output::write_if_changed(&file.path, &file.contents).unwrap();
            println!(
                "{}: {} ({})",
                rendered.name,
                file.path.display(),
                if changed { "written" } else { "unchanged" }
            );
        }
        for cmd in &rendered.reload {
            println!("{}: would run {cmd:?}", rendered.name);
        }
    }
}
