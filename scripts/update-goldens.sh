#!/usr/bin/env bash
# Regenerates crates/obojeno-core/tests/goldens/*.json from the two reference
# implementations obojeno is checked against:
#   - matugen for picking the source color of a wallpaper
#   - Google's material-color-utilities for the schemes built from it
set -euo pipefail

root="$(cd "$(dirname "$0")/.." && pwd)"
fixtures="$root/crates/obojeno-core/tests/fixtures"
goldens="$root/crates/obojeno-core/tests/goldens"
mcu_version="0.4.0"

matugen="$(nix build --no-link --print-out-paths nixpkgs#matugen)/bin/matugen"
node_bin="$(nix build --no-link --print-out-paths nixpkgs#nodejs)/bin"
export PATH="$node_bin:$PATH"

work="$(mktemp -d)"
trap 'rm -rf "$work"' EXIT
cp "$root/scripts/mcu-schemes.mjs" "$work/"
(
  cd "$work"
  npm init -y >/dev/null
  npm install --silent "@material/material-color-utilities@$mcu_version" esbuild
  # The published package uses extensionless ESM imports that node rejects;
  # bundling resolves them.
  npx esbuild mcu-schemes.mjs --bundle --platform=node --format=esm --log-level=warning --outfile=bundle.mjs
)

mkdir -p "$goldens"
for image in "$fixtures"/*.png; do
  name="$(basename "$image" .png)"
  source="$("$matugen" image "$image" --dry-run --quiet --json hex --source-color-index 0 |
    jq -r '.colors.source_color.default.color')"
  node "$work/bundle.mjs" "$source" |
    jq --sort-keys --arg source "$source" 'map_values(. + {source: $source})' >"$goldens/$name.json"
  echo "wrote $goldens/$name.json (source $source)"
done
