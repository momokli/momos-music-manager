//! Shared `HX-Trigger` toast helper.
//!
//! htmx shows a toast for `HX-Trigger: {"toast":{"msg":…,"kind":…}}`. The value
//! has to survive as a **plain-ASCII** header: a raw multi-byte punctuation char
//! (e.g. `—` / `…`) made the header vanish before it reached the browser, so the
//! toast silently never fired (reproduced in `tests/download_ux.rs`). The message
//! is therefore `\u`-escaped — JSON decodes it back to the exact same text.

use axum::http::{HeaderMap, HeaderName, HeaderValue};
use axum::response::Response;

/// `HX-Trigger: {"toast":{…}}` as a header map (empty when it can't be built).
pub fn header(msg: &str, kind: &str) -> HeaderMap {
    let mut h = HeaderMap::new();
    if let Some(v) = value(msg, kind) {
        h.insert(HeaderName::from_static("hx-trigger"), v);
    }
    h
}

/// Attach the toast header to `resp` (no-op when it can't be built).
pub fn apply(resp: Response, msg: &str, kind: &str) -> Response {
    let mut resp = resp;
    if let Some(v) = value(msg, kind) {
        resp.headers_mut().insert("HX-Trigger", v);
    }
    resp
}

/// The `HX-Trigger` value for a toast.
pub fn value(msg: &str, kind: &str) -> Option<HeaderValue> {
    let body = format!(
        "{{\"toast\":{{\"msg\":{},\"kind\":{}}}}}",
        json_str_ascii(msg),
        json_str_ascii(kind)
    );
    HeaderValue::from_str(&body).ok()
}

/// JSON string literal for `s` with every non-ASCII char `\u`-escaped, so the
/// result is guaranteed to be plain ASCII and survives the header round-trip.
pub fn json_str_ascii(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
            c if c.is_ascii() => out.push(c),
            c => {
                let mut buf = [0u16; 2];
                for u in c.encode_utf16(&mut buf).iter() {
                    out.push_str(&format!("\\u{:04x}", *u));
                }
            }
        }
    }
    out.push('"');
    out
}

/// The `HX-Trigger` value for a toast **plus** `extra` event names set to `true`,
/// so one response can show a toast and wake a shell widget at the same time
/// (e.g. `hub:queue-changed` / `hub:now-changed`).
pub fn value_with(msg: &str, kind: &str, extra: &[&str]) -> Option<HeaderValue> {
    let mut body = String::from("{\"toast\":{\"msg\":");
    body.push_str(&json_str_ascii(msg));
    body.push_str(",\"kind\":");
    body.push_str(&json_str_ascii(kind));
    body.push('}');
    for e in extra {
        body.push_str(&format!(",\"{e}\":true"));
    }
    body.push('}');
    HeaderValue::from_str(&body).ok()
}

/// Attach a toast plus `extra` events to `resp` (no-op when it can't be built).
pub fn apply_with(resp: Response, msg: &str, kind: &str, extra: &[&str]) -> Response {
    let mut resp = resp;
    if let Some(v) = value_with(msg, kind, extra) {
        resp.headers_mut().insert("HX-Trigger", v);
    }
    resp
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn escapes_to_plain_ascii_and_round_trips() {
        let literal = json_str_ascii("FLAC — läuft …");
        assert!(literal.is_ascii(), "must be plain ASCII: {literal}");
        assert!(literal.starts_with('"') && literal.ends_with('"'));
        let back: String = serde_json::from_str(&literal).unwrap();
        assert_eq!(back, "FLAC — läuft …");
    }

    #[test]
    fn header_survives_non_ascii_messages() {
        let h = header("Grüße — …", "ok");
        let v = h
            .get("hx-trigger")
            .expect("header present")
            .to_str()
            .expect("ascii value");
        assert!(v.is_ascii());
        assert!(v.contains("\"kind\":\"ok\""), "{v}");
    }

    #[test]
    fn with_extra_events_stays_valid_json() {
        let v = value_with("Grüße — …", "ok", &["hub:queue-changed", "hub:now-changed"])
            .expect("value")
            .to_str()
            .expect("ascii value")
            .to_string();
        assert!(v.is_ascii(), "{v}");
        let parsed: serde_json::Value = serde_json::from_str(&v).expect("valid json");
        assert_eq!(parsed["toast"]["msg"], "Grüße — …");
        assert_eq!(parsed["toast"]["kind"], "ok");
        assert_eq!(parsed["hub:queue-changed"], serde_json::Value::Bool(true));
        assert_eq!(parsed["hub:now-changed"], serde_json::Value::Bool(true));
        assert!(
            value_with("x", "ok", &[]).is_some(),
            "no extras still works"
        );
    }
}
