//! Request gate for one MCP endpoint, run before any JSON is read: the `Host` must be exactly this server
//! (blocks DNS rebinding), a browser `Origin` is refused, and the bearer token must match this endpoint's token
//! (compared in constant time). Each endpoint has its own gate, so the agent token never opens `/mcp/analyst`
//! and the analyst token never opens `/mcp`. A refusal carries no detail and no CORS headers.

use axum::{
    extract::{Request, State},
    http::{HeaderMap, StatusCode, header},
    middleware::Next,
    response::{IntoResponse, Response},
};

#[derive(Clone)]
pub struct Gate {
    pub token: String,
    pub port: u16,
}

/// Compares without an early exit on the first differing byte.
pub fn ct_eq(a: &[u8], b: &[u8]) -> bool {
    let mut diff = a.len() ^ b.len();
    for i in 0..a.len().max(b.len()) {
        let (x, y) = (
            a.get(i).copied().unwrap_or(0),
            b.get(i).copied().unwrap_or(0),
        );
        diff |= usize::from(x ^ y);
    }
    std::hint::black_box(diff) == 0
}

fn host_name(authority: &str) -> &str {
    let rest = authority.split_once("://").map_or(authority, |(_, r)| r);
    let authority = rest.split('/').next().unwrap_or("");
    if let Some(end) = authority.strip_prefix('[').and_then(|a| a.find(']')) {
        return &authority[..end + 2];
    }
    authority.split(':').next().unwrap_or("")
}

fn is_loopback(value: &str) -> bool {
    matches!(host_name(value), "127.0.0.1" | "localhost" | "[::1]")
}

fn host_ok(headers: &HeaderMap, port: u16) -> bool {
    let Some(host) = headers.get(header::HOST).and_then(|v| v.to_str().ok()) else {
        return false;
    };
    host == format!("127.0.0.1:{port}") || host == format!("localhost:{port}")
}

fn origin_ok(headers: &HeaderMap) -> bool {
    headers
        .get(header::ORIGIN)
        .is_none_or(|v| v.to_str().is_ok_and(is_loopback))
}

fn bearer(headers: &HeaderMap) -> Option<&str> {
    let value = headers.get(header::AUTHORIZATION)?.to_str().ok()?;
    let (scheme, token) = value.split_once(' ')?;
    scheme.eq_ignore_ascii_case("bearer").then(|| token.trim())
}

/// What the gate decides for a request's headers.
pub fn check(headers: &HeaderMap, gate: &Gate) -> Result<(), StatusCode> {
    if !host_ok(headers, gate.port) || !origin_ok(headers) {
        return Err(StatusCode::FORBIDDEN);
    }
    match bearer(headers) {
        Some(t) if !gate.token.is_empty() && ct_eq(t.as_bytes(), gate.token.as_bytes()) => Ok(()),
        _ => Err(StatusCode::UNAUTHORIZED),
    }
}

pub async fn gate(State(gate): State<Gate>, req: Request, next: Next) -> Response {
    match check(req.headers(), &gate) {
        Ok(()) => next.run(req).await,
        Err(StatusCode::UNAUTHORIZED) => (
            StatusCode::UNAUTHORIZED,
            [(header::WWW_AUTHENTICATE, "Bearer")],
        )
            .into_response(),
        Err(code) => code.into_response(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::http::HeaderValue;

    fn headers(host: &str, auth: Option<&str>, origin: Option<&str>) -> HeaderMap {
        let mut h = HeaderMap::new();
        h.insert(header::HOST, HeaderValue::from_str(host).unwrap());
        if let Some(a) = auth {
            h.insert(header::AUTHORIZATION, HeaderValue::from_str(a).unwrap());
        }
        if let Some(o) = origin {
            h.insert(header::ORIGIN, HeaderValue::from_str(o).unwrap());
        }
        h
    }

    #[test]
    fn ct_eq_matches_only_equal_bytes() {
        assert!(ct_eq(b"abc", b"abc"));
        assert!(!ct_eq(b"abc", b"abd"));
        assert!(!ct_eq(b"abc", b"abcd"));
        assert!(!ct_eq(b"", b"a"));
    }

    #[test]
    fn check_needs_local_host_no_browser_origin_and_the_token() {
        let gate = Gate {
            token: "t0k".into(),
            port: 38547,
        };
        let ok = Some("Bearer t0k");
        assert_eq!(check(&headers("127.0.0.1:38547", ok, None), &gate), Ok(()));
        assert_eq!(check(&headers("localhost:38547", ok, None), &gate), Ok(()));
        assert_eq!(
            check(&headers("evil.test:38547", ok, None), &gate),
            Err(StatusCode::FORBIDDEN)
        );
        assert_eq!(
            check(
                &headers("127.0.0.1:38547", ok, Some("https://evil.test")),
                &gate
            ),
            Err(StatusCode::FORBIDDEN)
        );
        assert_eq!(
            check(
                &headers("127.0.0.1:38547", Some("Bearer nope"), None),
                &gate
            ),
            Err(StatusCode::UNAUTHORIZED)
        );
        assert_eq!(
            check(&headers("127.0.0.1:38547", None, None), &gate),
            Err(StatusCode::UNAUTHORIZED)
        );
        let empty = Gate {
            token: String::new(),
            port: 38547,
        };
        assert_eq!(
            check(&headers("127.0.0.1:38547", Some("Bearer "), None), &empty),
            Err(StatusCode::UNAUTHORIZED)
        );
    }
}
