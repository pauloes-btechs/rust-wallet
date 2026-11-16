//! Capability declarations. A backend states what it supports; the UI and
//! CLI render or accept only what is declared. Undeclared actions fail with
//! [`crate::ApiError::Unsupported`] instead of half-working.

use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Capability {
    /// Read node state, balances and history.
    Read,
    OnchainReceive,
    OnchainSend,
    LightningReceive,
    LightningSend,
    Channels,
    ChannelBackups,
    /// litd sub-server status.
    Services,
    TaprootAssets,
    Loop,
    Faraday,
}

/// The ordered set a backend declares.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct CapabilitySet(BTreeSet<Capability>);

impl CapabilitySet {
    pub fn of(caps: &[Capability]) -> Self {
        Self(caps.iter().copied().collect())
    }

    pub fn has(&self, c: Capability) -> bool {
        self.0.contains(&c)
    }

    /// `Ok(())` if declared, otherwise the error every caller should return.
    pub fn require(&self, c: Capability) -> crate::ApiResult<()> {
        if self.has(c) {
            Ok(())
        } else {
            Err(crate::ApiError::Unsupported(format!("{c:?}")))
        }
    }

    pub fn iter(&self) -> impl Iterator<Item = Capability> + '_ {
        self.0.iter().copied()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn require_reports_undeclared() {
        let s = CapabilitySet::of(&[Capability::Read]);
        assert!(s.require(Capability::Read).is_ok());
        assert_eq!(s.require(Capability::LightningSend), Err(crate::ApiError::Unsupported("LightningSend".into())));
    }

    #[test]
    fn serializes_kebab_case_sorted() {
        let s = CapabilitySet::of(&[Capability::OnchainSend, Capability::Read]);
        assert_eq!(serde_json::to_string(&s).unwrap(), r#"["read","onchain-send"]"#);
    }
}
