//! Static legal pages: Terms of Service and Privacy Policy.
//!
//! These are required for Stripe-gated `SaaS` products (Stripe expects checkout
//! to link to both) and for basic user trust at signup time. They are
//! deliberately session-exempt so unauthenticated visitors can read them.

use axum::response::{Html, IntoResponse, Response};

use super::OptionalSession;

const WRAPPER_OPEN: &str = r#"<div style="max-width:740px;line-height:1.75;">"#;
const WRAPPER_CLOSE: &str = "</div>";

async fn is_logged_in(session: Option<tower_sessions::Session>) -> bool {
    match session {
        Some(s) => s.get::<String>("user_id").await.ok().flatten().is_some(),
        None => false,
    }
}

/// GET `/terms` — Terms of Service.
pub async fn terms_page(OptionalSession(session): OptionalSession) -> Response {
    let logged_in = is_logged_in(session).await;
    let body = format!(
        "{WRAPPER_OPEN}{inner}{WRAPPER_CLOSE}",
        inner = terms_inner()
    );
    Html(modelrelay_web::templates::page_shell(
        "Terms of Service",
        "/terms",
        &body,
        logged_in,
    ))
    .into_response()
}

/// GET `/privacy` — Privacy Policy.
pub async fn privacy_page(OptionalSession(session): OptionalSession) -> Response {
    let logged_in = is_logged_in(session).await;
    let body = format!(
        "{WRAPPER_OPEN}{inner}{WRAPPER_CLOSE}",
        inner = privacy_inner()
    );
    Html(modelrelay_web::templates::page_shell(
        "Privacy Policy",
        "/privacy",
        &body,
        logged_in,
    ))
    .into_response()
}

fn terms_inner() -> &'static str {
    r#"<p><em>Last updated: 2026-04-16</em></p>

<section>
  <h2>1. Acceptance of Terms</h2>
  <p>By creating an account, subscribing to a paid plan, or otherwise using ModelRelay ("the Service"), you agree to these Terms of Service. If you do not agree, do not use the Service.</p>
</section>

<section>
  <h2>2. Description of Service</h2>
  <p>ModelRelay is a managed LLM relay. It routes inference requests from your applications to your own GPU worker endpoints and/or third-party model providers (for example, OpenAI or Anthropic). You are responsible for configuring the workers and providers that your traffic is routed to.</p>
</section>

<section>
  <h2>3. Account Responsibilities</h2>
  <ul>
    <li>You must provide accurate signup information and keep your account credentials safe.</li>
    <li>You are responsible for anything sent through your API keys, including the behavior of your workers and any content your end users submit.</li>
    <li>You must not share your API keys publicly or embed them in client-side code distributed to untrusted users.</li>
  </ul>
</section>

<section>
  <h2>4. Acceptable Use</h2>
  <p>You agree not to use the Service to:</p>
  <ul>
    <li>Transmit illegal content or content that violates another party's rights.</li>
    <li>Circumvent or abuse the acceptable-use policies of any third-party model provider we route to on your behalf.</li>
    <li>Resell raw ModelRelay or upstream provider API keys, or operate the Service as a passthrough key broker.</li>
    <li>Attempt to disrupt, reverse engineer, or gain unauthorized access to the Service.</li>
  </ul>
</section>

<section>
  <h2>5. Payment and Cancellation</h2>
  <p>Paid subscriptions are billed through Stripe on a recurring monthly basis. You may cancel at any time from your dashboard or the Stripe billing portal. Cancellation takes effect at the end of the current billing period; we do not issue refunds for partial months.</p>
</section>

<section>
  <h2>6. Service Availability and Rate Limits</h2>
  <p>We operate the Service on a best-effort basis. At this tier we do not offer a written uptime SLA. We may apply per-account rate limits to protect the Service or upstream providers, and we may adjust those limits over time.</p>
</section>

<section>
  <h2>7. Suspension and Termination</h2>
  <p>We may suspend or terminate accounts that violate these Terms or our acceptable-use expectations, that incur chargebacks, or that we reasonably believe put the Service or other users at risk. You may close your account at any time.</p>
</section>

<section>
  <h2>8. Disclaimers</h2>
  <p>THE SERVICE IS PROVIDED "AS IS" AND "AS AVAILABLE" WITHOUT WARRANTIES OF ANY KIND, EXPRESS OR IMPLIED, INCLUDING MERCHANTABILITY, FITNESS FOR A PARTICULAR PURPOSE, AND NON-INFRINGEMENT. We make no guarantees about the accuracy, quality, or availability of outputs returned by upstream model providers or your workers.</p>
</section>

<section>
  <h2>9. Limitation of Liability</h2>
  <p>To the fullest extent permitted by law, our aggregate liability to you for any claim arising out of or relating to the Service is limited to the fees you paid to ModelRelay for the Service in the twelve (12) months immediately preceding the claim. We are not liable for indirect, incidental, consequential, special, or punitive damages.</p>
