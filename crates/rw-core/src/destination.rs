//! Offline validation of what a user pastes into "Send".
//!
//! This is a gate, not a decoder of record: the node's own `DecodePayReq`
//! remains authoritative for invoice fields. What this module guarantees is
//! that a malformed string, a bad checksum, or a destination for the wrong
//! network is rejected *before* a payment intent is ever authorized.

use crate::{ApiError, Network};
use bech32::primitives::decode::CheckedHrpstring;
use bech32::Checksum;

/// BOLT-11 uses the bech32 checksum without bech32's 90-character limit.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Bolt11Checksum {}

impl Checksum for Bolt11Checksum {
    type MidstateRepr = u32;
    // Same generator and residue as BIP-173 bech32; only the length limit differs.
    const CODE_LENGTH: usize = 7089;
    const CHECKSUM_LENGTH: usize = 6;
    const GENERATOR_SH: [u32; 5] = [0x3b6a57b2, 0x26508e6d, 0x1ea119fa, 0x3d4233dd, 0x2a1462b3];
    const TARGET_RESIDUE: u32 = 1;
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Destination {
    /// A BOLT-11 invoice. `amount_msat` is parsed from the HRP (None = zero-amount invoice).
    Bolt11 { invoice: String, network: Network, amount_msat: Option<u64> },
    /// A segwit (v0) or taproot (v1+) address, checksum-verified.
    SegwitAddress { address: String, network: Network, witness_version: u8 },
    /// A base58 address (P2PKH/P2SH). Shape-checked only; the node validates the checksum.
    LegacyAddress { address: String, network_hint: Option<Network> },
}

impl Destination {
    pub fn network(&self) -> Option<Network> {
        match self {
            Destination::Bolt11 { network, .. } | Destination::SegwitAddress { network, .. } => Some(*network),
            Destination::LegacyAddress { network_hint, .. } => *network_hint,
        }
    }

    pub fn is_lightning(&self) -> bool {
        matches!(self, Destination::Bolt11 { .. })
    }

