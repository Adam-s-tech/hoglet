//! Cross-cutting security policy: operator settings, the CSRF guard, response
//! hardening headers, login throttling, and the outbound address policy.
//!
//! Everything here is bounded (no unbounded maps, no unbounded queues) and
//! none of it touches the PostHog wire edge: wire routes carry no ambient
//! credentials, so they are exempt from the CSRF guard and keep their CORS.

use std::collections::HashMap;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use axum::{
    Json,
    extract::Request,
    http::{HeaderMap, HeaderValue, Method, StatusCode, header},
    middleware::Next,
    response::{IntoResponse, Response},
};
use sha2::{Digest, Sha256};

use crate::contract::common::{ApiError, ApiErrorBody};

/// Operator-controlled security settings, read from the environment by the
/// binary. `Default` is the strict, no-environment configuration.
#[derive(Debug, Clone, Default)]
pub struct SecurityConfig {
    /// `HOGLET_SETUP_TOKEN`: when set, first-run setup must present it.
    pub setup_token: Option<String>,
    /// `HOGLET_TRUST_PROXY=1`: the last `X-Forwarded-For` entry is the client.
    pub trust_proxy: bool,
    /// `HOGLET_SECURE_COOKIES=1`: always mark the session cookie `Secure`.
    pub secure_cookies: bool,
    /// `HOGLET_METRICS_TOKEN`: when set, `/metrics` needs `Authorization: Bearer`.
    pub metrics_token: Option<String>,
    /// `HOGLET_ALLOW_PRIVATE_FORWARDING=1`: forwarding may reach private ranges.
    pub allow_private_forwarding: bool,
}

impl SecurityConfig {
    pub fn from_env() -> Self {
        let flag = |name: &str| std::env::var(name).is_ok_and(|value| value == "1");
        let text = |name: &str| std::env::var(name).ok().filter(|value| !value.is_empty());
        Self {
            setup_token: text("HOGLET_SETUP_TOKEN"),
            trust_proxy: flag("HOGLET_TRUST_PROXY"),
            secure_cookies: flag("HOGLET_SECURE_COOKIES"),
            metrics_token: text("HOGLET_METRICS_TOKEN"),
            allow_private_forwarding: flag("HOGLET_ALLOW_PRIVATE_FORWARDING"),
        }
    }
}

/// Compare secrets without an early exit on the first differing byte.
pub fn constant_time_eq(left: &str, right: &str) -> bool {
    let left = Sha256::digest(left.as_bytes());
    let right = Sha256::digest(right.as_bytes());
    left.iter()
        .zip(right.iter())
        .fold(0_u8, |difference, (a, b)| difference | (a ^ b))
        == 0
}

/// Peer address of the TCP connection, attached by the server loop.
#[derive(Debug, Clone, Copy)]
pub struct PeerAddr(pub IpAddr);

/// Whether the request arrived over TLS according to the reverse proxy.
pub fn forwarded_https(headers: &HeaderMap) -> bool {
    headers
        .get("x-forwarded-proto")
        .and_then(|value| value.to_str().ok())
        .is_some_and(|value| {
            value
                .split(',')
                .next_back()
                .is_some_and(|proto| proto.trim().eq_ignore_ascii_case("https"))
        })
}

// ---------------------------------------------------------------------------
// CSRF
// ---------------------------------------------------------------------------

fn error_body(code: &str, message: &str) -> Json<ApiError> {
    Json(ApiError {
        error: ApiErrorBody {
            code: code.to_owned(),
            message: message.to_owned(),
            request_id: None,
        },
    })
}

fn has_session_cookie(headers: &HeaderMap) -> bool {
    headers
        .get_all(header::COOKIE)
        .iter()
        .filter_map(|value| value.to_str().ok())
        .any(|cookies| {
            cookies
                .split(';')
                .any(|cookie| cookie.trim().starts_with("hoglet_sid="))
        })
}

/// Authority (`host[:port]`) of an `Origin` header value.
fn origin_authority(origin: &str) -> Option<&str> {
    let (_, rest) = origin.split_once("://")?;
    let authority = rest.split('/').next().unwrap_or(rest);
    (!authority.is_empty()).then_some(authority)
}

