//! # rw-core
//!
//! The contract every other rust-wallet crate builds on:
//!
//! * [`views`]: the JSON wire contract between a node backend and the UI.
//!   Every sats amount that can exceed 2^53 is a decimal string on the wire
//!   ([`decstr`]), so a JavaScript or WASM consumer never loses precision.
//!   The shapes match the litd responses recorded on regtest in
//!   `crates/rw-replay/fixtures/litd-regtest.json`; a contract test in
//!   `rw-replay` fails CI if they drift.
//! * [`capability`]: a backend *declares* what it can do and the UI renders
//!   only that. Nothing is inferred from the network or guessed at runtime.
//! * [`destination`]: offline validation of BOLT-11 invoices and on-chain
//!   addresses, including the network-mismatch check, before anything is
//!   authorized.
//! * [`network`]: the mainnet guard. Mainnet is refused unless explicitly
//!   enabled (product sign-off), and autonomous tests never leave regtest.
//! * [`api::NodeApi`]: the async surface implemented by the live litd backend
//!   (`rw-litd`) and the recorded replay backend (`rw-replay`).
//!
//! This crate has no I/O and compiles to `wasm32-unknown-unknown`.

#![forbid(unsafe_code)]

pub mod api;
pub mod capability;
pub mod decstr;
pub mod destination;
pub mod error;
pub mod network;
pub mod sats;
pub mod views;

pub use api::NodeApi;
pub use capability::{Capability, CapabilitySet};
pub use destination::{parse_destination, Destination};
pub use error::{ApiError, ApiResult};
pub use network::{MainnetPolicy, Network};
pub use sats::Sats;
