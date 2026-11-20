#!/usr/bin/env bash
# Prints the release notes for TAG in Markdown: the commits since the
# previous release tag, without the version bumps, without changes that
# were reverted in the same range, and without commits that only touch
# documentation (README, docs/ media), which ship nothing.
#
#   scripts/release-notes.sh v0.2.0
set -euo pipefail

tag="$1"
previous="$(git describe --tags --abbrev=0 --match 'v*' "$tag^" 2>/dev/null || true)"
range="${previous:+$previous..}$tag"

# Reverts within the range, and the commits they revert.
skip=" "
while read -r revert; do
  reverted="$(git log -1 --format=%B "$revert" | sed -n 's/^This reverts commit \([0-9a-f]\{40\}\).*/\1/p')"
  if [[ -n "$reverted" ]] && git merge-base --is-ancestor "$reverted" "$tag" \
    && { [[ -z "$previous" ]] || ! git merge-base --is-ancestor "$reverted" "$previous"; }; then
    skip+="$revert $reverted "
  fi
done < <(git log --no-merges --format=%H --grep='^This reverts commit ' "$range")

echo "## Changes"
echo
git log --no-merges --reverse --format='%H %s' "$range" | while read -r commit subject; do
  [[ "$skip" == *" $commit "* ]] && continue
  [[ "$subject" =~ ^Release\ [0-9] ]] && continue
  if ! git diff-tree --no-commit-id --name-only -r "$commit" | grep -qvE '^(docs/|[^/]+\.md$)'; then
    continue
  fi
  echo "* $subject"
done
if [[ -n "$previous" ]]; then
  echo
  echo "**Full changelog**: https://github.com/st1vc3/obojeno/compare/$previous...$tag"
fi