/// True when a browser says this request was initiated by another site.
///
/// `Sec-Fetch-Site` is authoritative where the browser sends it; otherwise the
/// `Origin` authority must equal the `Host` the request was addressed to.
/// A request carrying neither header did not come from a browser, so no
/// ambient-credential attack applies.
pub fn is_cross_site(headers: &HeaderMap) -> bool {
    if let Some(site) = headers
        .get("sec-fetch-site")
        .and_then(|value| value.to_str().ok())
    {
        return !matches!(site, "same-origin" | "none");
    }
    let Some(origin) = headers
        .get(header::ORIGIN)
        .and_then(|value| value.to_str().ok())
    else {
        return false;
    };
    let host = headers
        .get("x-forwarded-host")
        .or_else(|| headers.get(header::HOST))
        .and_then(|value| value.to_str().ok());
    match (origin_authority(origin), host) {
        (Some(origin), Some(host)) => !origin.eq_ignore_ascii_case(host.trim()),
        _ => true,
    }
}

/// Reject state-changing requests that carry the session cookie but were
/// initiated cross-site. `SameSite=Lax` already stops third-party sites; this
/// also closes same-site siblings (other subdomains) and old browsers.
/// Requests authenticated by `Authorization: Bearer` carry no cookie and are
/// unaffected.
pub async fn csrf_guard(request: Request, next: Next) -> Response {
    let safe = matches!(
        *request.method(),
        Method::GET | Method::HEAD | Method::OPTIONS
    );
    if !safe && has_session_cookie(request.headers()) && is_cross_site(request.headers()) {
        return (
            StatusCode::FORBIDDEN,
            error_body("forbidden", "Cross-site request refused."),
        )
            .into_response();
    }
    next.run(request).await
}

// ---------------------------------------------------------------------------
// Response headers
// ---------------------------------------------------------------------------

/// Content-Security-Policy for the embedded dashboard. Scripts run only from
/// this origin plus the hashed inline theme bootstrap; nothing may frame it.
pub fn dashboard_csp(inline_script_hashes: &[String]) -> String {
    let mut script = String::from("script-src 'self'");
    for hash in inline_script_hashes {
        script.push_str(" '");
        script.push_str(hash);
        script.push('\'');
    }
    format!(
        "default-src 'self'; {script}; style-src 'self' 'unsafe-inline'; \
         img-src 'self' data:; font-src 'self' data:; connect-src 'self'; \
         object-src 'none'; base-uri 'none'; form-action 'self'; frame-ancestors 'none'"
    )
}

/// CSP for the API reference page (a vendored single-file viewer that needs
/// inline styles and scripts, but never talks to a third party).
pub const DOCS_CSP: &str = "default-src 'self'; script-src 'self' 'unsafe-inline' 'unsafe-eval'; \
     style-src 'self' 'unsafe-inline'; img-src 'self' data:; font-src 'self' data:; \
     connect-src 'self'; object-src 'none'; base-uri 'none'; frame-ancestors 'none'";

