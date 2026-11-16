//! Serde helpers: integers as decimal strings on the wire.
//!
//! LND reports sats and msats as uint64/int64. JSON numbers above 2^53 lose
//! precision in JavaScript, so the view contract carries them as strings.
//! Deserialization also accepts a JSON number, because some recorded routes
//! (for example invoice values) were captured as numbers.

use serde::{de, Deserializer, Serializer};
use std::fmt;

/// `#[serde(with = "rw_core::decstr")]` for `u64`.
pub fn serialize<S: Serializer>(v: &u64, s: S) -> Result<S::Ok, S::Error> {
    s.collect_str(v)
}

/// `#[serde(with = "rw_core::decstr")]` for `u64`.
pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<u64, D::Error> {
    d.deserialize_any(U64Visitor)
}

struct U64Visitor;

impl de::Visitor<'_> for U64Visitor {
    type Value = u64;
    fn expecting(&self, f: &mut fmt::Formatter) -> fmt::Result {
        f.write_str("an unsigned integer or a decimal string")
    }
    fn visit_u64<E: de::Error>(self, v: u64) -> Result<u64, E> {
        Ok(v)
    }
    fn visit_i64<E: de::Error>(self, v: i64) -> Result<u64, E> {
        u64::try_from(v).map_err(|_| E::custom("negative amount"))
    }
    fn visit_str<E: de::Error>(self, v: &str) -> Result<u64, E> {
        v.parse().map_err(|_| E::custom(format!("not a decimal u64: {v:?}")))
    }
}

/// Signed variant, for on-chain deltas.
pub mod signed {
    use super::*;

    pub fn serialize<S: Serializer>(v: &i64, s: S) -> Result<S::Ok, S::Error> {
        s.collect_str(v)
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<i64, D::Error> {
        d.deserialize_any(I64Visitor)
    }

    struct I64Visitor;

    impl de::Visitor<'_> for I64Visitor {
        type Value = i64;
        fn expecting(&self, f: &mut fmt::Formatter) -> fmt::Result {
            f.write_str("a signed integer or a decimal string")
        }
        fn visit_u64<E: de::Error>(self, v: u64) -> Result<i64, E> {
            i64::try_from(v).map_err(|_| E::custom("amount overflows i64"))
        }
        fn visit_i64<E: de::Error>(self, v: i64) -> Result<i64, E> {
            Ok(v)
        }
        fn visit_str<E: de::Error>(self, v: &str) -> Result<i64, E> {
            v.parse().map_err(|_| E::custom(format!("not a decimal i64: {v:?}")))
        }
    }
}

/// Optional variant: absent or null stays `None`.
pub mod opt {
    use super::*;
    use serde::Deserialize;

    pub fn serialize<S: Serializer>(v: &Option<u64>, s: S) -> Result<S::Ok, S::Error> {
        match v {
            Some(v) => s.collect_str(v),
            None => s.serialize_none(),
        }
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<Option<u64>, D::Error> {
        #[derive(Deserialize)]
        struct W(#[serde(with = "super")] u64);
        Option::<W>::deserialize(d).map(|o| o.map(|W(v)| v))
    }
}

#[cfg(test)]
mod tests {
    use serde::{Deserialize, Serialize};

    #[derive(Serialize, Deserialize, PartialEq, Debug)]
    struct T {
        #[serde(with = "crate::decstr")]
        a: u64,
        #[serde(with = "crate::decstr::signed")]
        b: i64,
        #[serde(with = "crate::decstr::opt", default)]
        c: Option<u64>,
    }

    #[test]
    fn round_trips_above_2_pow_53() {
        let t = T { a: u64::MAX, b: i64::MIN, c: Some(9_007_199_254_740_993) };
        let j = serde_json::to_string(&t).unwrap();
        assert_eq!(j, r#"{"a":"18446744073709551615","b":"-9223372036854775808","c":"9007199254740993"}"#);
        assert_eq!(serde_json::from_str::<T>(&j).unwrap(), t);
    }

    #[test]
    fn accepts_numbers_and_missing_optional() {
        let t: T = serde_json::from_str(r#"{"a":5,"b":-3}"#).unwrap();
        assert_eq!(t, T { a: 5, b: -3, c: None });
    }

    #[test]
    fn rejects_garbage() {
        assert!(serde_json::from_str::<T>(r#"{"a":"1e3","b":0}"#).is_err());
        assert!(serde_json::from_str::<T>(r#"{"a":-1,"b":0}"#).is_err());
    }
}
