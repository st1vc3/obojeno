#!/usr/bin/env bash
# Prints the Homebrew cask for a release:
#
#   scripts/cask.sh VERSION SHA256 [--notarized]
#
# Without --notarized the app is only signed ad hoc, so the cask removes
# the quarantine flag that would make Gatekeeper refuse to open it.
set -euo pipefail

if [[ $# -lt 2 ]]; then
  echo "usage: $0 VERSION SHA256 [--notarized]" >&2
  exit 2
fi
version="$1"
sha256="$2"
notarized="${3:-}"

cat <<CASK
cask "obojeno" do
  version "$version"
  sha256 "$sha256"

  url "https://github.com/st1vc3/obojeno/releases/download/v#{version}/Obojeno-#{version}.zip"
  name "obojeno"
  desc "Wallpaper-driven Material You theming for macOS"
  homepage "https://github.com/st1vc3/obojeno"

  livecheck do
    url :url
    strategy :github_latest
  end

  depends_on macos: ">= :tahoe"

  app "Obojeno.app"
  binary "#{appdir}/Obojeno.app/Contents/Helpers/obojeno"
CASK

if [[ "$notarized" != "--notarized" ]]; then
  cat <<'CASK'

  # Signed ad hoc, not notarized.
  postflight do
    system_command "/usr/bin/xattr",
                   args: ["-dr", "com.apple.quarantine", "#{appdir}/Obojeno.app"]
  end
CASK
fi

cat <<'CASK'

  uninstall launchctl: "io.github.st1vc3.obojeno",
            quit:      "io.github.st1vc3.obojeno",
            script:    {
              executable:   "/usr/bin/pkill",
              args:         ["-f", "Obojeno.app/Contents/Helpers/obojeno daemon"],
              must_succeed: false,
            }

  # The library of imported wallpapers (~/Pictures/obojeno) is yours and stays.
  zap trash: [
    "~/.cache/obojeno",
    "~/.config/obojeno",
    "~/.local/state/obojeno",
    "~/Library/Logs/obojeno.log",
    "~/Library/Preferences/io.github.st1vc3.obojeno.plist",
  ]
end
CASK