/// Baseline hardening on every response, wire routes included. These headers
/// cannot change how an SDK uses an endpoint.
pub async fn security_headers(request: Request, next: Next) -> Response {
    let path = request.uri().path().to_owned();
    let https = forwarded_https(request.headers());
    let mut response = next.run(request).await;
    let headers = response.headers_mut();
    headers
        .entry("x-content-type-options")
        .or_insert(HeaderValue::from_static("nosniff"));
    headers
        .entry("referrer-policy")
        .or_insert(HeaderValue::from_static("no-referrer"));
    let html = headers
        .get(header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .is_some_and(|value| value.starts_with("text/html"));
    if html {
        headers
            .entry("x-frame-options")
            .or_insert(HeaderValue::from_static("DENY"));
        headers
            .entry("content-security-policy")
            .or_insert(HeaderValue::from_static(
                "default-src 'none'; frame-ancestors 'none'",
            ));
    }
    // Authenticated JSON must not sit in shared caches. Local evaluation has
    // its own ETag protocol and is left alone.
    if path.starts_with("/api/")
        && !path.starts_with("/api/feature_flag/")
        && !headers.contains_key(header::CACHE_CONTROL)
    {
        headers.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    }
    if https {
        headers
            .entry("strict-transport-security")
            .or_insert(HeaderValue::from_static("max-age=31536000"));
    }
    response
}

// ---------------------------------------------------------------------------
// Login throttling
// ---------------------------------------------------------------------------

/// Failures per email before attempts start being delayed.
const EMAIL_FREE_FAILURES: u32 = 5;
/// Longest delay imposed on one email.
const EMAIL_MAX_DELAY: Duration = Duration::from_secs(60);
/// How long failures are remembered for an email.
const EMAIL_WINDOW: Duration = Duration::from_secs(15 * 60);
/// Failures per source address per window before it is refused.
const SOURCE_MAX_FAILURES: u32 = 30;
const SOURCE_WINDOW: Duration = Duration::from_secs(10 * 60);
/// Cap on tracked keys per table.
const MAX_TRACKED: usize = 4096;

#[derive(Clone, Copy)]
struct Entry {
    failures: u32,
    since: Instant,
    blocked_until: Option<Instant>,
}

#[derive(Default)]
struct Tables {
    emails: HashMap<String, Entry>,
    sources: HashMap<String, Entry>,
}

/// Bounded per-email backoff and per-source failure cap for password logins.
///
/// An email is delayed (1 s doubling to 60 s) after five consecutive
/// failures, so brute force is held to about one guess a minute while the
/// real owner is never locked out for longer than that. A source address that
/// fails 30 times in ten minutes is refused for the rest of the window.
#[derive(Default)]
pub struct LoginThrottle {
    tables: Mutex<Tables>,
}

fn normalize_email(email: &str) -> String {
    email
        .trim()
        .chars()
        .take(254)
        .collect::<String>()
        .to_ascii_lowercase()
}

fn insert_bounded(table: &mut HashMap<String, Entry>, key: String, now: Instant, window: Duration) {
    if !table.contains_key(&key) && table.len() >= MAX_TRACKED {
        table.retain(|_, entry| {
            now.duration_since(entry.since) < window
                || entry.blocked_until.is_some_and(|until| until > now)
        });
        if table.len() >= MAX_TRACKED
            && let Some(oldest) = table
                .iter()
                .min_by_key(|(_, entry)| entry.since)
                .map(|(key, _)| key.clone())
        {
            table.remove(&oldest);
        }
    }
    table.entry(key).or_insert(Entry {
        failures: 0,
        since: now,
        blocked_until: None,
    });
}

impl LoginThrottle {
    /// `Err(retry_after)` when this attempt must be refused without hashing.
    pub fn check(&self, email: &str, source: Option<&str>, now: Instant) -> Result<(), Duration> {
        let tables = self
            .tables
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let mut wait = Duration::ZERO;
        if let Some(entry) = tables.emails.get(&normalize_email(email))
            && let Some(until) = entry.blocked_until
            && until > now
        {
            wait = wait.max(until - now);
        }
        if let Some(entry) = source.and_then(|source| tables.sources.get(source))
            && let Some(until) = entry.blocked_until
            && until > now
        {
            wait = wait.max(until - now);
        }
        if wait.is_zero() { Ok(()) } else { Err(wait) }
    }

    pub fn record_failure(&self, email: &str, source: Option<&str>, now: Instant) {
        let mut tables = self
            .tables
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let key = normalize_email(email);
        insert_bounded(&mut tables.emails, key.clone(), now, EMAIL_WINDOW);
        if let Some(entry) = tables.emails.get_mut(&key) {
            if now.duration_since(entry.since) >= EMAIL_WINDOW {
                entry.failures = 0;
                entry.since = now;
            }
            entry.failures = entry.failures.saturating_add(1);
            if entry.failures >= EMAIL_FREE_FAILURES {
                let shift = (entry.failures - EMAIL_FREE_FAILURES).min(6);
                let delay = Duration::from_secs(1_u64 << shift).min(EMAIL_MAX_DELAY);
                entry.blocked_until = Some(now + delay);
            }
        }
        if let Some(source) = source {
            insert_bounded(&mut tables.sources, source.to_owned(), now, SOURCE_WINDOW);
            if let Some(entry) = tables.sources.get_mut(source) {
                if now.duration_since(entry.since) >= SOURCE_WINDOW {
                    entry.failures = 0;
                    entry.since = now;
                    entry.blocked_until = None;
                }
                entry.failures = entry.failures.saturating_add(1);
                if entry.failures >= SOURCE_MAX_FAILURES {
                    entry.blocked_until = Some(entry.since + SOURCE_WINDOW);
                }
            }
        }
    }

    pub fn record_success(&self, email: &str) {
        self.tables
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .emails
            .remove(&normalize_email(email));
    }
}

/// The source key used by the per-source cap. Addresses that are the reverse
/// proxy itself (loopback or private) are not keyed unless the operator said
/// the proxy's `X-Forwarded-For` is trustworthy: one shared bucket for every
/// user behind the proxy would let a stranger lock everybody out.
pub fn source_key(peer: Option<IpAddr>, headers: &HeaderMap, trust_proxy: bool) -> Option<String> {
    if trust_proxy
        && let Some(last) = headers
            .get("x-forwarded-for")
            .and_then(|value| value.to_str().ok())
            .and_then(|value| value.rsplit(',').next())
            .map(str::trim)
            .filter(|value| !value.is_empty() && value.len() <= 64)
    {
        return Some(last.to_owned());
    }
    let peer = peer?;
    (!is_local_or_private(peer)).then(|| peer.to_string())
}

// ---------------------------------------------------------------------------
// Outbound address policy
// ---------------------------------------------------------------------------

fn is_local_or_private(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(v4) => is_forbidden_v4(v4),
        IpAddr::V6(v6) => is_forbidden_v6(v6),
    }
}

