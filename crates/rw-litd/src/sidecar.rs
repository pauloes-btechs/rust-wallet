//! Launching a bundled `litd` as a loopback-only sidecar.
//!
//! The desktop app ships litd as a Tauri sidecar binary whose SHA-256 is
//! pinned in `sidecars.lock.json` (copied from the signed upstream release
//! manifest; `cargo xtask fetch-sidecars` verifies it). This module builds
//! the argument list and waits for the node's TLS certificate. Every
//! listener is bound to 127.0.0.1; nothing is exposed to the network.

use rw_core::Network;
use std::path::{Path, PathBuf};
use std::time::Duration;

/// Ports used by the sidecar (offset from LND defaults so a user's own node
/// on 10009 is not disturbed).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SidecarPorts {
    /// litd's own RPC/UI port (lit RPC + proxied lnd/loop/pool/tapd/faraday).
    pub lit_rpc: u16,
    /// Embedded LND's raw gRPC port.
    pub lnd_rpc: u16,
    /// LND P2P port.
    pub lnd_p2p: u16,
}

impl Default for SidecarPorts {
    fn default() -> Self {
        Self { lit_rpc: 18443 + 3000, lnd_rpc: 10019, lnd_p2p: 9745 }
    }
}

#[derive(Clone, Debug)]
pub struct SidecarConfig {
    pub litd_bin: PathBuf,
    /// App data dir; litd and LND state live under `lit/` and `lnd/`.
    pub data_dir: PathBuf,
    pub network: Network,
    pub ports: SidecarPorts,
    /// Neutrino peers (`host:port`). Regtest/signet only in this build.
    pub neutrino_peers: Vec<String>,
    /// litd UI password: random per install, stored by the app (never logged).
    pub ui_password: String,
}

impl SidecarConfig {
    pub fn lit_dir(&self) -> PathBuf {
        self.data_dir.join("lit")
    }

    pub fn lnd_dir(&self) -> PathBuf {
        self.data_dir.join("lnd")
    }

    /// Command-line arguments. Keys mirror Sovereign Wallet's verified
    /// litd v0.17.8 harness config: integrated LND mode, litd's own
    /// `network` flag (not `lnd.bitcoin.*`), autopilot disabled off mainnet.
    pub fn args(&self) -> Result<Vec<String>, String> {
        if self.network.is_mainnet() {
            return Err("mainnet sidecar is disabled in this build".into());
        }
        let p = self.ports;
        let mut a = vec![
            format!("--lit-dir={}", self.lit_dir().display()),
            format!("--lnd.lnddir={}", self.lnd_dir().display()),
            "--lnd-mode=integrated".into(),
            format!("--network={}", self.network),
            "--autopilot.disable=true".into(),
            format!("--uipassword={}", self.ui_password),
            format!("--httpslisten=127.0.0.1:{}", p.lit_rpc),
            format!("--lnd.rpclisten=127.0.0.1:{}", p.lnd_rpc),
            format!("--lnd.listen=127.0.0.1:{}", p.lnd_p2p),
            "--lnd.restlisten=".into(),
            "--lnd.bitcoin.node=neutrino".into(),
        ];
        for peer in &self.neutrino_peers {
            a.push(format!("--lnd.neutrino.addpeer={peer}"));
        }
        Ok(a)
    }

    pub fn tls_cert(&self) -> PathBuf {
        self.lnd_dir().join("tls.cert")
    }

    pub fn admin_macaroon(&self) -> PathBuf {
        self.lnd_dir().join("data/chain/bitcoin").join(self.network.to_string()).join("admin.macaroon")
    }
}

/// Wait until `path` exists and is non-empty (LND writes tls.cert at startup).
pub async fn wait_for_file(path: &Path, timeout: Duration) -> bool {
    let deadline = tokio::time::Instant::now() + timeout;
    while tokio::time::Instant::now() < deadline {
        if tokio::fs::metadata(path).await.map(|m| m.len() > 0).unwrap_or(false) {
            return true;
        }
        tokio::time::sleep(Duration::from_millis(250)).await;
    }
    false
}

/// Spawn litd. The child is killed when the returned handle is dropped.
pub fn spawn(cfg: &SidecarConfig) -> std::io::Result<tokio::process::Child> {
    let args = cfg.args().map_err(std::io::Error::other)?;
    std::fs::create_dir_all(cfg.lit_dir())?;
    std::fs::create_dir_all(cfg.lnd_dir())?;
    tokio::process::Command::new(&cfg.litd_bin)
        .args(args)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::piped())
        .kill_on_drop(true)
        .spawn()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cfg(network: Network) -> SidecarConfig {
        SidecarConfig {
            litd_bin: "litd".into(),
            data_dir: "/tmp/rw".into(),
            network,
            ports: SidecarPorts::default(),
            neutrino_peers: vec!["127.0.0.1:18555".into()],
            ui_password: "pw".into(),
        }
    }

    #[test]
    fn every_listener_is_loopback() {
        let args = cfg(Network::Regtest).args().unwrap();
        for a in args.iter().filter(|a| a.contains("listen=") && !a.ends_with('=')) {
            assert!(a.contains("=127.0.0.1:"), "{a}");
        }
        assert!(args.contains(&"--network=regtest".to_string()));
        assert!(args.contains(&"--lnd-mode=integrated".to_string()));
    }

    #[test]
    fn mainnet_sidecar_refused() {
        assert!(cfg(Network::Mainnet).args().is_err());
    }

    #[test]
    fn macaroon_path_per_network() {
        assert!(cfg(Network::Signet).admin_macaroon().ends_with("data/chain/bitcoin/signet/admin.macaroon"));
    }
}
