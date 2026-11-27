use std::collections::HashMap;
use std::net::IpAddr;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use sqlx::PgPool;

const MAX_TRACKED_RATE_LIMIT_IPS: usize = 10_000;
pub const DEFAULT_PUBLIC_RELAY_URL: &str = "https://api.modelrelay.io";
pub const DEFAULT_PUBLIC_SITE_URL: &str = "https://modelrelay.io";

/// Build the shared outbound HTTP client used by cloud routes with sane
/// connection and overall request timeouts.
#[must_use]
pub fn default_http_client() -> reqwest::Client {
    reqwest::Client::builder()
        .user_agent("modelrelay-cloud (+https://modelrelay.io)")
        .connect_timeout(Duration::from_secs(5))
        .timeout(Duration::from_secs(15))
        .build()
        .unwrap_or_else(|_| reqwest::Client::new())
}

/// In-memory IP-based rate limiter.
///
/// Tracks timestamps of recent attempts per IP address and rejects requests
/// that exceed `max_attempts` within `window` duration. Bounded to prevent
/// unbounded memory growth under distributed brute-force attacks.
pub struct RateLimiter {
    attempts: Mutex<HashMap<IpAddr, Vec<Instant>>>,
    max_attempts: usize,
    window: Duration,
}

impl RateLimiter {
    /// Create a new rate limiter with the given limits.
    #[must_use]
    pub fn new(max_attempts: usize, window: Duration) -> Self {
        Self {
            attempts: Mutex::new(HashMap::new()),
            max_attempts,
            window,
        }
    }

    /// Check whether the given IP is currently rate-limited.
    /// Returns `true` if the IP has exceeded the limit.
    pub fn is_limited(&self, ip: IpAddr) -> bool {
        let now = Instant::now();
        let mut map = self
            .attempts
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if let Some(timestamps) = map.get_mut(&ip) {
            timestamps.retain(|t| now.duration_since(*t) < self.window);
            let is_over = timestamps.len() >= self.max_attempts;
            if timestamps.is_empty() {
                map.remove(&ip);
            }
            is_over
        } else {
            false
        }
    }

    /// Record an attempt for the given IP. Call this on *failed* auth attempts.
    pub fn record_attempt(&self, ip: IpAddr) {
        let now = Instant::now();
        let mut map = self
            .attempts
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if map.len() >= MAX_TRACKED_RATE_LIMIT_IPS && !map.contains_key(&ip) {
            map.retain(|_, timestamps| {
                timestamps.retain(|t| now.duration_since(*t) < self.window);
                !timestamps.is_empty()
            });
            if map.len() >= MAX_TRACKED_RATE_LIMIT_IPS {
                map.clear();
            }
        }
        let timestamps = map.entry(ip).or_default();
        timestamps.retain(|t| now.duration_since(*t) < self.window);
        timestamps.push(now);
    }
}

/// Shared application state for the commercial `ModelRelay` cloud service.
#[derive(Clone)]
pub struct CloudState {
    /// `PostgreSQL` pool — `None` when `DATABASE_URL` is not set.
    pub db: Option<PgPool>,
    /// Stripe secret key — `None` when `STRIPE_SECRET_KEY` is not set.
    pub stripe_key: Option<String>,
    /// Stripe webhook signing secret — `None` when `STRIPE_WEBHOOK_SECRET` is not set.
    pub webhook_secret: Option<String>,
    /// Internal base URL of the modelrelay-server admin API (e.g. `http://modelrelay-server:8080`).
    pub admin_url: Option<String>,
    /// Bearer token for authenticating with the modelrelay-server admin API.
    pub admin_token: Option<String>,
    /// Publicly reachable relay URL shown to end users in `/`, `/setup`, `/integrate`,
    /// `/AGENTS.md`, `/pi/modelrelay.ts`, and allowed by CSP `connect-src`
    /// (e.g. `https://api.modelrelay.io`).
    pub public_relay_url: String,
    /// Publicly reachable site URL shown in bootstrap prompts, `/AGENTS.md`, and OAuth links
    /// (e.g. `https://modelrelay.io`).
    pub public_site_url: String,
    /// Shared outbound HTTP client with timeouts.
    pub http_client: reqwest::Client,
    /// Lowercase, trimmed admin email addresses from `ADMIN_EMAILS` env var.
    pub admin_emails: Vec<String>,
    /// IP-based rate limiter for auth endpoints (login/signup).
    pub rate_limiter: std::sync::Arc<RateLimiter>,
}

