# Security

## Trust boundaries

| Boundary | Control |
| --- | --- |
| Rust ↔ node | TLS pinned to the exact `tls.cert` DER (`rw-litd/src/tls.rs`); handshake signatures still verified. LND's cert is CA-flagged, which webpki rejects as an end entity, so pinning is stricter than disabling checks, not looser. Tested with an rcgen CA-flagged cert. |
| Auth | Macaroon read from disk per connection, sent as gRPC metadata. Never crosses into the webview, never logged. |
| Webview ↔ Rust | Tauri IPC; capability `core:default` only (no shell, fs, http plugins). App commands validate input in Rust. |
| Webview content | CSP: `script-src 'self' 'wasm-unsafe-eval'`, `connect-src ipc:` only; `freezePrototype`. |
| Sidecar | Every listener 127.0.0.1; UI password random per install, file mode 0600; mainnet sidecar refused. |
| Supply chain | litd fetched only if SHA-256 matches `sidecars.lock.json` (copied from the signed release manifest); protos vendored byte-for-byte with provenance in `proto/upstream.json`; `Cargo.lock` committed. |
| Web demo | Static, no backend, no env vars. CSP `default-src 'none'`, `script-src 'self' 'wasm-unsafe-eval'`, `style-src 'self'`, `connect-src 'self'`, `frame-ancestors 'none'`; no inline scripts (Trunk's loader is externalized at build). |

## Input gates

- `rw_core::parse_destination` checksum-verifies BOLT-11 (bech32 without the
  90-char limit) and segwit/taproot addresses and rejects a destination for
  the wrong network *before* an intent is authorized.
- The mainnet guard checks the node-reported network on connect and before
  every write.

## Secrets inventory

| Secret | Where | Notes |
| --- | --- | --- |
| admin.macaroon | node data dir | path only in `connection.json` |
| litd UI password | app data dir, 0600 | random, never shown |
| Wallet password | typed by the user | passed to UnlockWallet, not stored |
| Aezeed | shown once at creation | never stored by the app |
| Regtest creds | `harness/regtest/creds/` | throwaway, gitignored |

Nothing in this repo needs a `.env`. Never put a secret in a Vercel env var:
the demo is a static WASM bundle anyone can read.

## Reporting

Open a private security advisory on the GitHub repository.
