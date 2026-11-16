# Architecture

## One contract, two backends, three surfaces

```
                ┌──────────── rw-core ────────────┐
                │ views (wire contract) · NodeApi │
                │ capabilities · destination gate │
                └───────┬───────────────┬─────────┘
                        │               │
              rw-litd (live)      rw-replay (recorded)
         tonic gRPC → litd/LND    litd v0.17.8 regtest JSON
                        │               │
     ┌──────────────────┼───────┐       │
     │                  │       │       │
  rw-cli            rw-desktop  │     rw-ui in a browser
  (`rw`)            (Tauri cmds)│     (hosted demo)
                        ▲       │
                        │ IPC   │
                     rw-ui (Leptos/WASM) inside Tauri
```

- **`rw-core`** has no I/O and compiles to `wasm32-unknown-unknown`. Its
  views are the JSON wire contract: u64 sats travel as decimal strings so
  nothing above 2^53 loses precision in JS/WASM. A backend *declares* its
  [`CapabilitySet`]; the UI shows only declared tabs, and undeclared calls
  fail with `Unsupported` rather than half-working.
- **`rw-litd`** generates tonic clients from the vendored protos
  (`lnrpc`, `routerrpc`, `walletunlocker`, `litrpc` status, `taprpc`,
  `looprpc`, `frdrpc`) with a vendored `protoc`, so no system tools are
  needed. Mapping is pure functions in `map.rs` (unit-tested).
- **`rw-replay`** answers from real litd responses recorded on regtest for
  Sovereign Wallet's demo. Its contract test fails CI if any recorded shape
  stops deserializing into `rw-core` views.
- **`rw-payments`** is runtime-agnostic (no tokio), so the *same* engine runs
  natively over LND and in the browser over the replay dispatcher.

## Process model (desktop)

```
┌───────────── rust-wallet (Tauri) ─────────────┐
│ webview: rw-ui (WASM)  ──IPC──▶  Rust commands │──gRPC/TLS(pinned)──▶ litd sidecar (127.0.0.1)
│ CSP: no remote origins          rw-litd        │                       or your own LND/litd
│                                 SQLite journal │
└────────────────────────────────────────────────┘
```

- The webview holds no secrets. Macaroon paths, the litd UI password
  (random, `0600`) and the gRPC connection live in the Rust process.
- **Sidecar mode** launches the bundled litd with `--lnd-mode=integrated`,
  `--network=<regtest|signet>`, Neutrino backend, every listener on
  127.0.0.1 (LND gRPC on 10019 so a node on 10009 is undisturbed). First
  run creates the wallet through the unlocker (24-word aezeed shown once).
- **Remote mode** connects to `endpoint` + `tls.cert` + `admin.macaroon`.
- The last profile (paths only) is saved to `connection.json` and restored
  at launch.

## Why litd over gRPC (and not an embedded Rust node)

Lightning Labs' stack is Go. The Rust-native alternative (LDK + BDK) would
not exercise litd, Loop, Pool, Taproot Assets or Faraday. rust-wallet keeps
the node in Lightning Labs' software and makes Rust the client, shell and
UI, the same split Sovereign Wallet's advanced mode uses.

## Web demo

The Leptos bundle detects `window.__TAURI__`. Absent, it answers from
`rw-replay` and runs sends through `rw-payments` + `ReplayDispatcher`, so the
Journal shows genuine intent transitions. No network, no keys.
