#!/usr/bin/env bash
# Prints obojeno's version, from the workspace Cargo.toml.
set -euo pipefail
sed -n 's/^version = "\(.*\)"/\1/p' "$(dirname "$0")/../Cargo.toml" | head -1
