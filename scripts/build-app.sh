#!/usr/bin/env bash
# Builds target/Obojeno.app: the menu bar app with the obojeno command
# (CLI and daemon) inside it. Needs cargo (e.g. `nix develop`) and swift
# (Xcode or the Command Line Tools).
#
#   UNIVERSAL=1        build for Apple silicon and Intel (needs rustup with
#                      both targets, and Xcode for swift)
#   SIGN_IDENTITY=...  sign with this Developer ID (hardened runtime, secure
#                      timestamp) instead of ad hoc
set -euo pipefail

root="$(cd "$(dirname "$0")/.." && pwd)"
app="$root/target/Obojeno.app"
# Apple's swift, even inside `nix develop`, whose SDK variables would make
# the xcrun shim look for swift in the Nix SDK.
swift() {
  env -u DEVELOPER_DIR -u SDKROOT -u NIX_CFLAGS_COMPILE -u NIX_LDFLAGS /usr/bin/swift "$@"
}

version="$("$root/scripts/version.sh")"

rm -rf "$app"
mkdir -p "$app/Contents/MacOS" "$app/Contents/Helpers"

if [[ "${UNIVERSAL:-}" == 1 ]]; then
  binaries=()
  for target in aarch64-apple-darwin x86_64-apple-darwin; do
    cargo build --release --locked --package obojeno --target "$target" --manifest-path "$root/Cargo.toml"
    binaries+=("$root/target/$target/release/obojeno")
  done
  lipo -create -output "$app/Contents/Helpers/obojeno" "${binaries[@]}"
  swift_arch=(--arch arm64 --arch x86_64)
else
  cargo build --release --locked --package obojeno --manifest-path "$root/Cargo.toml"
  cp "$root/target/release/obojeno" "$app/Contents/Helpers/obojeno"
  swift_arch=()
fi
swift build --configuration release ${swift_arch[@]+"${swift_arch[@]}"} --product Obojeno --package-path "$root/app"
cp "$(swift build --configuration release ${swift_arch[@]+"${swift_arch[@]}"} --package-path "$root/app" --show-bin-path)/Obojeno" \
  "$app/Contents/MacOS/Obojeno"

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
  <key>LSMinimumSystemVersion</key><string>26.0</string>
  <key>LSUIElement</key><true/>
  <key>NSHumanReadableCopyright</key><string>MIT License</string>
</dict>
</plist>
PLIST

# Inside out: the helper first, then the bundle that seals it.
if [[ -n "${SIGN_IDENTITY:-}" ]]; then
  sign=(--sign "$SIGN_IDENTITY" --options runtime --timestamp)
else
  sign=(--sign -)
fi
codesign --force "${sign[@]}" "$app/Contents/Helpers/obojeno"
codesign --force "${sign[@]}" "$app"

codesign --verify --strict --deep "$app"
if [[ "${UNIVERSAL:-}" == 1 ]]; then
  for binary in "$app/Contents/MacOS/Obojeno" "$app/Contents/Helpers/obojeno"; do
    archs="$(lipo -archs "$binary" | tr ' ' '\n' | sort | xargs)"
    if [[ "$archs" != "arm64 x86_64" ]]; then
      echo "$binary is $archs, not universal" >&2
      exit 1
    fi
  done
fi
echo "built $app"
