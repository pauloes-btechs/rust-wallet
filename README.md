# rust-wallet

A Lightning wallet in Rust over **Lightning Labs' litd** (LND + Loop + Pool +
Faraday + Taproot Assets), with a desktop app, a hosted web demo and a CLI.
It is the Rust sibling of [Sovereign Wallet](https://github.com/pauloes-btechs/sovereign-wallet)
and carries over its safety invariants.

> Work in progress: this first slice is the core, the gRPC backend, the CLI
> and the regtest harness. The desktop app and web demo land next.

```sh
cargo test                      # unit, contract and CLI tests (no node needed)
cargo run -p rw-cli -- --replay balance
```

## Safety invariants

- Payment intents are persisted **before** dispatch; recovery reconciles by
  payment hash (lightning) or transaction label (on-chain), never re-sends.
- Mainnet is refused unless explicitly enabled; autonomous tests are regtest only.
- The node's TLS certificate is pinned exactly; every sidecar listener is loopback.
- No telemetry, no accounts, no network calls from the hosted demo.

MIT licensed.
