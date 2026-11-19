#!/usr/bin/env bash
# Builds target/Obojeno.app: the menu bar app with the obojeno command
# (CLI and daemon) inside it. Needs cargo (e.g. `nix develop`) and swift
# (Xcode or the Command Line Tools).
set -euo pipefail

root="$(cd "$(dirname "$0")/.." && pwd)"
app="$root/target/Obojeno.app"
# Apple's swift, even inside `nix develop`, whose SDK variables would make
# the xcrun shim look for swift in the Nix SDK.
swift() {
  env -u DEVELOPER_DIR -u SDKROOT -u NIX_CFLAGS_COMPILE -u NIX_LDFLAGS /usr/bin/swift "$@"
}

version="$(sed -n 's/^version = "\(.*\)"/\1/p' "$root/Cargo.toml" | head -1)"

cargo build --release --locked --package obojeno --manifest-path "$root/Cargo.toml"
swift build --configuration release --package-path "$root/app"

rm -rf "$app"
mkdir -p "$app/Contents/MacOS" "$app/Contents/Helpers"
cp "$(swift build --configuration release --package-path "$root/app" --show-bin-path)/Obojeno" "$app/Contents/MacOS/Obojeno"
cp "$root/target/release/obojeno" "$app/Contents/Helpers/obojeno"

cat >"$app/Contents/Info.plist" <<PLIST
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>CFBundleIdentifier</key><string>io.github.st1vc3.obojeno</string>
  <key>CFBundleName</key><string>Obojeno</string>
  <key>CFBundleDisplayName</key><string>Obojeno</string>
  <key>CFBundleExecutable</key><string>Obojeno</string>
  <key>CFBundlePackageType</key><string>APPL</string>
  <key>CFBundleShortVersionString</key><string>$version</string>
  <key>CFBundleVersion</key><string>$version</string>
  <key>LSMinimumSystemVersion</key><string>14.0</string>
  <key>LSUIElement</key><true/>
  <key>NSHumanReadableCopyright</key><string>MIT License</string>
</dict>
</plist>
PLIST

# Ad-hoc signatures, inside out. Release builds re-sign with a Developer ID.
codesign --force --sign - "$app/Contents/Helpers/obojeno"
codesign --force --sign - "$app"
echo "built $app"
