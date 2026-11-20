#!/usr/bin/env bash
# Builds target/Obojeno.app: the menu bar app with the obojeno command
# (CLI and daemon) inside it. Needs cargo (e.g. `nix develop`) and swift
# (Xcode or the Command Line Tools).
#
#   UNIVERSAL=1        build for Apple silicon and Intel (needs rustup with
#                      both targets, and Xcode for swift)
#   SIGN_IDENTITY=...  sign with this Developer ID (hardened runtime, secure
#                      timestamp) instead of ad hoc
#   FEED_URL=...       the update feed, for testing updates (default: the
#                      latest GitHub release's appcast.xml)
set -euo pipefail

root="$(cd "$(dirname "$0")/.." && pwd)"
app="$root/target/Obojeno.app"
# Apple's swift, even inside `nix develop`, whose SDK variables would make
# the xcrun shim look for swift in the Nix SDK.
swift() {
  env -u DEVELOPER_DIR -u SDKROOT -u NIX_CFLAGS_COMPILE -u NIX_LDFLAGS /usr/bin/swift "$@"
}

version="$("$root/scripts/version.sh")"
feed="${FEED_URL:-https://github.com/st1vc3/obojeno/releases/latest/download/appcast.xml}"
# Sparkle accepts only updates signed with the private half of this key
# (the SPARKLE_PRIVATE_KEY secret of the release workflow).
public_key="W5G9/wImPmjFC9KBMw1gqgWpEiBnbsFmDW/zDvXTz1M="

rm -rf "$app"
mkdir -p "$app/Contents/MacOS" "$app/Contents/Helpers" "$app/Contents/Frameworks" "$app/Contents/Resources"

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
bin="$(swift build --configuration release ${swift_arch[@]+"${swift_arch[@]}"} --package-path "$root/app" --show-bin-path)"
cp "$bin/Obojeno" "$app/Contents/MacOS/Obojeno"
# Sparkle, without the XPC services only sandboxed apps need.
ditto "$bin/Sparkle.framework" "$app/Contents/Frameworks/Sparkle.framework"
rm -rf "$app/Contents/Frameworks/Sparkle.framework/Versions/B/XPCServices" \
  "$app/Contents/Frameworks/Sparkle.framework/XPCServices"

# The icon, drawn by app/Icon/icon.swift at every size macOS asks for.
iconset="$(mktemp -d)/AppIcon.iconset"
mkdir "$iconset"
for side in 16 32 128 256 512; do
  swift "$root/app/Icon/icon.swift" "$iconset/icon_${side}x${side}.png" "$side"
  swift "$root/app/Icon/icon.swift" "$iconset/icon_${side}x${side}@2x.png" "$((side * 2))"
done
iconutil --convert icns --output "$app/Contents/Resources/AppIcon.icns" "$iconset"
rm -rf "$(dirname "$iconset")"

cat >"$app/Contents/Info.plist" <<PLIST
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>CFBundleIdentifier</key><string>io.github.st1vc3.obojeno</string>
  <key>CFBundleName</key><string>Obojeno</string>
  <key>CFBundleDisplayName</key><string>Obojeno</string>
  <key>CFBundleExecutable</key><string>Obojeno</string>
  <key>CFBundleIconFile</key><string>AppIcon</string>
  <key>CFBundlePackageType</key><string>APPL</string>
  <key>CFBundleShortVersionString</key><string>$version</string>
  <key>CFBundleVersion</key><string>$version</string>
  <key>LSMinimumSystemVersion</key><string>26.0</string>
  <key>LSUIElement</key><true/>
  <key>NSHumanReadableCopyright</key><string>MIT License</string>
  <key>SUFeedURL</key><string>$feed</string>
  <key>SUPublicEDKey</key><string>$public_key</string>
  <key>SUEnableAutomaticChecks</key><true/>
  <key>SURequireSignedFeed</key><true/>
  <key>SUVerifyUpdateBeforeExtraction</key><true/>
</dict>
</plist>
PLIST

# Inside out: helpers and Sparkle's parts first, then the bundles that seal
# them.
if [[ -n "${SIGN_IDENTITY:-}" ]]; then
  sign=(--sign "$SIGN_IDENTITY" --options runtime --timestamp)
else
  sign=(--sign -)
fi
sparkle="$app/Contents/Frameworks/Sparkle.framework"
for part in "$sparkle/Versions/B/Autoupdate" "$sparkle/Versions/B/Updater.app" "$sparkle" \
  "$app/Contents/Helpers/obojeno" "$app"; do
  codesign --force "${sign[@]}" "$part"
done

codesign --verify --strict --deep "$app"
if [[ "${UNIVERSAL:-}" == 1 ]]; then
  for binary in "$app/Contents/MacOS/Obojeno" "$app/Contents/Helpers/obojeno" "$sparkle/Versions/B/Sparkle"; do
    archs="$(lipo -archs "$binary" | tr ' ' '\n' | sort | xargs)"
    if [[ "$archs" != "arm64 x86_64" ]]; then
      echo "$binary is $archs, not universal" >&2
      exit 1
    fi
  done
fi
echo "built $app"
