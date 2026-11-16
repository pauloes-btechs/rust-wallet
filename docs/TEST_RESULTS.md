# Test results

## Suites

| Suite | Where | Count | Needs |
| --- | --- | --- | --- |
| Unit: rw-core (contract, sats, destination gate, mainnet guard) | `cargo test` | 17 | nothing |
| Unit + engine: rw-payments (state machine, crash recovery, SQLite) | `cargo test` | 15 | nothing |
| Unit: rw-litd (mappers, dispatcher classification, TLS pin, sidecar args) | `cargo test` | 16 | nothing |
| Contract: rw-replay (every recorded litd route ↔ rw-core views) | `cargo test` | 4 | nothing |
| CLI end-to-end over the recording | `cargo test -p rw-cli` | 6 | nothing |
| Desktop shell (profile, secrets, paths) | `cargo test -p rw-desktop` | 3 | webkit (Linux) |
| xtask (strict-CSP boot externalizer) | `cargo test -p xtask` | 1 | nothing |
| **Live regtest** against 2× LND v0.21.5-beta.rc1 + litd v0.17.8 | `Regtest` workflow | 7 | Docker |

## Live regtest evidence (GitHub Actions)

From run [43922158638](https://github.com/pauloes-btechs/rust-wallet/actions/runs/43922158638)
(PR #1) and run [43925258667](https://github.com/pauloes-btechs/rust-wallet/actions/runs/43925258667) (PR #2):

```
test crash_after_send_is_reconciled_on_restart_not_resent ... ok
test litd_embedded_lnd_is_ready ... ok
test onchain_send_is_reconcilable_by_label ... ok
test pay_settles_and_a_second_pay_never_pays_twice ... ok
test read_surfaces_answer ... ok
test state_is_ready_on_regtest ... ok
test wrong_network_invoice_is_refused_before_authorization ... ok
test result: ok. 7 passed; 0 failed
```

### A real failure the harness caught

The first live run (PR #1, run 43921028156) failed two payment tests with
`insufficient_balance`. LND logs showed the channel link had come up
(`bandwidth=882000000 mSAT`) 0.7 s before the tests, but the router still
saw `local balance: 0 mSAT`. The wallet behaved correctly (a definitive
failure recorded as `Failed`, nothing re-sent); the harness was racing.
Fix: bootstrap now waits for `queryroutes` to find a route
(commit a7c21cc).

## Not yet covered

- Desktop sidecar mode end-to-end (spawn litd → create wallet → pay) needs a
  Neutrino-serving peer in CI (Sovereign uses a host-side btcd); planned.
- Installer smoke tests per OS (`release.yml` builds them; launching them in
  CI is next).
- Signet.
