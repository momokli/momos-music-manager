//! Server-side flash cookie (`hub_flash`) for full-page (non-htmx) POSTs.
//!
//! htmx actions show their toast via `HX-Trigger` (`toast.rs`). A full-page form
//! post instead redirects (PRG) and would lose the message, so it is carried in
//! a short-lived `hub_flash` cookie: [`redirect`] sets it on the `303`, and
//! [`middleware`] consumes it once (injecting `x-hub-flash-msg` / `-kind` request
//! headers) and clears it on the way out, so a reload shows no toast.
//!
//! The cookie value is plain ASCII (`"<kind>:<urlencoded msg>"`); the message is
//! URL-encoded, so umlauts and punctuation survive the header round-trip.

use axum::extract::Request;
use axum::http::{HeaderMap, HeaderName, HeaderValue, header};
use axum::middleware::Next;
use axum::response::{IntoResponse, Redirect, Response};

/// Cookie name carrying the flash between requests.
pub const COOKIE: &str = "hub_flash";
/// Request header the middleware injects with the (url-encoded) flash message.
pub const MSG_HEADER: &str = "x-hub-flash-msg";
/// Request header the middleware injects with the flash kind.
pub const KIND_HEADER: &str = "x-hub-flash-kind";

/// Encode `kind` + `msg` into the cookie value (`kind:urlencoded(msg)`).
pub fn encode(kind: &str, msg: &str) -> String {
    format!("{kind}:{}", urlencoding::encode(msg))
}

/// Decode a cookie value into `(kind, msg)`. A value without a `:` is treated as
/// an `ok` message (defensive: never panics on a malformed cookie).
pub fn decode(value: &str) -> (String, String) {
    let (kind, encoded) = match value.split_once(':') {
        Some((k, v)) => (k, v),
        None => ("ok", value),
    };
    let msg = urlencoding::decode(encoded)
        .map(|c| c.into_owned())
        .unwrap_or_else(|_| encoded.to_string());
    (kind.to_string(), msg)
}

/// `Set-Cookie` value that stores the flash for exactly one navigation.
pub fn set_cookie(kind: &str, msg: &str) -> String {
    format!(
        "{COOKIE}={}; Path=/; HttpOnly; SameSite=Lax; Max-Age=30",
        encode(kind, msg)
    )
}

/// `Set-Cookie` value that clears the flash cookie.
pub fn clear_cookie() -> String {
    format!("{COOKIE}=; Path=/; HttpOnly; Max-Age=0")
}

/// A `303 See Other` redirect that carries `msg` in the flash cookie.
pub fn redirect(to: &str, msg: &str, kind: &str) -> Response {
    let mut resp = Redirect::to(to).into_response();
    if let Ok(v) = HeaderValue::from_str(&set_cookie(kind, msg)) {
        resp.headers_mut().append(header::SET_COOKIE, v);
    }
    resp
}

/// The flash message injected into the request headers by [`middleware`] (empty
/// when the request carried no flash cookie).
pub fn from_headers(headers: &HeaderMap) -> String {
    headers
        .get(MSG_HEADER)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| urlencoding::decode(v).ok())
        .map(|c| c.into_owned())
        .unwrap_or_default()
}

/// Read the raw `hub_flash` cookie value from a request header map.
fn read_cookie(headers: &HeaderMap) -> Option<String> {
    let raw = headers.get(header::COOKIE)?.to_str().ok()?;
    for part in raw.split(';') {
        let part = part.trim();
        if let Some(v) = part.strip_prefix(&format!("{COOKIE}=")) {
            if !v.is_empty() {
                return Some(v.to_string());
            }
        }
    }
    None
}

/// Axum middleware: turn a `hub_flash` cookie into `x-hub-flash-*` request
/// headers, and clear the cookie on the way out — but only when the request
/// actually carried one (so the POST that *sets* the flash is left untouched).
pub async fn middleware(mut req: Request, next: Next) -> Response {
    let had = read_cookie(req.headers());
    if let Some(value) = had.clone() {
        let (kind, msg) = decode(&value);
        let headers = req.headers_mut();
        if let Ok(v) = HeaderValue::from_str(&urlencoding::encode(&msg)) {
            headers.insert(HeaderName::from_static(MSG_HEADER), v);
        }
        if let Ok(v) = HeaderValue::from_str(&kind) {
            headers.insert(HeaderName::from_static(KIND_HEADER), v);
        }
    }
    let mut resp = next.run(req).await;
    if had.is_some() {
        if let Ok(v) = HeaderValue::from_str(&clear_cookie()) {
            resp.headers_mut().append(header::SET_COOKIE, v);
        }
    }
    resp
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn encode_decode_round_trips_umlauts() {
        let v = encode("ok", "Grüße — läuft …");
        assert!(v.is_ascii(), "cookie value must be plain ASCII: {v}");
        let (kind, msg) = decode(&v);
        assert_eq!(kind, "ok");
        assert_eq!(msg, "Grüße — läuft …");
    }

    #[test]
    fn decode_tolerates_value_without_kind() {
        let (kind, msg) = decode("Hallo");
        assert_eq!(kind, "ok");
        assert_eq!(msg, "Hallo");
    }

    #[test]
    fn set_cookie_carries_flags() {
        let c = set_cookie("ok", "Gespeichert");
        assert!(c.starts_with("hub_flash=ok:"), "{c}");
        assert!(c.contains("Path=/"));
        assert!(c.contains("HttpOnly"));
        assert!(c.contains("SameSite=Lax"));
        assert!(c.contains("Max-Age=30"));
    }

    #[test]
    fn clear_cookie_expires_immediately() {
        let c = clear_cookie();
        assert!(c.starts_with("hub_flash=;"), "{c}");
        assert!(c.contains("Max-Age=0"), "{c}");
    }

    #[test]
    fn from_headers_reads_injected_message() {
        let empty = HeaderMap::new();
        assert_eq!(from_headers(&empty), "");
        let mut h = HeaderMap::new();
        h.insert(
            HeaderName::from_static(MSG_HEADER),
            HeaderValue::from_str(&urlencoding::encode("Grüße")).unwrap(),
        );
        assert_eq!(from_headers(&h), "Grüße");
    }
}
