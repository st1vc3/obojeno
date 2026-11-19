#!/usr/bin/env bash
# Style-checks a cask file the way Homebrew checks casks in a tap (a loose
# file is held to the rules for Homebrew's own code instead):
#
#   scripts/check-cask.sh Casks/obojeno.rb
set -euo pipefail

cask="$1"
tap=obojeno/check
if ! brew tap | grep -qx "$tap"; then
  brew tap-new --no-git "$tap" >/dev/null
fi
dir="$(brew --repository "$tap")/Casks"
mkdir -p "$dir"
cp "$cask" "$dir/obojeno.rb"
brew style "$tap/obojeno"
