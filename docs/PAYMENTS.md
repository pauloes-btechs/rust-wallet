# Payments: never pay twice

`rw-payments` ports Sovereign Wallet's payment-intent state machine.

```
Draft ─authorize─▶ Authorized ─sign─▶ Signing ─dispatch─▶ InFlight ─┬─▶ Settled
  │                                      │                          ├─▶ Failed (incl. not found)
  └──fail──▶ Failed                      └──────── reconcile ───────┴─▶ NeedsRecovery ─resolve─▶ Settled | Failed
```

## Rules

1. **Persist before dispatch.** `Authorized` is saved before signing;
   `InFlight` with its *backend reference* is saved before the network call.
   The SQLite store runs WAL with `synchronous=FULL`, so a save is on disk
   when it returns.
2. **Reconcile, never re-dispatch.** On startup (and after unlock),
   `recover_interrupted` looks every `Signing`/`InFlight` intent up. It has
   no code path to `dispatch`. An intent the node never saw (not found, or
   still `Signing`) becomes `Failed` ("never reached node"); the user sends
   again as a new intent.
3. **Ambiguity stays ambiguous.** A timeout, a dropped stream, or LND saying
   "already paid / in transition" leaves the intent `InFlight`. Only a
   definitive answer settles or fails it. An unknown lookup becomes
   `NeedsRecovery`, which only `rw resolve` (a human) can close.
4. **Recovery waits for an unlocked wallet.** A lookup against a locked node
   would wrongly escalate in-flight intents.

## Backend references

| Kind | Reference | How it's known before sending | Lookup |
| --- | --- | --- | --- |
| Lightning | payment hash | `DecodePayReq` | `Router.TrackPaymentV2` |
| On-chain | tx label `rw-intent:<id>` | generated | `GetTransactions`, match label |

LND's `SendCoins` builds, signs and broadcasts in one call, so a txid isn't
known in advance. The unique label is persisted first and attached to the
transaction, which makes on-chain sends reconcilable too.
`SendCoins` has no idempotency key, so while any on-chain intent of the
node is `InFlight` or `NeedsRecovery`, a new on-chain send is refused until
recovery (on reconnect, or `rw recover`) or `rw resolve` closes it. A label
that is not (yet) in `GetTransactions` is ambiguous, so it becomes
`NeedsRecovery`, never "never reached node".

## Fee policy

Lightning: the routing-fee cap defaults to 1% of the amount (minimum 10
sats) and is passed to LND as `fee_limit_sat`. The UI and CLI show it before
confirmation. On-chain: the user's sat/vB if given, otherwise LND's estimate
for 6 blocks; the fee is shown before confirmation.

## Proofs

- `crates/rw-payments/tests/engine.rs`: persist-before-dispatch (the store is
  read *inside* dispatch), double execute refused, crash-after-send
  reconciled with SQLite across a dropped engine, not-found fails the intent,
  policy rejection fails before signing.
- `crates/rw-litd/tests/regtest.rs`, against two real LNDs in CI:
  - paying the same invoice twice: one payment, second intent reconciled to
    settled;
  - crash after send: a dispatcher that sends then "dies"; reopening the
    backend reconciles the intent, exactly one payment exists for the hash;
  - on-chain send found again by its label;
  - a mainnet invoice is refused and nothing is persisted.
