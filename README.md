# rust-wallet

A noncustodial Lightning wallet in **Rust**, over **Lightning Labs' Lightning
Terminal (litd)**: LND + Loop + Pool + Faraday + Taproot Assets in one
binary. It has a desktop app (Tauri 2), a hosted web demo (the same UI in
WebAssembly), and a CLI.

It is the Rust sibling of
[Sovereign Wallet](https://github.com/pauloes-btechs/sovereign-wallet)
(TypeScript/React) and carries over its safety invariants, payment-intent
engine and regtest-first workflow. A Go version may follow.

| Surface | What it is | Where it runs |
| --- | --- | --- |
| **Desktop app** | Tauri 2 shell + Leptos UI; connects to your LND/litd or runs the bundled litd on loopback | Your machine; installers on GitHub Releases |
| **Web demo** | The same Leptos UI compiled to WASM, replaying a real litd regtest session | Vercel (static, no backend, no env vars) |
| **CLI** | `rw`: every wallet action from a terminal | Anywhere Rust builds |

## Layout

```
crates/
  rw-core/      View contract, capabilities, destination gate, mainnet guard (wasm-safe)
  rw-payments/  Payment-intent state machine + SQLite journal
  rw-litd/      tonic gRPC over litd/LND, pinned TLS, macaroons, sidecar
  rw-replay/    NodeApi over recorded litd regtest responses (demo + contract test)
  rw-cli/       `rw`
apps/desktop/
  ui/           Leptos 0.8 (CSR → WASM), built with Trunk
  src-tauri/    Tauri 2 shell (rw-desktop)
proto/          Lightning Labs protos, vendored at litd v0.17.8 (see proto/upstream.json)
harness/regtest Docker: bitcoind + 2× LND + litd, bootstrap script
xtask/          fetch-sidecars (SHA-256 verified), externalize-boot (strict CSP)
docs/           Architecture, payments, security, hosting, local dev, release, upstream
```

## Quick start

```sh
cargo test                                   # 60+ unit, contract and CLI tests; no node needed
cargo run -p rw-cli -- --replay balance      # CLI over the recorded litd session

# live, against the regtest harness (Docker)
harness/regtest/scripts/bootstrap.sh && source harness/regtest/.env.regtest
cargo test -p rw-litd --features regtest -- --test-threads=1
cargo run -p rw-cli -- --endpoint $RW_ALICE_ENDPOINT --tls-cert $RW_ALICE_CERT --macaroon $RW_ALICE_MACAROON balance

# web demo / desktop UI
rustup target add wasm32-unknown-unknown && cargo install trunk --locked
cd apps/desktop/ui && trunk serve            # http://127.0.0.1:1420 (demo mode in a browser)

# desktop app
cargo install tauri-cli --version '^2' --locked
cargo xtask fetch-sidecars                   # bundled litd, hash-verified
cd apps/desktop/src-tauri && cargo tauri dev
```

The full Mac setup (external SSD notes included) is in
[docs/LOCAL_DEV_MAC.md](docs/LOCAL_DEV_MAC.md).

## Safety invariants

- **Never pay twice.** A payment intent is persisted before it is signed,
  and persisted in-flight (with its payment hash, or a unique on-chain tx
  label) before the network call. On restart, interrupted intents are
  *looked up* on the node and reconciled; recovery never re-sends. Proven
  against real LNDs in CI ([docs/PAYMENTS.md](docs/PAYMENTS.md)).
- **Mainnet is off** until an explicit product sign-off; autonomous tests are
  regtest only. The guard runs on the node-reported network.
- **Exact TLS pin** on the node's certificate; macaroons stay in the Rust
  process; every sidecar listener is 127.0.0.1.
- **Supply chain:** the litd sidecar is fetched only if its SHA-256 matches
  the signed upstream manifest; protos are vendored byte-for-byte.
- **No telemetry, no accounts.** The hosted demo makes no network calls.

## Documentation

- [Architecture](docs/ARCHITECTURE.md): crates, process model, data flow
- [Payments](docs/PAYMENTS.md): the intent engine and its proofs
- [Security](docs/SECURITY.md): trust boundaries, CSP, secrets
- [Hosting](docs/HOSTING.md): Vercel demo, GitHub Releases, what costs money
- [Local dev on macOS](docs/LOCAL_DEV_MAC.md): toolchain, SSD caveats, change → ship loop
- [Release](docs/RELEASE.md): installers and signing
- [Upstream](docs/UPSTREAM.md): pins, weekly watch, re-pinning
- [Test results](docs/TEST_RESULTS.md): live regtest evidence

MIT licensed.
