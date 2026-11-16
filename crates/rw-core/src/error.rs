use serde::{Deserialize, Serialize};
use thiserror::Error;

/// Errors crossing the backend → UI boundary. Serializable so the Tauri
/// shell and the WASM UI see the same variants.
#[derive(Debug, Error, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", content = "message", rename_all = "kebab-case")]
pub enum ApiError {
    #[error("the node is not reachable: {0}")]
    Unreachable(String),
    #[error("the node wallet is locked")]
    Locked,
    #[error("this backend does not support {0}")]
    Unsupported(String),
    #[error("invalid input: {0}")]
    Invalid(String),
    #[error("the node rejected the request: {0}")]
    Rejected(String),
    #[error("mainnet is disabled in this build (regtest/signet only until product sign-off)")]
    MainnetRefused,
    #[error("payment safety: {0}")]
    PaymentSafety(String),
    #[error("{0}")]
    Other(String),
}

pub type ApiResult<T> = Result<T, ApiError>;
