#!/usr/bin/env bash
# Build the Leptos UI (WASM) to static files.
#
#   scripts/build-web.sh            → ./dist            (the hosted demo; what Vercel serves)
#   scripts/build-web.sh desktop    → apps/desktop/ui/dist (Tauri's frontendDist)
#
# Needs: rustup target wasm32-unknown-unknown, and trunk (see docs/LOCAL_DEV_MAC.md).
set -euo pipefail
root=$(cd "$(dirname "$0")/.." && pwd)
[ -f "$HOME/.cargo/env" ] && . "$HOME/.cargo/env"

cd "$root/apps/desktop/ui"
trunk build --release
# Strict CSP: move Trunk's inline module loader into a hashed file.
cargo run --quiet --manifest-path "$root/Cargo.toml" -p xtask -- externalize-boot "$root/apps/desktop/ui/dist"

if [ "${1:-web}" = "web" ]; then
  rm -rf "$root/dist"
  cp -R "$root/apps/desktop/ui/dist" "$root/dist"
  echo "web demo → $root/dist"
fi
