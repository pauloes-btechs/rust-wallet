//! The last connection, so the app reconnects on launch. Paths only: no
//! macaroon contents, no passwords.

use rw_core::Network;
use serde::{Deserialize, Serialize};
use std::path::Path;

const FILE: &str = "connection.json";

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case")]
pub enum Profile {
    #[serde(rename_all = "camelCase")]
    Remote { endpoint: String, tls_cert_path: String, macaroon_path: String },
    #[serde(rename_all = "camelCase")]
    Sidecar { network: Network, neutrino_peers: Vec<String> },
}

impl Profile {
    pub fn kind(&self) -> &'static str {
        match self {
            Profile::Remote { .. } => "remote",
            Profile::Sidecar { .. } => "sidecar",
        }
    }

    pub fn network(&self) -> Option<Network> {
        match self {
            Profile::Sidecar { network, .. } => Some(*network),
            Profile::Remote { .. } => None,
        }
    }

    pub fn save(&self, dir: &Path) -> std::io::Result<()> {
        std::fs::write(dir.join(FILE), serde_json::to_vec_pretty(self)?)
    }

    pub fn load(dir: &Path) -> Option<Self> {
        serde_json::from_slice(&std::fs::read(dir.join(FILE)).ok()?).ok()
    }

    pub fn clear(dir: &Path) {
        let _ = std::fs::remove_file(dir.join(FILE));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip_and_clear() {
        let d = tempfile::tempdir().unwrap();
        assert_eq!(Profile::load(d.path()), None);
        let p = Profile::Sidecar { network: Network::Signet, neutrino_peers: vec!["a:1".into()] };
        p.save(d.path()).unwrap();
        assert_eq!(Profile::load(d.path()), Some(p));
        let raw = std::fs::read_to_string(d.path().join(FILE)).unwrap();
        assert!(raw.contains(r#""kind": "sidecar""#), "{raw}");
        Profile::clear(d.path());
        assert_eq!(Profile::load(d.path()), None);
    }
}
