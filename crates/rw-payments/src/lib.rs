//! # rw-payments
//!
//! A Rust port of Sovereign Wallet's payment-intent state machine, with the
//! same spec-locked safety rules:
//!
//! 1. An intent is persisted in `Authorized` **before** anything is signed or
//!    sent, and persisted `InFlight` (with its backend reference, the payment
//!    hash or txid) **before** the network call.
//! 2. On startup, every intent found `Signing` or `InFlight` is reconciled
//!    against the backend with a *lookup*. Recovery never calls `dispatch`.
//!    Blind re-dispatch is a test-failing bug, not a style choice.
//! 3. An ambiguous outcome (sent, timed out) leaves the intent `InFlight`.
//!    An unknown lookup result becomes `NeedsRecovery`, which only a human
//!    (or a dedicated recovery flow) can resolve.
//!
//! ```text
//! Draft ─authorize─▶ Authorized ─sign─▶ Signing ─dispatch─▶ InFlight ─┬─▶ Settled
//!   │                                      │                          ├─▶ Failed (incl. not found)
//!   └──fail──▶ Failed                      └──────── reconcile ───────┴─▶ NeedsRecovery ─resolve─▶ Settled | Failed
//! ```
//!
//! The engine is runtime-agnostic (no tokio), so it also runs in the WASM
//! web demo over an in-memory store.

#![forbid(unsafe_code)]

mod engine;
mod intent;
mod store;

pub use engine::{DispatchOutcome, Dispatcher, EngineError, PaymentEngine, RecoveryReport};
pub use intent::{
    Attempt, AttemptResult, IntentState, InvalidTransition, PaymentIntent, PaymentKind, ReconcileResult,
    RecoveryOutcome, NEVER_REACHED_NODE,
};
#[cfg(feature = "sqlite")]
pub use store::SqliteStore;
pub use store::{IntentStore, MemoryStore, StoreError};
