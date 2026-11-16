//! # rw-litd
//!
//! The live backend: Rust gRPC (tonic) clients for Lightning Labs' **litd**
//! (Lightning Terminal: LND + Loop + Pool + Faraday + Taproot Assets in one
//! binary), generated from protos vendored byte-for-byte from the upstream
//! repos at the versions litd v0.17.8 pins.
//!
//! * [`connect`] pins the node's exact TLS certificate (LND's cert is
//!   self-signed and marked as a CA, which stock webpki rejects as an end
//!   entity) and attaches the macaroon to every call.
//! * [`LitdBackend`] implements [`rw_core::NodeApi`]. Lightning payments and
//!   on-chain sends run through the [`rw_payments`] engine with an
//!   [`LndDispatcher`]: persisted before dispatch, reconciled by payment hash
//!   or by transaction label after a crash, never blindly re-sent.
//! * [`sidecar`] launches a bundled `litd` bound to loopback only.
//!
//! The mainnet guard runs on connect against the node-reported network.

#![forbid(unsafe_code)]

mod backend;
mod client;
mod config;
mod dispatcher;
pub mod map;
pub mod sidecar;
mod tls;

pub use backend::LitdBackend;
pub use client::{connect, LitdClient};
pub use config::{ConfigError, ConnectConfig};
pub use dispatcher::{default_fee_cap_sat, LndDispatcher};

pub mod proto {
    #![allow(clippy::all, missing_docs, rustdoc::all)]
    pub mod lnrpc {
        tonic::include_proto!("lnrpc");
    }
    pub mod routerrpc {
        tonic::include_proto!("routerrpc");
    }
    pub mod litrpc {
        tonic::include_proto!("litrpc");
    }
    pub mod taprpc {
        tonic::include_proto!("taprpc");
    }
    pub mod looprpc {
        tonic::include_proto!("looprpc");
    }
    pub mod frdrpc {
        tonic::include_proto!("frdrpc");
    }
}