fn is_forbidden_v4(ip: Ipv4Addr) -> bool {
    let [a, b, ..] = ip.octets();
    ip.is_loopback()
        || ip.is_private()
        || ip.is_link_local() // 169.254.0.0/16, includes the cloud metadata address
        || ip.is_unspecified()
        || ip.is_broadcast()
        || ip.is_multicast()
        || a == 0
        || (a == 100 && (64..128).contains(&b)) // carrier-grade NAT
        || (a == 192 && b == 0 && ip.octets()[2] == 0) // 192.0.0.0/24
        || a >= 240
}

fn is_forbidden_v6(ip: Ipv6Addr) -> bool {
    if let Some(v4) = ip.to_ipv4_mapped() {
        return is_forbidden_v4(v4);
    }
    let first = ip.segments()[0];
    ip.is_loopback()
        || ip.is_unspecified()
        || ip.is_multicast()
        || (first & 0xfe00) == 0xfc00 // unique local fc00::/7
        || (first & 0xffc0) == 0xfe80 // link local fe80::/10
        || (first == 0x64 && ip.segments()[1] == 0xff9b) // NAT64 64:ff9b::/96
}

/// Whether server-side requests to `ip` are refused by the outbound policy
/// (loopback, link-local and metadata, private, CGNAT, multicast, reserved).
pub fn is_forbidden_address(ip: IpAddr) -> bool {
    is_local_or_private(ip)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn headers(pairs: &[(&'static str, &'static str)]) -> HeaderMap {
        let mut map = HeaderMap::new();
        for (name, value) in pairs {
            map.insert(*name, HeaderValue::from_static(value));
        }
        map
    }

    #[test]
    fn cross_site_detection() {
        assert!(!is_cross_site(&headers(&[])));
        assert!(!is_cross_site(&headers(&[("sec-fetch-site", "same-origin")])));
        assert!(is_cross_site(&headers(&[("sec-fetch-site", "cross-site")])));
        assert!(is_cross_site(&headers(&[("sec-fetch-site", "same-site")])));
        assert!(!is_cross_site(&headers(&[
            ("origin", "https://a.example"),
            ("host", "a.example"),
        ])));
        assert!(is_cross_site(&headers(&[
            ("origin", "https://evil.example"),
            ("host", "a.example"),
        ])));
        assert!(is_cross_site(&headers(&[("origin", "null"), ("host", "a.example")])));
        // Sec-Fetch-Site outranks Origin.
        assert!(!is_cross_site(&headers(&[
            ("sec-fetch-site", "same-origin"),
            ("origin", "http://127.0.0.1:8000"),
            ("host", "hoglet.internal"),
        ])));
    }

    #[test]
    fn throttle_delays_after_five_failures_and_resets_on_success() {
        let throttle = LoginThrottle::default();
        let start = Instant::now();
        for _ in 0..4 {
            throttle.record_failure("A@b.co", None, start);
            assert!(throttle.check("a@b.co", None, start).is_ok());
        }
        throttle.record_failure("a@b.co ", None, start);
        let wait = throttle.check("a@b.co", None, start).unwrap_err();
        assert_eq!(wait, Duration::from_secs(1));
        assert!(
            throttle
                .check("a@b.co", None, start + Duration::from_millis(1100))
                .is_ok()
        );
        for _ in 0..20 {
            throttle.record_failure("a@b.co", None, start);
        }
        assert_eq!(
            throttle.check("a@b.co", None, start).unwrap_err(),
            EMAIL_MAX_DELAY
        );
        // Another email is unaffected.
        assert!(throttle.check("c@d.co", None, start).is_ok());
        throttle.record_success("a@b.co");
        assert!(throttle.check("a@b.co", None, start).is_ok());
    }

    #[test]
    fn source_cap_blocks_then_expires() {
        let throttle = LoginThrottle::default();
        let start = Instant::now();
        for index in 0..SOURCE_MAX_FAILURES {
            throttle.record_failure(&format!("u{index}@x.co"), Some("9.9.9.9"), start);
        }
        assert!(throttle.check("fresh@x.co", Some("9.9.9.9"), start).is_err());
        assert!(throttle.check("fresh@x.co", Some("8.8.8.8"), start).is_ok());
        assert!(
            throttle
                .check("fresh@x.co", Some("9.9.9.9"), start + SOURCE_WINDOW)
                .is_ok()
        );
    }

    #[test]
    fn tables_stay_bounded() {
        let throttle = LoginThrottle::default();
        let start = Instant::now();
        for index in 0..(MAX_TRACKED * 2) {
            throttle.record_failure(&format!("u{index}@x.co"), Some(&format!("s{index}")), start);
        }
        let tables = throttle.tables.lock().unwrap();
        assert!(tables.emails.len() <= MAX_TRACKED);
        assert!(tables.sources.len() <= MAX_TRACKED);
    }

    #[test]
    fn outbound_policy_refuses_internal_ranges() {
        for ip in [
            "127.0.0.1",
            "10.1.2.3",
            "172.16.0.1",
            "192.168.1.1",
            "169.254.169.254",
            "100.100.100.200",
            "0.0.0.0",
            "::1",
            "fe80::1",
            "fd00::1",
            "::ffff:127.0.0.1",
            "::ffff:169.254.169.254",
        ] {
            assert!(is_forbidden_address(ip.parse().unwrap()), "{ip}");
        }
        for ip in ["8.8.8.8", "104.18.0.1", "2606:4700::1111"] {
            assert!(!is_forbidden_address(ip.parse().unwrap()), "{ip}");
        }
    }

    #[test]
    fn constant_time_eq_matches_equality() {
        assert!(constant_time_eq("secret", "secret"));
        assert!(!constant_time_eq("secret", "secreT"));
        assert!(!constant_time_eq("secret", "secret2"));
    }

    #[test]
    fn source_key_ignores_proxy_peers_unless_trusted() {
        let none = HeaderMap::new();
        assert_eq!(source_key(Some("127.0.0.1".parse().unwrap()), &none, false), None);
        assert_eq!(
            source_key(Some("203.0.113.9".parse().unwrap()), &none, false).as_deref(),
            Some("203.0.113.9")
        );
        let forwarded = headers(&[("x-forwarded-for", "1.1.1.1, 203.0.113.7")]);
        assert_eq!(
            source_key(Some("127.0.0.1".parse().unwrap()), &forwarded, true).as_deref(),
            Some("203.0.113.7")
        );
    }
}
