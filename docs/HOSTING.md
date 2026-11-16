# Hosting & distribution

A noncustodial wallet has no server to host: keys, macaroons and node state
live on the user's machine. "Hosting" means a public demo, a place for
installers, and CI. Everything below is free except Apple signing.

| Surface | Where | Cost | Secrets |
| --- | --- | --- | --- |
| Web demo (Leptos/WASM, replay, regtest) | **Vercel** project `rust-wallet` | Free (Hobby) | None |
| Desktop installers | **GitHub Releases** via `release.yml` | Free | Signing certs later |
| CI: fmt, clippy, tests, WASM, desktop, live regtest | **GitHub Actions** | Free (public repo) | None |
| Weekly upstream watch | GitHub Actions (`upstream-watch.yml`) | Free | None |
| Local Lightning dev | Docker (`harness/regtest`) or **Polar** | Free | Throwaway |

Apple Developer Program ($99/yr) is the one paid item: it covers macOS
notarization (and iOS, if a mobile app follows).

## Web demo on Vercel

`vercel.json` (repo root) is the whole configuration:

- `installCommand`: `scripts/vercel-install.sh` installs a minimal Rust
  toolchain, the `wasm32-unknown-unknown` target and Trunk (Vercel's image
  has no Rust). About a minute.
- `buildCommand`: `scripts/build-web.sh` → `trunk build --release` →
  `cargo xtask externalize-boot` → `dist/`.
- Security headers: strict CSP (no inline scripts or styles), `no-referrer`,
  `nosniff`, COOP, locked-down `Permissions-Policy`, `noindex`. Hashed
  assets get `immutable` caching.
- **Environment variables: none.** The demo is static and makes no network
  calls. Never add a secret: anything in a WASM bundle is public.

One-time setup (already done for `btechs-org`): import
`pauloes-btechs/rust-wallet` in Vercel with Root Directory = repo root and
no framework preset. Every push to `main` deploys production; every PR gets
a preview URL.

The CI `wasm` job runs the exact same build script, so a green check means
the Vercel build will succeed too.

**Alternatives:** Cloudflare Pages (same build command, headers in
`_headers`, unlimited bandwidth) or GitHub Pages (no custom headers, so the
CSP would be weaker).

## Desktop: GitHub Releases

Push a tag `vX.Y.Z` (or run *Release* manually). `release.yml` builds
unsigned installers for macOS arm64 and x64 (`.dmg`, `.app`), Windows
(NSIS `.exe`, `.msi`) and Linux (`.deb`, `.AppImage`), each with its own
hash-verified litd sidecar, and attaches them to a **draft** release. See
[RELEASE.md](RELEASE.md) for signing.

## Local Lightning

- `harness/regtest`: Docker Compose (bitcoind 27, LND v0.21.5-beta.rc1 ×2,
  litd v0.17.8). Ports are Sovereign's +100, so both harnesses can run at
  once. `scripts/bootstrap.sh` funds alice and opens a channel to bob.
- **Polar** (lightningpolar.com): click-to-create regtest networks; point
  the desktop app's "Connect to your node" at a Polar LND.
- **Signet** next, for testing with other people. Mainnet stays off.
