//! Bitcoin networks and the mainnet guard.

use serde::{Deserialize, Serialize};
use std::fmt;
use std::str::FromStr;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Network {
    Mainnet,
    Testnet,
    Testnet4,
    Signet,
    Regtest,
}

impl Network {
    /// BOLT-11 human-readable-part prefix after `ln`.
    pub fn bolt11_prefix(self) -> &'static str {
        match self {
            Network::Mainnet => "lnbc",
            Network::Testnet | Network::Testnet4 => "lntb",
            Network::Signet => "lntbs",
            Network::Regtest => "lnbcrt",
        }
    }

    /// Segwit address HRP.
    pub fn address_hrp(self) -> &'static str {
        match self {
            Network::Mainnet => "bc",
            Network::Testnet | Network::Testnet4 | Network::Signet => "tb",
            Network::Regtest => "bcrt",
        }
    }

    pub fn is_mainnet(self) -> bool {
        matches!(self, Network::Mainnet)
    }
}

impl fmt::Display for Network {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Network::Mainnet => "mainnet",
            Network::Testnet => "testnet",
            Network::Testnet4 => "testnet4",
            Network::Signet => "signet",
            Network::Regtest => "regtest",
        })
    }
}

impl FromStr for Network {
    type Err = String;
    /// Accepts LND's `chains[].network` strings ("mainnet", "testnet",
    /// "testnet4", "signet", "regtest") and the bitcoind alias "main"/"bitcoin".
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_ascii_lowercase().as_str() {
            "mainnet" | "main" | "bitcoin" => Ok(Network::Mainnet),
            "testnet" | "testnet3" => Ok(Network::Testnet),
            "testnet4" => Ok(Network::Testnet4),
            "signet" => Ok(Network::Signet),
            "regtest" => Ok(Network::Regtest),
            other => Err(format!("unknown network {other:?}")),
        }
    }
}

/// Whether this build/session may talk to a mainnet node at all.
///
/// The default is [`MainnetPolicy::Refuse`]: mainnet stays off until there is
/// an explicit product sign-off. The guard runs on the *node-reported*
/// network, so a misconfigured endpoint cannot sneak past it.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum MainnetPolicy {
    #[default]
    Refuse,
    /// Operator explicitly opted in (CLI `--allow-mainnet`).
    Allow,
}

impl MainnetPolicy {
    pub fn check(self, node_network: Network) -> Result<(), crate::ApiError> {
        if node_network.is_mainnet() && self == MainnetPolicy::Refuse {
            return Err(crate::ApiError::MainnetRefused);
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn guard_refuses_mainnet_by_default() {
        assert!(MainnetPolicy::default().check(Network::Mainnet).is_err());
        assert!(MainnetPolicy::default().check(Network::Regtest).is_ok());
        assert!(MainnetPolicy::Allow.check(Network::Mainnet).is_ok());
    }

    #[test]
    fn parses_lnd_strings() {
        assert_eq!("regtest".parse::<Network>().unwrap(), Network::Regtest);
        assert_eq!("main".parse::<Network>().unwrap(), Network::Mainnet);
        assert!("simnet2".parse::<Network>().is_err());
    }
}