    /// The canonical string to hand to the node.
    pub fn as_str(&self) -> &str {
        match self {
            Destination::Bolt11 { invoice, .. } => invoice,
            Destination::SegwitAddress { address, .. } | Destination::LegacyAddress { address, .. } => address,
        }
    }
}

/// Parse and validate. With `expected = Some(net)`, a destination for any
/// other network is rejected (the classic "testnet invoice in a regtest
/// wallet" mistake). Signet and testnet share address HRPs, so an address
/// for either is accepted when the other is expected.
pub fn parse_destination(input: &str, expected: Option<Network>) -> Result<Destination, ApiError> {
    let s = strip_uri(input.trim());
    if s.is_empty() {
        return Err(ApiError::Invalid("empty destination".into()));
    }
    let lower = s.to_ascii_lowercase();
    let dest = if lower.starts_with("ln") {
        parse_bolt11(&lower)?
    } else if let Some(d) = parse_segwit(&lower)? {
        d
    } else {
        parse_legacy(s)?
    };
    if let (Some(want), Some(got)) = (expected, dest.network()) {
        let compatible = want == got
            || (want.address_hrp() == got.address_hrp() && !dest.is_lightning())
            || (matches!(want, Network::Testnet | Network::Testnet4)
                && matches!(got, Network::Testnet | Network::Testnet4));
        if !compatible {
            return Err(ApiError::Invalid(format!("this destination is for {got}, but the wallet is on {want}")));
        }
    }
    Ok(dest)
}

fn strip_uri(s: &str) -> &str {
    for p in ["lightning:", "LIGHTNING:", "bitcoin:", "BITCOIN:"] {
        if let Some(rest) = s.strip_prefix(p) {
            // BIP-21: drop the query string; a `lightning=` param is not followed here.
            return rest.split('?').next().unwrap_or(rest);
        }
    }
    s
}

fn parse_bolt11(s: &str) -> Result<Destination, ApiError> {
    let checked = CheckedHrpstring::new::<Bolt11Checksum>(s)
        .map_err(|e| ApiError::Invalid(format!("not a valid lightning invoice: {e}")))?;
    let hrp = checked.hrp().to_lowercase();
    // Longest prefix first: lnbcrt before lnbc, lntbs before lntb.
    let (network, rest) = [Network::Regtest, Network::Mainnet, Network::Signet, Network::Testnet]
        .into_iter()
        .find_map(|n| hrp.strip_prefix(n.bolt11_prefix()).map(|r| (n, r)))
        .ok_or_else(|| ApiError::Invalid(format!("unknown invoice network prefix {hrp:?}")))?;
    let amount_msat = parse_hrp_amount(rest)?;
    // The data part must at least hold a timestamp (7 groups) and a signature (104 groups).
    if checked.fe32_iter::<std::vec::IntoIter<u8>>().count() < 7 + 104 {
        return Err(ApiError::Invalid("lightning invoice is truncated".into()));
    }
    Ok(Destination::Bolt11 { invoice: s.to_string(), network, amount_msat })
}

/// BOLT-11 amount: digits followed by an optional multiplier.
fn parse_hrp_amount(rest: &str) -> Result<Option<u64>, ApiError> {
    if rest.is_empty() {
        return Ok(None);
    }
    let bad = || ApiError::Invalid(format!("invalid invoice amount {rest:?}"));
    let (digits, mult) = match rest.chars().last() {
        Some(c @ ('m' | 'u' | 'n' | 'p')) => (&rest[..rest.len() - 1], Some(c)),
        _ => (rest, None),
    };
    if digits.is_empty() || !digits.bytes().all(|b| b.is_ascii_digit()) || digits.starts_with('0') {
        return Err(bad());
    }
    let n: u64 = digits.parse().map_err(|_| bad())?;
    // 1 BTC = 10^11 msat.
    let msat = match mult {
        None => n.checked_mul(100_000_000_000),
        Some('m') => n.checked_mul(100_000_000),
        Some('u') => n.checked_mul(100_000),
        Some('n') => n.checked_mul(100),
        Some('p') => {
            if n % 10 != 0 {
                return Err(ApiError::Invalid("pico-bitcoin amount must be a multiple of 10".into()));
            }
            Some(n / 10)
        }
        _ => unreachable!(),
    };
    msat.map(Some).ok_or_else(bad)
}

fn parse_segwit(s: &str) -> Result<Option<Destination>, ApiError> {
    let Some((hrp, _)) = s.split_once('1') else { return Ok(None) };
    let network = match hrp {
        "bc" => Network::Mainnet,
        "tb" => Network::Testnet,
        "bcrt" => Network::Regtest,
        _ => return Ok(None),
    };
    let (_, version, _program) =
        bech32::segwit::decode(s).map_err(|e| ApiError::Invalid(format!("not a valid {network} address: {e}")))?;
    Ok(Some(Destination::SegwitAddress { address: s.to_string(), network, witness_version: version.to_u8() }))
}

fn parse_legacy(s: &str) -> Result<Destination, ApiError> {
    const B58: &str = "123456789ABCDEFGHJKLMNPQRSTUVWXYZabcdefghijkmnopqrstuvwxyz";
    if !(26..=35).contains(&s.len()) || !s.chars().all(|c| B58.contains(c)) {
        return Err(ApiError::Invalid("not a lightning invoice or bitcoin address".into()));
    }
    let network_hint = match s.as_bytes()[0] {
        b'1' | b'3' => Some(Network::Mainnet),
        b'm' | b'n' | b'2' => Some(Network::Testnet),
        _ => return Err(ApiError::Invalid("unrecognised address prefix".into())),
    };
    Ok(Destination::LegacyAddress { address: s.to_string(), network_hint })
}

#[cfg(test)]
mod tests {
    use super::*;

