# Release

## Artifacts (`.github/workflows/release.yml`)

| Target | Runner | Bundles |
| --- | --- | --- |
| aarch64-apple-darwin | macos-latest | `.app`, `.dmg` |
| x86_64-apple-darwin | macos-latest (cross) | `.app`, `.dmg` |
| x86_64-pc-windows-msvc | windows-latest | NSIS `.exe`, `.msi` |
| x86_64-unknown-linux-gnu | ubuntu-22.04 | `.deb`, `.AppImage`, `.rpm` |

Each job fetches the litd sidecar for *its* target with
`cargo xtask fetch-sidecars --triple <target>`, which refuses to extract
anything whose SHA-256 differs from `sidecars.lock.json`. Tauri bundles it
via `bundle.externalBin`.

Releases are created as **drafts** and **prereleases**: review, then publish.

## Signing (pending credentials)

Unsigned builds work for development: on macOS, right-click → Open the
first time; on Windows, SmartScreen → More info → Run anyway.

When credentials exist, add these GitHub Actions secrets; `tauri-action`
picks them up:

| Secret | For |
| --- | --- |
| `APPLE_CERTIFICATE`, `APPLE_CERTIFICATE_PASSWORD` | Developer ID Application cert (base64 .p12) |
| `APPLE_SIGNING_IDENTITY` | e.g. `Developer ID Application: … (TEAMID)` |
| `APPLE_ID`, `APPLE_PASSWORD` (app-specific), `APPLE_TEAM_ID` | notarization |
| `TAURI_SIGNING_PRIVATE_KEY` (+ password) | optional auto-updater |
| Windows code-signing cert | SmartScreen reputation |

## Checklist

1. `main` green on CI and Regtest.
2. Bump `version` in `Cargo.toml` (workspace) and `apps/desktop/src-tauri/tauri.conf.json`.
3. Tag and push: `git tag v0.1.0 && git push origin v0.1.0`.
4. Download one installer per OS, open it, connect to the regtest harness.
5. Publish the draft.
