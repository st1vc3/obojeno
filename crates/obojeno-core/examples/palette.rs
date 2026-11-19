//! Prints the palette of an image: `cargo run --example palette -- <image> [variant]`

use obojeno_core::palette::{self, SchemeVariant};
use obojeno_core::{Mode, Palette};

fn main() {
    let mut args = std::env::args().skip(1);
    let path = args.next().expect("usage: palette <image> [variant]");
    let variant: SchemeVariant = args
        .next()
        .map_or_else(SchemeVariant::default, |v| v.parse().unwrap());
    let image = palette::open_image(path.as_ref()).unwrap();
    let candidates = palette::source_candidates(&image);
    println!(
        "candidates: {}",
        candidates
            .iter()
            .map(ToString::to_string)
            .collect::<Vec<_>>()
            .join(" ")
    );
    let p = Palette::from_source(candidates[0], variant, 0.0);
    for mode in [Mode::Dark, Mode::Light] {
        println!("{mode}:");
        for role in [
            "primary",
            "secondary",
            "tertiary",
            "surface",
            "on_surface",
            "primary_container",
        ] {
            println!("  {role:18} {}", p.role(mode, role));
        }
        let t = p.terminal(mode);
        println!(
            "  terminal           {}",
            t.iter()
                .map(ToString::to_string)
                .collect::<Vec<_>>()
                .join(" ")
        );
    }
}
