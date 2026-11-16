use rw_core::Sats;

pub fn sats(v: u64) -> String {
    Sats(v).grouped()
}

pub fn btc(v: u64) -> String {
    Sats(v).btc_string()
}

/// `0123456789abcdef…` → `01234567…cdef`.
pub fn short(s: &str) -> String {
    if s.chars().count() > 18 {
        let head: String = s.chars().take(8).collect();
        let tail: String = s.chars().rev().take(6).collect::<Vec<_>>().into_iter().rev().collect();
        format!("{head}…{tail}")
    } else {
        s.to_string()
    }
}

/// Unix millis → `2026-11-05 19:55` (UTC).
pub fn when_ms(ms: i64) -> String {
    if ms <= 0 {
        return "—".into();
    }
    #[cfg(target_family = "wasm")]
    {
        let d = js_sys::Date::new(&wasm_bindgen::JsValue::from_f64(ms as f64));
        let iso: String = d.to_iso_string().into();
        iso.get(..16).unwrap_or(&iso).replace('T', " ")
    }
    #[cfg(not(target_family = "wasm"))]
    {
        format!("{ms}")
    }
}

pub fn when_ns(ns: u64) -> String {
    when_ms((ns / 1_000_000) as i64)
}

/// Percentage of `part` in `whole`, 0–100.
pub fn pct(part: u64, whole: u64) -> f64 {
    if whole == 0 {
        0.0
    } else {
        (part as f64 / whole as f64 * 100.0).clamp(0.0, 100.0)
    }
}
