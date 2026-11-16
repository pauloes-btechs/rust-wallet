#!/usr/bin/env bash
# Vercel install step: Rust with the wasm32 target, and trunk.
# Works whether or not the build image already ships rustup.
set -euo pipefail
root=$(cd "$(dirname "$0")/.." && pwd)
[ -f "$HOME/.cargo/env" ] && . "$HOME/.cargo/env"
if ! command -v rustup > /dev/null 2>&1; then
  curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y --profile minimal --default-toolchain stable
  . "$HOME/.cargo/env"
fi
rustup show active-toolchain || rustup toolchain install stable --profile minimal
rustup target add wasm32-unknown-unknown
# trunk goes in the repo-local .bin (on PATH in build-web.sh), not a guessed CARGO_HOME.
mkdir -p "$root/.bin"
if [ ! -x "$root/.bin/trunk" ]; then
  arch=$(uname -m)
  curl -sSL "https://github.com/trunk-rs/trunk/releases/download/v0.22.1/trunk-${arch}-unknown-linux-gnu.tar.gz" | tar -xz -C "$root/.bin" trunk
fi
"$root/.bin/trunk" --version
cargo --version
