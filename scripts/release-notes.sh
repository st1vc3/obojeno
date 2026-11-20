#!/usr/bin/env bash
# Prints the release notes for TAG in Markdown: the commits since the
# previous release tag, without the version bumps.
#
#   scripts/release-notes.sh v0.2.0
set -euo pipefail

tag="$1"
previous="$(git describe --tags --abbrev=0 --match 'v*' "$tag^" 2>/dev/null || true)"
range="${previous:+$previous..}$tag"
echo "## Changes"
echo
git log --no-merges --reverse --pretty='* %s' "$range" | grep -v '^\* Release [0-9]' || true
if [[ -n "$previous" ]]; then
  echo
  echo "**Full changelog**: https://github.com/st1vc3/obojeno/compare/$previous...$tag"
fi