    // BIP-173 / BIP-350 test vectors.
    const P2WPKH_MAIN: &str = "bc1qw508d6qejxtdg4y5r3zarvary0c5xw7kv8f3t4";
    const P2TR_MAIN: &str = "bc1p0xlxvlhemja6c4dqv22uapctqupfhlxm9h8z3k2e72q4k9hcz7vqzk5jj0";
    const P2WPKH_TB: &str = "tb1qw508d6qejxtdg4y5r3zarvary0c5xw7kxpjzsx";
    // BOLT-11 spec example ("Please send $3 for a cup of coffee", 2500u).
    const BOLT11_2500U: &str = "lnbc2500u1pvjluezsp5zyg3zyg3zyg3zyg3zyg3zyg3zyg3zyg3zyg3zyg3zyg3zyg3zygspp5qqqsyqcyq5rqwzqfqqqsyqcyq5rqwzqfqqqsyqcyq5rqwzqfqypqdq5xysxxatsyp3k7enxv4jsxqzpu9qrsgquk0rl77nj30yxdy8j9vdx85fkpmdla2087ne0xh8nhedh8w27kyke0lp53ut353s06fv3qfegext0eh0ymjpf39tuven09sam30g4vgpfna3rh";

    #[test]
    fn segwit_addresses() {
        let d = parse_destination(P2WPKH_MAIN, None).unwrap();
        assert!(matches!(d, Destination::SegwitAddress { network: Network::Mainnet, witness_version: 0, .. }));
        let d = parse_destination(P2TR_MAIN, None).unwrap();
        assert!(matches!(d, Destination::SegwitAddress { witness_version: 1, .. }));
        // Uppercase (QR codes) is accepted and canonicalised.
        let d = parse_destination(&P2WPKH_MAIN.to_uppercase(), None).unwrap();
        assert_eq!(d.as_str(), P2WPKH_MAIN);
    }

    #[test]
    fn bad_checksum_is_rejected() {
        let mut s = P2WPKH_MAIN.to_string();
        s.pop();
        s.push('5');
        assert!(parse_destination(&s, None).is_err());
        let mut inv = BOLT11_2500U.to_string();
        inv.replace_range(20..21, "x");
        assert!(parse_destination(&inv, None).is_err());
    }

    #[test]
    fn bolt11_spec_vector() {
        match parse_destination(BOLT11_2500U, None).unwrap() {
            Destination::Bolt11 { network, amount_msat, .. } => {
                assert_eq!(network, Network::Mainnet);
                assert_eq!(amount_msat, Some(250_000_000));
            }
            d => panic!("{d:?}"),
        }
        assert!(parse_destination(&format!("lightning:{BOLT11_2500U}"), None).is_ok());
    }

    #[test]
    fn network_mismatch_is_rejected() {
        let e = parse_destination(BOLT11_2500U, Some(Network::Regtest)).unwrap_err();
        assert!(e.to_string().contains("mainnet"), "{e}");
        assert!(parse_destination(P2WPKH_TB, Some(Network::Signet)).is_ok());
        assert!(parse_destination(P2WPKH_TB, Some(Network::Regtest)).is_err());
    }

    #[test]
    fn hrp_amounts() {
        assert_eq!(parse_hrp_amount("").unwrap(), None);
        assert_eq!(parse_hrp_amount("120u").unwrap(), Some(12_000_000));
        assert_eq!(parse_hrp_amount("20m").unwrap(), Some(2_000_000_000));
        assert_eq!(parse_hrp_amount("10p").unwrap(), Some(1));
        assert!(parse_hrp_amount("15p").is_err());
        assert!(parse_hrp_amount("012u").is_err());
        assert!(parse_hrp_amount("99999999999999999999").is_err());
    }

    #[test]
    fn garbage() {
        for s in ["", "   ", "hello", "lnbc1", "bitcoin:", "bc1qqqqq"] {
            assert!(parse_destination(s, None).is_err(), "{s:?}");
        }
    }

    #[test]
    fn bip21_uri() {
        let d = parse_destination(&format!("bitcoin:{P2WPKH_MAIN}?amount=0.1"), None).unwrap();
        assert_eq!(d.as_str(), P2WPKH_MAIN);
    }
}