/// Parse the `ADMIN_EMAILS` environment variable into a deduplicated list of
/// lowercase, trimmed email addresses.
#[must_use = "returns the parsed email list"]
pub fn parse_admin_emails(raw: &str) -> Vec<String> {
    raw.split(',')
        .map(|s| s.trim().to_lowercase())
        .filter(|s| !s.is_empty())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_admin_emails_basic() {
        let result = parse_admin_emails("alice@example.com,bob@example.com");
        assert_eq!(result, vec!["alice@example.com", "bob@example.com"]);
    }

    #[test]
    fn parse_admin_emails_handles_whitespace_and_case() {
        let result = parse_admin_emails("  Foo@Example.com , BAR@baz.io ,,");
        assert_eq!(result, vec!["foo@example.com", "bar@baz.io"]);
    }

    #[test]
    fn parse_admin_emails_empty_string() {
        let result = parse_admin_emails("");
        assert!(result.is_empty());
    }

    #[test]
    fn parse_admin_emails_only_commas() {
        let result = parse_admin_emails(",,,");
        assert!(result.is_empty());
    }

    #[test]
    fn parse_admin_emails_single() {
        let result = parse_admin_emails("admin@test.com");
        assert_eq!(result, vec!["admin@test.com"]);
    }

    #[test]
    fn rate_limiter_allows_under_limit() {
        let limiter = RateLimiter::new(3, Duration::from_secs(60));
        let ip: IpAddr = "1.2.3.4".parse().unwrap();
        assert!(!limiter.is_limited(ip));
        limiter.record_attempt(ip);
        limiter.record_attempt(ip);
        assert!(!limiter.is_limited(ip));
    }

    #[test]
    fn rate_limiter_blocks_at_limit() {
        let limiter = RateLimiter::new(3, Duration::from_secs(60));
        let ip: IpAddr = "1.2.3.4".parse().unwrap();
        limiter.record_attempt(ip);
        limiter.record_attempt(ip);
        limiter.record_attempt(ip);
        assert!(limiter.is_limited(ip));
    }

    #[test]
    fn rate_limiter_different_ips_independent() {
        let limiter = RateLimiter::new(2, Duration::from_secs(60));
        let ip1: IpAddr = "1.2.3.4".parse().unwrap();
        let ip2: IpAddr = "5.6.7.8".parse().unwrap();
        limiter.record_attempt(ip1);
        limiter.record_attempt(ip1);
        assert!(limiter.is_limited(ip1));
        assert!(!limiter.is_limited(ip2));
    }

    #[test]
    fn rate_limiter_expired_entries_not_counted() {
        let limiter = RateLimiter::new(2, Duration::from_nanos(1));
        let ip: IpAddr = "1.2.3.4".parse().unwrap();
        limiter.record_attempt(ip);
        limiter.record_attempt(ip);
        std::thread::sleep(Duration::from_millis(1));
        assert!(!limiter.is_limited(ip));
    }

    #[test]
    fn rate_limiter_ipv6() {
        let limiter = RateLimiter::new(2, Duration::from_secs(60));
        let ip: IpAddr = "::1".parse().unwrap();
        limiter.record_attempt(ip);
        limiter.record_attempt(ip);
        assert!(limiter.is_limited(ip));
    }
}
