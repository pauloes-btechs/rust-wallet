# Upstream (Lightning Labs) pins

| Pin | Where | Current |
| --- | --- | --- |
| litd release + per-platform SHA-256 | `sidecars.lock.json` | v0.17.8 (bundles LND v0.21.5-beta.rc1, tapd v0.8.6, Loop, Pool, Faraday) |
| Protos | `proto/*.proto`, provenance in `proto/upstream.json` | byte-identical to the module versions litd v0.17.8 pins (imports flattened) |
| Harness images | `harness/regtest/docker-compose.yml` | `lightninglabs/lnd:v0.21.5-beta.rc1`, `lightninglabs/lightning-terminal:v0.17.8`, `bitcoin/bitcoin:27.0` |
| Demo recording | `crates/rw-replay/fixtures/litd-regtest.json` | litd v0.17.8 on regtest, recorded 2026-11-05 for Sovereign Wallet |

These were carried over from Sovereign Wallet, where they were verified
against the signed release manifest and upstream sources.

## Weekly watch

`.github/workflows/upstream-watch.yml` runs Mondays. If the latest litd
release differs from the pin, it opens an issue and dispatches the Regtest
workflow against the latest LND and litd images. Nothing is re-pinned
automatically.

## Re-pinning

1. From the new litd release, copy each platform's SHA-256 out of the
   **signed** `manifest-<tag>.txt` into `sidecars.lock.json` (verify the
   signature first). Never compute a hash from a download and pin that.
2. Re-vendor the protos listed in `proto/upstream.json` at the module
   versions the new litd's `go.mod` pins.
3. Update the image tags in the harness compose file.
4. `cargo test` (the contract test catches view drift), then let CI and
   Regtest run on the PR.
5. Optionally refresh the demo recording.
