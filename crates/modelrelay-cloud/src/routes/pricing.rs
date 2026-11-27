use axum::response::{Html, IntoResponse, Response};
use tower_sessions::Session;

use super::csrf;

const PRICING_CSS: &str = r"
  .pricing-hero {
    text-align: center; padding: 64px 0 40px; position: relative; overflow: hidden;
  }
  .pricing-hero::before {
    content: ''; position: absolute; top: -140px; left: 50%; transform: translateX(-50%);
    width: 760px; height: 400px; pointer-events: none;
    background: radial-gradient(ellipse at center, rgba(99, 102, 241, 0.14) 0%, transparent 70%);
  }
  .pricing-hero .container { position: relative; z-index: 1; }
  .pricing-hero h1 { font-size: 2.5rem; font-weight: 800; letter-spacing: -0.03em; color: var(--text-strong); margin-bottom: 10px; }
  .pricing-hero p { color: var(--text-secondary); font-size: 1.05rem; }

  .plans-grid {
    display: grid; grid-template-columns: 1fr 1fr; gap: 22px;
    max-width: 860px; margin: 0 auto 64px;
  }
  .plan-card {
    background: var(--bg-surface); border: 1px solid var(--border);
    border-radius: var(--radius-xl); padding: 34px; display: flex; flex-direction: column;
    box-shadow: var(--shadow-card); position: relative; overflow: hidden;
  }
  .plan-card.featured { border-color: var(--accent-border); }
  .plan-card.featured::before {
    content: ''; position: absolute; top: 0; left: 0; right: 0; height: 3px;
    background: linear-gradient(90deg, var(--accent), var(--cyan));
  }
  .plan-amount { font-size: 2.8rem; font-weight: 800; color: var(--text-strong); margin: 12px 0 4px; letter-spacing: -0.03em; }
  .plan-amount span { font-size: 1rem; font-weight: 500; color: var(--text-muted); }
  .plan-desc { color: var(--text-secondary); font-size: 0.92rem; margin-bottom: 22px; }
  .plan-list { list-style: none; margin: 0 0 28px; display: flex; flex-direction: column; gap: 10px; flex: 1; }
  .plan-list li { position: relative; padding-left: 22px; color: var(--text-primary); font-size: 0.9rem; line-height: 1.5; }
  .plan-list li::before { content: '\2713'; position: absolute; left: 0; top: 0; color: var(--success); font-weight: 700; }

  @media (max-width: 760px) {
    .plans-grid { grid-template-columns: 1fr; }
    .pricing-hero h1 { font-size: 1.9rem; }
  }
";

pub async fn page(session: Session) -> Response {
    let logged_in = session
        .get::<String>("user_id")
        .await
        .ok()
        .flatten()
        .is_some();
    let csrf_field = csrf::hidden_field(&session).await;

    let main_html = format!(
        r#"
  <section class="pricing-hero">
    <div class="container">
      <h1>Simple, Transparent Pricing</h1>
      <p>Run ModelRelay yourself for free (MIT), or use the managed hosted relay at a flat rate.</p>
    </div>
  </section>

  <section class="container">
    <div class="plans-grid">
      <div class="plan-card">
        <div><span class="badge">MIT Open Source</span></div>
        <div class="plan-amount">$0<span>/forever</span></div>
        <p class="plan-desc">Self-host <code>modelrelay-server</code> and <code>modelrelay-worker</code> on your own machine or VPS.</p>
        <ul class="plan-list">
          <li>Unlimited workers, models, and requests</li>
          <li>Desktop tray app &amp; pre-built CLI binaries</li>
          <li>Full OpenAI, Responses &amp; Anthropic APIs</li>
          <li>First-class Pi Coding Agent provider setup</li>
          <li>Docker images &amp; TLS reverse proxy examples</li>
        </ul>
        <a href="/docs/self-hosting" class="btn btn-secondary" style="width:100%;">Self-Hosting Guide &rarr;</a>
      </div>

      <div class="plan-card featured">
        <div><span class="badge badge-accent">Managed Relay</span></div>
        <div class="plan-amount">$20<span>/mo</span></div>
        <p class="plan-desc">Flat rate on <code>api.modelrelay.io</code> with zero per-request or bandwidth fees.</p>
        <ul class="plan-list">
          <li>Managed TLS relay at <code>api.modelrelay.io</code></li>
          <li>Unlimited GPU workers &amp; unlimited requests</li>
          <li>Unbuffered SSE streaming &amp; cancellation</li>
          <li>Live dashboard &amp; API key management</li>
          <li>Cancel anytime from the billing portal</li>
        </ul>
        <form action="/checkout" method="post">
          {csrf_field}
          <button type="submit" class="btn" style="width:100%;">Subscribe Now &rarr;</button>
        </form>
      </div>
    </div>
  </section>"#
    );

    let extra_head = format!("<style>{PRICING_CSS}</style>");
    Html(modelrelay_web::templates::page_shell_full(
        "Pricing",
        "/pricing",
        &main_html,
        logged_in,
        &extra_head,
        "",
    ))
    .into_response()
}
