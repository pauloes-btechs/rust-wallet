# Local development on macOS (external SSD)

Running from `/Volumes/Extreme SSD/rust-wallet` and shipping to GitHub
(and from there to Vercel and GitHub Releases).

## 0. The drive

Cargo is happy on any file system, but **exFAT** has two traps: it sprinkles
`._*` AppleDouble files (ignored by `.gitignore`) and it is slow for
`target/`'s many small files. Keep builds fast by putting the target
directory on the internal disk:

```sh
diskutil info "/Volumes/Extreme SSD" | grep "File System Personality"
# exFAT? then, in ~/.zshrc:
export CARGO_TARGET_DIR="$HOME/.cache/rust-wallet-target"
```

APFS needs nothing special.

## 1. Toolchain (once per Mac)

```sh
xcode-select --install
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh     # rustup reads rust-toolchain.toml
rustup target add wasm32-unknown-unknown
cargo install trunk --locked                                       # or: brew install trunk
cargo install tauri-cli --version '^2' --locked                    # `cargo tauri dev|build`
brew install --cask docker                                         # regtest harness (or Docker Desktop)
```

No Node, no protoc: protobuf codegen uses a vendored `protoc`.

## 2. Clone / update

```sh
cd "/Volumes/Extreme SSD/rust-wallet"
git pull
cargo test                                    # should be all green
```

## 3. Run things

| What | Command | Opens |
| --- | --- | --- |
| CLI over the recorded session | `cargo run -p rw-cli -- --replay status` | terminal |
| Web demo (hot reload) | `cd apps/desktop/ui && trunk serve` | http://127.0.0.1:1420 |
| Web demo, production build | `scripts/build-web.sh` then `python3 -m http.server -d dist 8000` | http://localhost:8000 |
| Desktop app | `cargo xtask fetch-sidecars` once, then `cd apps/desktop/src-tauri && cargo tauri dev` | native window |
| Desktop installer (unsigned) | `cd apps/desktop/src-tauri && cargo tauri build` | `target/release/bundle/` |
| Regtest harness | `harness/regtest/scripts/bootstrap.sh && source harness/regtest/.env.regtest` | Docker |
| Live tests | `cargo test -p rw-litd --features regtest -- --test-threads=1` | terminal |
| CLI against alice | `cargo run -p rw-cli -- --endpoint $RW_ALICE_ENDPOINT --tls-cert $RW_ALICE_CERT --macaroon $RW_ALICE_MACAROON balance` | terminal |

Desktop against the harness: *Connect to your node* with endpoint
`127.0.0.1:10109`, and the cert/macaroon paths printed in
`harness/regtest/.env.regtest`.

## 4. Change → ship

```sh
git checkout -b feat/my-change
# …edit…
cargo fmt && cargo clippy --workspace --all-targets -- -D warnings && cargo test
git commit -am "feat: …" && git push -u origin feat/my-change
# open the PR on github.com
```

- CI runs fmt, clippy, tests, the WASM build, the desktop build and the live
  regtest suite on the PR. Vercel comments a preview URL.
- Merge to `main` → Vercel updates production.
- Tag `vX.Y.Z` → installers on a draft GitHub Release.
