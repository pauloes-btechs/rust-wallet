# rust-wallet: working agreements

- Git authorship for every commit: `pauloes <pauloes@btechs.io>`.
- Work on a branch, open a PR, merge only when **CI** and **Regtest** are green.
  Resolve issues yourself (push branches, open PRs); don't hand the owner
  terminal scripts to run.
- Before pushing: `cargo fmt --all`, `cargo clippy --workspace --all-targets -- -D warnings`, `cargo test`.
- Never weaken a safety invariant (README "Safety invariants"):
  persist-before-dispatch, reconcile-never-re-dispatch, mainnet guard,
  exact TLS pin, loopback-only sidecar, no telemetry, static demo with no env vars.
- Lightning Labs facts (RPC names, fields, flags, defaults): take them from
  the vendored `proto/` files or the Lightning KB (`/Volumes/Extreme SSD/lightning-kb`),
  not from memory.
- Autonomous financial testing: regtest only (signet with sign-off). Never mainnet.
- The view contract (`rw-core/src/views.rs`) must keep deserializing the
  recorded litd responses (`rw-replay` contract test).
