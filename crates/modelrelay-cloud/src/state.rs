use std::collections::HashMap;
use std::net::IpAddr;
use std::sync::Mutex;
use std::time::Instant;

use sqlx::PgPool;

/// In-memory IP-based rate limiter.
///
/// Tracks timestamps of recent attempts per IP address and rejects requests
/// that exceed `max_attempts` within `window` duration.
pub struct RateLimiter {
    attempts: Mutex<HashMap<IpAddr, Vec<Instant>>>,
    max_attempts: usize,
    window: std::time::Duration,
}

impl RateLimiter {
    /// Create a new rate limiter with the given limits.
    #[must_use]
    pub fn new(max_attempts: usize, window: std::time::Duration) -> Self {
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
        let mut map = self.attempts.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(timestamps) = map.get_mut(&ip) {
            timestamps.retain(|t| now.duration_since(*t) < self.window);
            timestamps.len() >= self.max_attempts
        } else {
            false
        }
    }

    /// Record an attempt for the given IP. Call this on *failed* auth attempts.
    pub fn record_attempt(&self, ip: IpAddr) {
        let now = Instant::now();
        let mut map = self.attempts.lock().unwrap_or_else(|e| e.into_inner());
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
    /// Base URL of the modelrelay-server admin API (e.g. `http://modelrelay-server:8080`).
    pub admin_url: Option<String>,
    /// Bearer token for authenticating with the modelrelay-server admin API.
    pub admin_token: Option<String>,
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
        let limiter = RateLimiter::new(3, std::time::Duration::from_secs(60));
        let ip: IpAddr = "1.2.3.4".parse().unwrap();
        assert!(!limiter.is_limited(ip));
        limiter.record_attempt(ip);
        limiter.record_attempt(ip);
        assert!(!limiter.is_limited(ip)); // 2 attempts, limit is 3
    }

    #[test]
    fn rate_limiter_blocks_at_limit() {
        let limiter = RateLimiter::new(3, std::time::Duration::from_secs(60));
        let ip: IpAddr = "1.2.3.4".parse().unwrap();
        limiter.record_attempt(ip);
        limiter.record_attempt(ip);
        limiter.record_attempt(ip);
        assert!(limiter.is_limited(ip)); // 3 attempts = blocked
    }

    #[test]
    fn rate_limiter_different_ips_independent() {
        let limiter = RateLimiter::new(2, std::time::Duration::from_secs(60));
        let ip1: IpAddr = "1.2.3.4".parse().unwrap();
        let ip2: IpAddr = "5.6.7.8".parse().unwrap();
        limiter.record_attempt(ip1);
        limiter.record_attempt(ip1);
        assert!(limiter.is_limited(ip1));
        assert!(!limiter.is_limited(ip2)); // different IP, no attempts
    }

    #[test]
    fn rate_limiter_expired_entries_not_counted() {
        // Use a very short window so entries expire immediately
        let limiter = RateLimiter::new(2, std::time::Duration::from_nanos(1));
        let ip: IpAddr = "1.2.3.4".parse().unwrap();
        limiter.record_attempt(ip);
        limiter.record_attempt(ip);
        // Entries should have expired by now (1ns window)
        std::thread::sleep(std::time::Duration::from_millis(1));
        assert!(!limiter.is_limited(ip));
    }

    #[test]
    fn rate_limiter_ipv6() {
        let limiter = RateLimiter::new(2, std::time::Duration::from_secs(60));
        let ip: IpAddr = "::1".parse().unwrap();
        limiter.record_attempt(ip);
        limiter.record_attempt(ip);
        assert!(limiter.is_limited(ip));
    }
}
