#!/usr/bin/env bash
# Vercel install step: a Rust toolchain with the wasm32 target, and trunk.
# Vercel's build image has no Rust, so this runs on every build (~1 min).
set -euo pipefail
if ! command -v rustup > /dev/null 2>&1 && [ ! -x "$HOME/.cargo/bin/rustup" ]; then
  curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y --profile minimal --default-toolchain stable
fi
. "$HOME/.cargo/env"
rustup target add wasm32-unknown-unknown
if ! command -v trunk > /dev/null 2>&1; then
  arch=$(uname -m)
  curl -sSL "https://github.com/trunk-rs/trunk/releases/download/v0.22.1/trunk-${arch}-unknown-linux-gnu.tar.gz" | tar -xz -C "$HOME/.cargo/bin" trunk
fi
trunk --version
