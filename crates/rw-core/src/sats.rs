//! A satoshi amount with checked arithmetic and human formatting.

use serde::{Deserialize, Serialize};
use std::fmt;

/// Satoshis. Arithmetic is checked: an overflow is a bug, never a wrap.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct Sats(#[serde(with = "crate::decstr")] pub u64);

/// 100,000,000 sats per bitcoin.
pub const SATS_PER_BTC: u64 = 100_000_000;

impl Sats {
    pub const ZERO: Sats = Sats(0);

    pub fn checked_add(self, o: Sats) -> Option<Sats> {
        self.0.checked_add(o.0).map(Sats)
    }

    pub fn checked_sub(self, o: Sats) -> Option<Sats> {
        self.0.checked_sub(o.0).map(Sats)
    }

    /// `1_234_567` → `"1,234,567"`.
    pub fn grouped(self) -> String {
        group(self.0)
    }

    /// `150_000_000` → `"1.50000000"`. Exact: integer math only, no floats.
    pub fn btc_string(self) -> String {
        format!("{}.{:08}", self.0 / SATS_PER_BTC, self.0 % SATS_PER_BTC)
    }

    /// Millisatoshis rounded *down* to whole sats (LND convention for display).
    pub fn from_msat_floor(msat: u64) -> Sats {
        Sats(msat / 1000)
    }
}

fn group(n: u64) -> String {
    let s = n.to_string();
    let mut out = String::with_capacity(s.len() + s.len() / 3);
    for (i, c) in s.chars().enumerate() {
        if i > 0 && (s.len() - i) % 3 == 0 {
            out.push(',');
        }
        out.push(c);
    }
    out
}

impl fmt::Display for Sats {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{} sats", self.grouped())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn grouping() {
        assert_eq!(Sats(0).grouped(), "0");
        assert_eq!(Sats(999).grouped(), "999");
        assert_eq!(Sats(1000).grouped(), "1,000");
        assert_eq!(Sats(159_952_157).grouped(), "159,952,157");
        assert_eq!(Sats(u64::MAX).grouped(), "18,446,744,073,709,551,615");
    }

    #[test]
    fn btc_is_exact() {
        assert_eq!(Sats(1).btc_string(), "0.00000001");
        assert_eq!(Sats(150_000_000).btc_string(), "1.50000000");
        assert_eq!(Sats(2_100_000_000_000_000).btc_string(), "21000000.00000000");
    }

    #[test]
    fn checked() {
        assert_eq!(Sats(u64::MAX).checked_add(Sats(1)), None);
        assert_eq!(Sats(0).checked_sub(Sats(1)), None);
        assert_eq!(Sats::from_msat_floor(1999), Sats(1));
    }
}
