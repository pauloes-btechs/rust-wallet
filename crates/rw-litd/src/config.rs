use rw_core::{MainnetPolicy, Network};
use std::path::{Path, PathBuf};
use thiserror::Error;

#[derive(Debug, Error)]
pub enum ConfigError {
    #[error("cannot read {path}: {source}")]
    Read { path: PathBuf, source: std::io::Error },
    #[error("{0} contains no PEM certificate")]
    NoCert(PathBuf),
    #[error("invalid endpoint {0:?} (expected host:port)")]
    Endpoint(String),
}

/// How to reach one LND / litd RPC endpoint.
#[derive(Clone, Debug)]
pub struct ConnectConfig {
    /// `host:port`, e.g. `127.0.0.1:10009`.
    pub endpoint: String,
    pub tls_cert: PathBuf,
    pub macaroon: PathBuf,
    pub mainnet: MainnetPolicy,
}

impl ConnectConfig {
    /// Paths inside an LND data dir: `tls.cert` and
    /// `data/chain/bitcoin/<network>/admin.macaroon`.
    pub fn from_lnd_dir(endpoint: impl Into<String>, lnd_dir: &Path, network: Network) -> Self {
        let net = match network {
            Network::Mainnet => "mainnet".to_string(),
            Network::Testnet => "testnet".to_string(),
            n => n.to_string(),
        };
        Self {
            endpoint: endpoint.into(),
            tls_cert: lnd_dir.join("tls.cert"),
            macaroon: lnd_dir.join("data/chain/bitcoin").join(net).join("admin.macaroon"),
            mainnet: MainnetPolicy::Refuse,
        }
    }

    pub(crate) fn read_cert_der(&self) -> Result<Vec<u8>, ConfigError> {
        let pem = std::fs::read(&self.tls_cert)
            .map_err(|source| ConfigError::Read { path: self.tls_cert.clone(), source })?;
        let mut rd = pem.as_slice();
        let first = rustls_pemfile::certs(&mut rd).next().and_then(Result::ok).map(|c| c.as_ref().to_vec());
        first.ok_or_else(|| ConfigError::NoCert(self.tls_cert.clone()))
    }

    pub(crate) fn read_macaroon_hex(&self) -> Result<String, ConfigError> {
        std::fs::read(&self.macaroon)
            .map(hex::encode)
            .map_err(|source| ConfigError::Read { path: self.macaroon.clone(), source })
    }
}
