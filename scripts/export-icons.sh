#!/usr/bin/env bash
# Rebuild every derived Haystack app-icon asset from haystack-icon.svg.
# cargo-packager converts the committed PNGs into the .icns inside the macOS
# .app bundle, so no platform-specific bundle artwork is hand-maintained here.
set -euo pipefail
repo_root="$(cd "$(dirname "$0")/.." && pwd)"
cd "$repo_root"
cargo run --quiet --example export_icons
