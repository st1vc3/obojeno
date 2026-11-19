//! Source colors must match matugen's (except for images without color),
//! and schemes must match Google's
//! material-color-utilities. Regenerate the goldens with
//! `scripts/update-goldens.sh`.

use std::collections::BTreeMap;
use std::path::Path;

use obojeno_core::palette::{self, Roles};
use obojeno_core::{Color, Mode, Palette, SchemeVariant};
use serde::Deserialize;

#[derive(Deserialize)]
struct Golden {
    source: Color,
    light: Roles,
    dark: Roles,
}

#[test]
fn palettes_match_reference_implementations() {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests");
    let mut checked = 0;
    let mut mismatches = Vec::new();

    for entry in std::fs::read_dir(dir.join("goldens")).unwrap() {
        let path = entry.unwrap().path();
        let name = path.file_stem().unwrap().to_str().unwrap().to_owned();
        let goldens: BTreeMap<SchemeVariant, Golden> =
            serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        let image = image::open(dir.join("fixtures").join(format!("{name}.png"))).unwrap();
        let source = palette::source_color(&image);

        for (variant, golden) in goldens {
            // matugen falls back to Google blue for images without color;
            // obojeno deliberately keeps their gray (and themes monochrome).
            if golden.source == Color::new(0x42, 0x85, 0xf4) {
                assert!(
                    palette::is_achromatic(source),
                    "{name}: expected a gray source, got {source}"
                );
                continue;
            }
            assert_eq!(source, golden.source, "{name}: source color");
            let palette = Palette::from_source(source, variant, 0.0);
            for (mode, expected) in [(Mode::Light, &golden.light), (Mode::Dark, &golden.dark)] {
                for (role, want) in expected {
                    // The `*_dim` roles only exist in the 2025 spec.
                    if role.ends_with("_dim") && !role.ends_with("_fixed_dim") {
                        continue;
                    }
                    let got = palette.role(mode, role);
                    if got != *want {
                        mismatches.push(format!(
                            "{name} {variant} {mode} {role}: got {got}, reference {want}"
                        ));
                    }
                    checked += 1;
                }
            }
        }
    }

    assert!(checked > 0, "no goldens found");
    assert!(
        mismatches.is_empty(),
        "{} of {checked} colors differ:\n{}",
        mismatches.len(),
        mismatches.join("\n")
    );
}
