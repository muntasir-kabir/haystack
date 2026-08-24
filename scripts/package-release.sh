#!/usr/bin/env bash
# Build the release binary for the current platform, then wrap it into native
# installers with cargo-packager. Usage:
#   ./scripts/package-release.sh            # auto-detect host platform
#   ./scripts/package-release.sh <target>   # e.g. x86_64-pc-windows-msvc
#
# Formats chosen per platform:
#   bsd*|darwin*  → app,dmg
#   linux*        → deb,appimage
#   msys*|cygwin* → nsis
set -euo pipefail

HOST_TRIPLE="${1:-$(rustc -vV | sed -n 's/^host: //p')}"

case "$HOST_TRIPLE" in
  *-apple-darwin)  FORMATS="app,dmg" ;;
  *-pc-windows-*)  FORMATS="nsis" ;;
  *-unknown-linux*) FORMATS="deb,appimage" ;;
  *) echo "Unknown platform '$HOST_TRIPLE' — pass a target explicitly." >&2; exit 1 ;;
esac

echo "==> Building release binary for $HOST_TRIPLE"
cargo build --release --target "$HOST_TRIPLE"

if ! command -v cargo-packager >/dev/null 2>&1; then
  echo "==> Installing cargo-packager"
  cargo install cargo-packager --locked
fi

echo "==> Packaging installers ($FORMATS)"
cargo packager --release --formats "$FORMATS" --target "$HOST_TRIPLE"

echo "==> Done. Installers:"
ls -1 "target/$HOST_TRIPLE/release/" | grep -Ei '\.(exe|deb|dmg|AppImage)$' \
  || ls -1 "target/release/" | grep -Ei '\.(exe|deb|dmg|AppImage)$'