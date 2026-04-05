use sqlx::PgPool;

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
}