</section>

<section>
  <h2>10. Governing Law and Venue</h2>
  <p>These Terms are governed by the laws of the State of California, excluding its conflict-of-laws principles. Any dispute will be resolved in the state or federal courts located in San Francisco County, California, and you consent to personal jurisdiction and venue there.</p>
</section>

<section>
  <h2>11. Changes to These Terms</h2>
  <p>We may update these Terms over time. For material changes, we will email the address on your account at least 30 days before the change takes effect. Continued use of the Service after a change takes effect constitutes acceptance of the updated Terms.</p>
</section>

<section>
  <h2>12. Contact</h2>
  <p>If you have questions, email <a href="mailto:support@modelrelay.io">support@modelrelay.io</a>.</p>
</section>"#
}

fn privacy_inner() -> &'static str {
    r#"<p><em>Last updated: 2026-04-16</em></p>

<section>
  <h2>1. Data We Collect at Sign-up</h2>
  <ul>
    <li>Email address (used for login, receipts, and service notices).</li>
    <li>A hash of your password (we use Argon2id; we never store the plaintext password).</li>
  </ul>
</section>

<section>
  <h2>2. Data We Collect During Use</h2>
  <p>When you route requests through ModelRelay we record metadata about each request:</p>
  <ul>
    <li>Your user id, the API key id used, and the timestamp of the request.</li>
    <li>The model name, token counts (input and output), latency, and success or error status.</li>
  </ul>
  <p>We do <strong>not</strong> persist the bodies of your requests or the responses returned to you. Request and response payloads are forwarded between your application, our relay, and the selected provider or worker, and are not retained after the request completes.</p>
</section>

<section>
  <h2>3. Payment Information</h2>
  <p>Billing is handled by Stripe. ModelRelay stores only a Stripe customer id and the current subscription state. We never see or store full card numbers.</p>
</section>

<section>
  <h2>4. Cookies</h2>
  <p>We use a single session cookie to keep you logged in. It is marked <code>HttpOnly</code>, <code>Secure</code>, and <code>SameSite=Lax</code>. We do not use third-party analytics cookies, advertising trackers, or cross-site tracking.</p>
</section>

<section>
  <h2>5. API Keys</h2>
  <p>API keys you generate in your dashboard are stored as raw values so that they can be redisplayed to you. Treat your API keys like passwords; do not share or embed them in untrusted clients. You can revoke and rotate keys at any time from the dashboard.</p>
</section>

<section>
  <h2>6. How We Use Your Data</h2>
  <ul>
    <li>To operate, monitor, and improve the Service.</li>
    <li>To enforce per-account rate limits and prevent abuse.</li>
    <li>To send essential service emails (receipts, security notices, incident communications).</li>
    <li>To comply with legal obligations.</li>
  </ul>
</section>

<section>
  <h2>7. Data Sharing</h2>
  <p>We share request metadata only with the parties necessary to fulfill each request:</p>
  <ul>
    <li>The LLM provider(s) you route to (for example, OpenAI, Anthropic, or your own workers).</li>
    <li>Stripe, for subscription billing.</li>
  </ul>
  <p>We do not sell personal data, and we do not share it with advertisers.</p>
</section>

<section>
  <h2>8. Retention</h2>
  <ul>
    <li>Account data is retained while your account is active.</li>
    <li>On account deletion, account data is removed within 30 days, except where retention is required by law.</li>
    <li>Per-request usage metadata is retained for up to 90 days, then deleted or aggregated.</li>
  </ul>
</section>

<section>
  <h2>9. Security</h2>
  <p>Passwords are hashed with Argon2id. Traffic to ModelRelay is served over TLS. Internal access to production databases is limited to operators who need it. No system is perfectly secure, but we take reasonable measures to protect your data.</p>
</section>

<section>
  <h2>10. Your Rights</h2>
  <p>You can access, export, or delete your account data by emailing <a href="mailto:support@modelrelay.io">support@modelrelay.io</a>. We will respond within a reasonable timeframe and in accordance with applicable law.</p>
</section>

<section>
  <h2>11. Children</h2>
  <p>ModelRelay is not directed to children under 13, and we do not knowingly collect personal information from them.</p>
</section>

<section>
  <h2>12. Jurisdiction</h2>
  <p>ModelRelay is operated from the United States and your data is processed on infrastructure located in the United States.</p>
</section>

<section>
  <h2>13. Changes to This Policy</h2>
  <p>We may update this Privacy Policy over time. Material changes will be announced by email to the address on your account before they take effect.</p>
</section>

<section>
  <h2>14. Contact</h2>
  <p>If you have questions, email <a href="mailto:support@modelrelay.io">support@modelrelay.io</a>.</p>
</section>"#
}
