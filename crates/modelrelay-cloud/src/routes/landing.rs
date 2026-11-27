use axum::response::{Html, IntoResponse, Response};

use super::OptionalSession;

const LANDING_CSS: &str = r"
  .hero {
    position: relative; overflow: hidden;
    padding: 76px 0 68px;
    border-bottom: 1px solid var(--border);
  }
  .hero::before {
    content: ''; position: absolute; top: -180px; left: 50%; transform: translateX(-50%);
    width: 960px; height: 540px; pointer-events: none; z-index: 0;
    background: radial-gradient(ellipse at center, rgba(99, 102, 241, 0.16) 0%, rgba(56, 189, 248, 0.06) 42%, transparent 72%);
  }
  .hero-grid {
    position: relative; z-index: 1;
    display: grid; grid-template-columns: 1.02fr 0.98fr; gap: 40px; align-items: center;
  }
  .eyebrow-pill {
    display: inline-flex; align-items: center; gap: 8px;
    padding: 5px 12px; border-radius: 999px; margin-bottom: 20px;
    background: var(--accent-soft); border: 1px solid var(--accent-border);
    font-size: 0.78rem; font-weight: 600; color: var(--accent-fg);
  }
  .eyebrow-dot { width: 7px; height: 7px; border-radius: 50%; background: var(--success); }
  .hero h1 {
    font-size: 3.15rem; font-weight: 800; line-height: 1.07; letter-spacing: -0.035em;
    color: var(--text-strong); margin-bottom: 18px;
  }
  .hero h1 .hl {
    background: linear-gradient(120deg, var(--hero-hl-from) 0%, var(--hero-hl-to) 100%);
    -webkit-background-clip: text; background-clip: text; color: transparent;
  }
  .hero-sub {
    font-size: 1.08rem; color: var(--text-secondary); line-height: 1.65;
    margin-bottom: 26px; max-width: 540px;
  }
  .hero-cta { display: flex; gap: 12px; flex-wrap: wrap; margin-bottom: 24px; }
  .hero-meta {
    display: flex; gap: 16px; flex-wrap: wrap; align-items: center;
    font-size: 0.82rem; color: var(--text-muted);
  }
  .hero-meta span { display: inline-flex; align-items: center; gap: 6px; }

  /* Hero Terminal Card (crisp dark terminal in both light and dark themes) */
  .hero-terminal {
    background: var(--bg-code); border: 1px solid var(--code-border);
    border-radius: var(--radius-xl); overflow: hidden;
    box-shadow: var(--shadow-hero);
  }
  .term-top {
    display: flex; align-items: center; justify-content: space-between; gap: 10px;
    padding: 10px 14px; background: var(--bg-code-head);
    border-bottom: 1px solid rgba(148, 163, 184, 0.16);
  }
  .term-dots { display: flex; gap: 6px; }
  .term-dots i { width: 10px; height: 10px; border-radius: 50%; display: block; background: rgba(148, 163, 184, 0.3); }
  .term-tabs { display: flex; gap: 4px; }
  .term-tab {
    padding: 4px 10px; border-radius: 6px; border: 1px solid transparent;
    background: transparent; color: var(--code-muted); font-size: 0.78rem; font-weight: 600;
    cursor: pointer; font-family: inherit;
  }
  .term-tab:hover { color: var(--code-fg); }
  .term-tab.active {
    background: rgba(99, 102, 241, 0.22); border-color: rgba(129, 140, 248, 0.45); color: #e0e7ff;
  }
  .term-pane { display: none; background: var(--bg-code); }
  .term-pane.active { display: block; }
  .term-pane pre {
    padding: 18px; margin: 0; overflow-x: auto;
    font-size: 0.8rem; line-height: 1.65; color: var(--code-fg);
  }
  .term-foot {
    display: flex; justify-content: space-between; align-items: center; gap: 10px; flex-wrap: wrap;
    padding: 10px 16px; border-top: 1px solid rgba(148, 163, 184, 0.16);
    background: var(--bg-code-head); font-size: 0.78rem; color: var(--code-muted);
  }
  .term-foot a { color: #a5b4fc; font-weight: 600; }
  .tk-c { color: #64748b; }
  .tk-k { color: #818cf8; }
  .tk-s { color: #38bdf8; }
  .tk-g { color: #34d399; }

  /* Sections */
  .section { padding: 76px 0; border-bottom: 1px solid var(--border); }
  .section-alt { background: var(--bg-section-alt); }
  .section-head { max-width: 680px; margin-bottom: 40px; }
  .section-head.center { text-align: center; margin-left: auto; margin-right: auto; }
  .eyebrow {
    font-size: 0.74rem; font-weight: 700; text-transform: uppercase;
    letter-spacing: 0.09em; color: var(--accent-bright); margin-bottom: 8px;
  }
  .section-head h2 {
    font-size: 2.05rem; font-weight: 800; letter-spacing: -0.03em; line-height: 1.15;
    margin-bottom: 12px; color: var(--text-strong);
  }
  .section-head p { color: var(--text-secondary); font-size: 1rem; }

  /* Topology Diagram */
  .topo-card {
    background: var(--bg-surface); border: 1px solid var(--border);
    border-radius: var(--radius-xl); padding: 28px; margin-bottom: 28px;
    box-shadow: var(--shadow-card);
  }
  .topo-grid {
    display: grid; grid-template-columns: 1fr auto 1.15fr auto 1fr; gap: 14px; align-items: center;
  }
  .topo-node {
    background: var(--bg-elevated); border: 1px solid var(--border);
    border-radius: var(--radius-md); padding: 18px;
  }
  .topo-node.relay {
    border-color: var(--accent-border);
    background: var(--accent-soft);
  }
  .topo-tag {
    font-family: var(--font-mono); font-size: 0.7rem; font-weight: 700;
    text-transform: uppercase; letter-spacing: 0.07em; color: var(--text-muted); margin-bottom: 6px;
  }
  .topo-node h3 { font-size: 0.98rem; font-weight: 700; color: var(--text-strong); margin-bottom: 6px; }
  .topo-node p { font-size: 0.82rem; color: var(--text-secondary); margin: 0; line-height: 1.55; }
  .topo-arrow {
    display: flex; flex-direction: column; align-items: center; gap: 4px;
    font-family: var(--font-mono); font-size: 0.72rem; color: var(--accent-bright); text-align: center;
  }
  .topo-arrow-line { width: 54px; height: 2px; background: linear-gradient(90deg, var(--accent), var(--cyan)); border-radius: 2px; }

  .pillars { display: grid; grid-template-columns: repeat(3, 1fr); gap: 18px; }
  .pillar {
    background: var(--bg-surface); border: 1px solid var(--border);
    border-radius: var(--radius-lg); padding: 24px; box-shadow: var(--shadow-card);
  }
  .pillar-icon {
    width: 38px; height: 38px; border-radius: 9px; margin-bottom: 14px;
    background: var(--accent-soft); border: 1px solid var(--accent-border);
    display: inline-flex; align-items: center; justify-content: center; color: var(--accent-bright);
  }
  .pillar h3 { font-size: 1.02rem; font-weight: 700; margin-bottom: 8px; color: var(--text-strong); }
  .pillar p { font-size: 0.88rem; color: var(--text-secondary); margin: 0; }

  /* Two-Column Spotlight */
  .split-2 { display: grid; grid-template-columns: 1fr 1fr; gap: 22px; align-items: start; }
  .ep-list { display: flex; flex-direction: column; gap: 10px; }
  .ep-row {
    display: flex; align-items: flex-start; gap: 12px; padding: 14px 16px;
    background: var(--bg-surface); border: 1px solid var(--border); border-radius: var(--radius-md);
    box-shadow: var(--shadow-card);
  }
  .ep-method {
    font-family: var(--font-mono); font-size: 0.7rem; font-weight: 700;
    padding: 3px 7px; border-radius: 5px; flex-shrink: 0; margin-top: 2px;
  }
  .ep-method.get { background: var(--success-soft); color: var(--success-fg); border: 1px solid rgba(5, 150, 105, 0.3); }
  .ep-method.post { background: var(--cyan-soft); color: var(--cyan-fg); border: 1px solid rgba(2, 132, 199, 0.3); }
  .ep-row code { font-size: 0.85rem; color: var(--text-strong); background: none; border: none; padding: 0; font-weight: 600; }
  .ep-row p { font-size: 0.83rem; color: var(--text-secondary); margin: 3px 0 0; }

  /* Interactive Curl Tabs */
  .home-tabs {
    background: var(--bg-surface); border: 1px solid var(--border);
    border-radius: var(--radius-lg); overflow: hidden; box-shadow: var(--shadow-card);
  }
  .home-tab-bar {
    display: flex; gap: 2px; overflow-x: auto; padding: 6px 8px 0;
    border-bottom: 1px solid var(--border); background: var(--bg-subtle);
  }
  .home-tab {
    padding: 8px 13px; background: transparent; border: none; border-bottom: 2px solid transparent;
    color: var(--text-muted); font-size: 0.82rem; font-weight: 600; font-family: inherit;
    cursor: pointer; white-space: nowrap;
  }
  .home-tab:hover { color: var(--text-strong); }
  .home-tab.active { color: var(--text-strong); border-bottom-color: var(--accent-bright); }
  .home-panel { display: none; position: relative; background: var(--bg-code); }
  .home-panel.active { display: block; }
  .home-panel pre { margin: 0; padding: 18px 66px 18px 18px; overflow-x: auto; font-size: 0.82rem; line-height: 1.65; color: var(--code-fg); }
  .home-panel.prompt-panel pre { white-space: pre-wrap; overflow-wrap: anywhere; overflow-x: hidden; font-size: 0.86rem; line-height: 1.68; }
  .home-panel .copy-btn { position: absolute; top: 12px; right: 12px; }

  /* Agent Bootstrap Spotlight */
  .agent-banner {
    background: var(--bg-surface); border: 1px solid var(--accent-border);
    border-radius: var(--radius-xl); padding: 28px; box-shadow: var(--shadow-card);
    position: relative; overflow: hidden;
  }
  .agent-banner::before {
    content: ''; position: absolute; top: 0; left: 0; right: 0; height: 3px;
    background: linear-gradient(90deg, var(--accent), var(--cyan));
  }
  .agent-top {
    display: flex; justify-content: space-between; align-items: flex-end; gap: 20px;
    flex-wrap: wrap; margin-bottom: 22px;
  }
  .agent-top h2 {
    font-size: 1.85rem; font-weight: 800; letter-spacing: -0.03em; line-height: 1.15;
    margin-bottom: 8px; color: var(--text-strong);
  }
  .agent-top p { color: var(--text-secondary); font-size: 0.96rem; max-width: 680px; margin: 0; }
  .agent-links { display: flex; gap: 10px; flex-wrap: wrap; align-items: center; }
  .agent-prompt-note {
    padding: 11px 18px; border-top: 1px solid rgba(148, 163, 184, 0.16);
    background: var(--bg-code-head); font-size: 0.8rem; color: var(--code-muted);
    display: flex; justify-content: space-between; align-items: center; gap: 10px; flex-wrap: wrap;
  }
  .agent-prompt-note code { color: #e0e7ff; background: rgba(99, 102, 241, 0.22); border-color: rgba(129, 140, 248, 0.35); }
  .agent-pillars {
    display: grid; grid-template-columns: repeat(3, 1fr); gap: 14px; margin-top: 18px;
  }
  .agent-pillar {
    background: var(--bg-subtle); border: 1px solid var(--border);
    border-radius: var(--radius-md); padding: 15px 16px;
  }
  .agent-pillar strong { display: block; font-size: 0.88rem; color: var(--text-strong); margin-bottom: 4px; }
  .agent-pillar p { font-size: 0.82rem; color: var(--text-secondary); margin: 0; line-height: 1.55; }

  /* Desktop Showcase */
  .desktop-grid { display: grid; grid-template-columns: 1.12fr 0.88fr; gap: 36px; align-items: center; }
  .window-frame {
    background: var(--bg-surface); border: 1px solid var(--border-strong);
    border-radius: var(--radius-xl); overflow: hidden;
    box-shadow: var(--shadow-hero);
  }
  .window-bar {
    display: flex; align-items: center; justify-content: space-between;
    padding: 10px 14px; background: var(--bg-subtle); border-bottom: 1px solid var(--border);
    font-size: 0.76rem; color: var(--text-muted); font-family: var(--font-mono);
  }
  .window-frame img { display: block; width: 100%; height: auto; }
  .dl-cards { display: grid; grid-template-columns: repeat(3, 1fr); gap: 10px; margin: 20px 0; }
  .dl-card {
    display: flex; flex-direction: column; gap: 4px; padding: 16px;
    background: var(--bg-surface); border: 1px solid var(--border); border-radius: var(--radius-md);
    color: var(--text-primary); text-decoration: none;
    transition: border-color 0.15s, transform 0.15s, background 0.15s;
  }
  .dl-card:hover { border-color: var(--accent-border); background: var(--bg-elevated); transform: translateY(-2px); text-decoration: none; }
  .dl-card.recommended { border-color: var(--accent); background: var(--accent-soft); }
  .dl-card strong { font-size: 0.94rem; color: var(--text-strong); }
  .dl-card span { font-size: 0.78rem; color: var(--text-muted); }

  /* Bottom CTA */
  .cta-banner {
    background: linear-gradient(135deg, var(--accent-soft), var(--cyan-soft));
    border: 1px solid var(--accent-border); border-radius: var(--radius-xl);
    padding: 44px 36px; text-align: center;
  }
  .cta-banner h2 { font-size: 1.85rem; font-weight: 800; letter-spacing: -0.025em; margin-bottom: 10px; color: var(--text-strong); }
  .cta-banner p { color: var(--text-secondary); max-width: 560px; margin: 0 auto 24px; }
  .cta-actions { display: flex; gap: 12px; justify-content: center; flex-wrap: wrap; }

  @media (max-width: 960px) {
    .hero-grid, .desktop-grid, .split-2 { grid-template-columns: 1fr; }
    .topo-grid { grid-template-columns: 1fr; }
    .topo-arrow-line { width: 2px; height: 24px; }
    .pillars, .agent-pillars { grid-template-columns: 1fr; }
    .hero h1 { font-size: 2.35rem; }
  }
  @media (max-width: 600px) {
    .hero { padding: 48px 0; }
    .section { padding: 52px 0; }
    .dl-cards { grid-template-columns: 1fr; }
    .agent-banner { padding: 20px 16px; }
    .agent-top h2 { font-size: 1.5rem; }
    .home-panel.prompt-panel pre { padding: 48px 16px 16px 16px; font-size: 0.82rem; }
    .cta-banner { padding: 32px 20px; }
  }
";

const LANDING_JSONLD: &str = r#"<script type="application/ld+json">
  {
    "@context": "https://schema.org",
    "@type": "SoftwareApplication",
    "name": "ModelRelay",
    "description": "Route inference requests to your own GPU workers over WebSocket behind api.modelrelay.io. First-class Pi coding agent provider setup and curl-ready OpenAI, Responses, and Anthropic APIs.",
    "applicationCategory": "DeveloperApplication",
    "operatingSystem": "Windows, macOS, Linux",
    "url": "https://modelrelay.io",
    "publisher": {
      "@type": "Organization",
      "name": "ModelRelay",
      "url": "https://modelrelay.io"
    }
  }
  </script>"#;

const LANDING_JS: &str = r#"
(function() {
  document.querySelectorAll('[data-tab-scope]').forEach(function(scope) {
    var btns = scope.querySelectorAll('[data-tab-btn]');
    var panes = scope.querySelectorAll('[data-tab-pane]');
    btns.forEach(function(btn) {
      btn.addEventListener('click', function() {
        var key = btn.getAttribute('data-tab-btn');
        btns.forEach(function(b) { b.classList.toggle('active', b === btn); });
        panes.forEach(function(p) { p.classList.toggle('active', p.getAttribute('data-tab-pane') === key); });
      });
    });
  });

  var ua = navigator.userAgent.toLowerCase();
  var os = 'linux';
  var label = 'Download for Linux (.deb)';
  var href = '/download/desktop/linux';
  if (ua.indexOf('mac') !== -1) {
    os = 'macos';
    label = 'Download for macOS (Apple Silicon)';
    href = '/download/desktop/macos';
  } else if (ua.indexOf('win') !== -1) {
    os = 'windows';
    label = 'Download for Windows (.exe)';
    href = '/download/desktop/windows';
  }
  var primaryBtn = document.getElementById('dl-primary-btn');
  if (primaryBtn) { primaryBtn.textContent = label; primaryBtn.setAttribute('href', href); }
  var card = document.querySelector('.dl-card[data-os="' + os + '"]');
  if (card) card.classList.add('recommended');
})();
"#;

/// GET / — session-aware overhauled landing page.
#[allow(clippy::too_many_lines)]
pub async fn page(OptionalSession(session): OptionalSession) -> Response {
    let logged_in = match session {
        Some(s) => s.get::<String>("user_id").await.ok().flatten().is_some(),
        None => false,
    };

    let account_cta = if logged_in {
        r#"<h2>Your Relay Is Ready</h2>
        <p>Open your dashboard to copy your API key, inspect connected workers, or configure Pi Coding Agent with your pre-filled credentials.</p>
        <div class="cta-actions">
          <a href="/dashboard" class="btn btn-lg">Open Dashboard &rarr;</a>
          <a href="/integrate#pi" class="btn btn-lg btn-secondary">Pi &amp; API Setup</a>
        </div>"#
    } else {
        r#"<h2>Use Hosted Access or Self-Host Free</h2>
        <p>Existing hosted accounts can log in anytime to manage keys and workers. Or deploy the MIT-licensed relay server on your own VPS in two minutes.</p>
        <div class="cta-actions">
          <a href="/login" class="btn btn-lg">Log In to Account &rarr;</a>
          <a href="/docs/self-hosting" class="btn btn-lg btn-secondary">Self-Hosting Guide</a>
        </div>"#
    };

    let main_html = format!(
        r##"
  <section class="hero">
    <div class="container hero-grid">
      <div>
        <div class="eyebrow-pill"><span class="eyebrow-dot"></span> Open-source LLM relay &middot; Built for Pi Coding Agent</div>
        <h1>One endpoint for all your <span class="hl">GPU boxes</span>.</h1>
        <p class="hero-sub">Stop exposing ports or rewriting configs for every machine. Workers connect <em>out</em> over WebSocket to <code>api.modelrelay.io</code>, and Pi Coding Agent or <code>curl</code> routes straight to your live local models.</p>
        <div class="hero-cta">
          <a href="#agents" class="btn btn-lg">Bootstrap With Your Agent &rarr;</a>
          <a href="/integrate#pi" class="btn btn-lg btn-secondary">Pi &amp; API Setup</a>
          <a href="/downloads" class="btn btn-lg btn-secondary">Download Desktop</a>
        </div>
        <div class="hero-meta">
          <span>MIT Open Source</span>
          <span>&middot;</span>
          <span>No inbound ports</span>
          <span>&middot;</span>
          <span><a href="/AGENTS.md">AGENTS.md</a></span>
          <span>&middot;</span>
          <span><a href="/docs">Read Docs &rarr;</a></span>
        </div>
      </div>

      <div class="hero-terminal" data-tab-scope="hero">
        <div class="term-top">
          <div class="term-dots"><i></i><i></i><i></i></div>
          <div class="term-tabs">
            <button type="button" class="term-tab active" data-tab-btn="hero-pi">Pi Provider + auth.json</button>
            <button type="button" class="term-tab" data-tab-btn="hero-curl">curl api.modelrelay.io</button>
          </div>
        </div>
        <div class="term-pane active" data-tab-pane="hero-pi">
<pre><span class="tk-c">// 1. ~/.pi/agent/extensions/modelrelay.ts</span>
<span class="tk-k">pi.registerProvider</span>(<span class="tk-s">"modelrelay"</span>, {{
  name: <span class="tk-s">"ModelRelay"</span>,
  baseUrl: <span class="tk-s">"https://api.modelrelay.io/v1"</span>,
  api: <span class="tk-s">"openai-completions"</span>,
  <span class="tk-k">async refreshModels</span>({{ credential, signal }}) {{
    <span class="tk-c">// Live /v1/models + thinkingLevelMap,</span>
    <span class="tk-c">// contextWindow, maxTokens &amp; compat</span>
  }},
}});

<span class="tk-c">// 2. ~/.pi/agent/auth.json  (or /login modelrelay)</span>
{{
  <span class="tk-s">"modelrelay"</span>: {{ <span class="tk-s">"type"</span>: <span class="tk-s">"api_key"</span>, <span class="tk-s">"key"</span>: <span class="tk-g">"mr_live_..."</span> }}
}}</pre>
        </div>
        <div class="term-pane" data-tab-pane="hero-curl">
<pre><span class="tk-c"># List live models across your connected GPU workers</span>
curl https://api.modelrelay.io/v1/models \
  -H <span class="tk-s">"Authorization: Bearer $MODELRELAY_API_KEY"</span>

<span class="tk-c"># Stream chat completions directly from your home rig</span>
curl -N https://api.modelrelay.io/v1/chat/completions \
  -H <span class="tk-s">"Content-Type: application/json"</span> \
  -H <span class="tk-s">"Authorization: Bearer $MODELRELAY_API_KEY"</span> \
  -d <span class="tk-g">'{{"model":"qwen3.5-27b","stream":true,"messages":[{{"role":"user","content":"Hi!"}}]}}'</span></pre>
        </div>
        <div class="term-foot">
          <span>Works with llama.cpp, Ollama, vLLM &amp; LM Studio</span>
          <a href="/integrate">Interactive setup &amp; live tester &rarr;</a>
        </div>
      </div>
    </div>
  </section>

  <!-- ═══ Agent-First Bootstrap (/AGENTS.md) ═══ -->
  <section class="section section-alt" id="agents">
    <div class="container">
      <div class="agent-banner">
        <div class="agent-top">
          <div>
            <div class="eyebrow">Agent-First Setup &middot; modelrelay.io/AGENTS.md</div>
            <h2>Already using Claude Code, Codex, or Pi? Let it bootstrap your self-hosted stack.</h2>
            <p>Use your existing commercial provider once to build your own local alternative. Paste a prompt below and your coding agent will read <a href="/AGENTS.md"><code>https://modelrelay.io/AGENTS.md</code></a>, inspect your running local LLM, install <code>modelrelay-server</code> and <code>modelrelay-worker</code>, and wire up your clients &mdash; without overwriting your current API keys.</p>
          </div>
          <div class="agent-links">
            <a href="/AGENTS.md" class="btn btn-sm">Read AGENTS.md &rarr;</a>
            <button type="button" class="btn btn-sm btn-secondary" data-copy-target="agents-md-url">Copy AGENTS.md URL</button>
            <span id="agents-md-url" style="display:none;">https://modelrelay.io/AGENTS.md</span>
          </div>
        </div>

        <div class="home-tabs" data-tab-scope="agent-bootstrap">
          <div class="home-tab-bar">
            <button type="button" class="home-tab active" data-tab-btn="ab-full">Full Self-Hosted Stack</button>
            <button type="button" class="home-tab" data-tab-btn="ab-worker">Connect This GPU Server</button>
            <button type="button" class="home-tab" data-tab-btn="ab-client">Configure Pi / Codex / Claude</button>
            <button type="button" class="home-tab" data-tab-btn="ab-cli">CLI One-Liners</button>
          </div>

          <div class="home-panel prompt-panel active" data-tab-pane="ab-full">
            <button type="button" class="copy-btn" data-copy-target="prompt-full">Copy</button>
<pre><code id="prompt-full">Read https://modelrelay.io/AGENTS.md and help me bootstrap a complete self-hosted ModelRelay setup alongside my current provider (do not overwrite my existing commercial API keys). Ask me where to run modelrelay-server and my GPU server's local LLM URL (or detect it on this machine), then deploy the relay with TLS, install modelrelay-worker as a persistent service with models=["*"], configure Pi (~/.pi/agent/extensions/modelrelay.ts + auth.json) and/or my installed coding agents with accurate thinking levels and context limits, and verify streaming chat completions end-to-end.</code></pre>
            <div class="agent-prompt-note">
              <span>Paste into <strong>Claude Code</strong>, <strong>Codex</strong>, or <strong>Pi</strong> &mdash; ideal when starting from scratch.</span>
              <span>Reads <code>https://modelrelay.io/AGENTS.md</code></span>
            </div>
          </div>

          <div class="home-panel prompt-panel" data-tab-pane="ab-worker">
            <button type="button" class="copy-btn" data-copy-target="prompt-worker">Copy</button>
<pre><code id="prompt-worker">Read https://modelrelay.io/AGENTS.md and connect this GPU machine's local LLM server to ModelRelay. Detect my running local backend (llama-server, Ollama, vLLM, LM Studio, or SGLang) and its context window, install modelrelay-worker, ask me for my relay URL and worker secret/API key, configure it as a persistent background service (systemd, launchd, or Docker) with models=["*"], and verify GET /v1/models and a streaming chat completion through the relay.</code></pre>
            <div class="agent-prompt-note">
              <span>Paste into an agent running on your <strong>GPU box</strong> &mdash; works with hosted or self-hosted relays.</span>
              <span>Sets up persistent worker + live model discovery</span>
            </div>
          </div>

          <div class="home-panel prompt-panel" data-tab-pane="ab-client">
            <button type="button" class="copy-btn" data-copy-target="prompt-client">Copy</button>
<pre><code id="prompt-client">Read https://modelrelay.io/AGENTS.md and configure my installed coding agents (Pi, Codex CLI, and/or Claude Code) on this machine to talk to my ModelRelay endpoint without overwriting my existing commercial providers. Ask me for my ModelRelay URL and API key, configure Pi as a dynamic provider (~/.pi/agent/extensions/modelrelay.ts + merged ~/.pi/agent/auth.json with proper thinking levels and context limits), add a non-destructive Codex provider or claude-local alias if installed, and verify live models respond.</code></pre>
            <div class="agent-prompt-note">
              <span>Paste into an agent on your <strong>laptop / workstation</strong> &mdash; keeps your commercial accounts untouched.</span>
              <span>Merges configs safely with <code>chmod 600</code></span>
            </div>
          </div>

          <div class="home-panel" data-tab-pane="ab-cli">
            <button type="button" class="copy-btn" data-copy-target="prompt-cli">Copy</button>
<pre><code id="prompt-cli"># Claude Code (using your existing Anthropic setup)
claude "Read https://modelrelay.io/AGENTS.md and bootstrap my self-hosted ModelRelay server, GPU worker, and client config without overwriting my existing commercial provider."

# OpenAI Codex CLI (using your existing OpenAI setup)
codex "Read https://modelrelay.io/AGENTS.md and bootstrap my self-hosted ModelRelay server, GPU worker, and client config without overwriting my existing commercial provider."

# Pi Coding Agent (using any existing provider)
pi "Read https://modelrelay.io/AGENTS.md and bootstrap my self-hosted ModelRelay server, GPU worker, and client config without overwriting my existing commercial provider."</code></pre>
            <div class="agent-prompt-note">
              <span>Run any line directly from your shell to kick off an interactive agent session.</span>
              <span>Works with Claude Code, Codex &amp; Pi</span>
            </div>
          </div>
        </div>

        <div class="agent-pillars">
          <div class="agent-pillar">
            <strong>1. Server &amp; GPU Worker Automation</strong>
            <p>Exact binary URLs, Docker images, <code>modelrelay-server</code> + Caddy/nginx TLS configs, and <code>systemd</code>/<code>launchd</code> worker units.</p>
          </div>
          <div class="agent-pillar">
            <strong>2. Safe Multi-Provider Client Setup</strong>
            <p>Wires Pi (<code>modelrelay.ts</code> + <code>auth.json</code>), Codex <code>config.toml</code>, and a <code>claude-local</code> alias without touching existing keys.</p>
          </div>
          <div class="agent-pillar">
            <strong>3. Live End-to-End Verification</strong>
            <p>Guides your agent to test <code>/health</code>, <code>/v1/models</code>, thinking effort, and unbuffered SSE streaming before finishing.</p>
          </div>
        </div>
      </div>
    </div>
  </section>

  <!-- ═══ Architecture & Engineering Pillars ═══ -->
  <section class="section">
    <div class="container">
      <div class="section-head">
        <div class="eyebrow">How It Works</div>
        <h2>Workers connect out. Requests route in.</h2>
        <p>Run the ModelRelay Desktop app or worker binary next to your local model server. Your machines stay safely behind NAT while every client talks to one stable TLS endpoint.</p>
      </div>

      <div class="topo-card">
        <div class="topo-grid">
          <div class="topo-node">
            <div class="topo-tag">Clients</div>
            <h3>Pi &amp; curl</h3>
            <p>One endpoint, one API key in <code>auth.json</code>. Zero per-machine URLs or port juggling.</p>
          </div>
          <div class="topo-arrow">
            <span>HTTPS / SSE</span>
            <span class="topo-arrow-line"></span>
            <span>/v1/*</span>
          </div>
          <div class="topo-node relay">
            <div class="topo-tag">Central Relay</div>
            <h3>api.modelrelay.io</h3>
            <p>Load-aware routing, per-provider FIFO queue, unbuffered SSE pass-through, and automatic requeue.</p>
          </div>
          <div class="topo-arrow">
            <span>Outbound WSS</span>
            <span class="topo-arrow-line"></span>
            <span>No open ports</span>
          </div>
          <div class="topo-node">
            <div class="topo-tag">Your GPU Boxes</div>
            <h3>Desktop App / Workers</h3>
            <p>Talks to local <code>llama-server</code>, Ollama, vLLM, or LM Studio and refreshes models live.</p>
          </div>
        </div>
      </div>

      <div class="pillars">
        <div class="pillar">
          <div class="pillar-icon">
            <svg width="19" height="19" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><path d="M12 22s8-4 8-10V5l-8-3-8 3v7c0 6 8 10 8 10z"/><path d="M9 12l2 2 4-4"/></svg>
          </div>
          <h3>Zero Inbound Firewall Holes</h3>
          <p>Workers initiate outbound WebSocket connections with exponential backoff, heartbeats, and graceful drain &mdash; ideal for home networks, WSL, or cloud GPUs behind NAT.</p>
        </div>
        <div class="pillar">
          <div class="pillar-icon">
            <svg width="19" height="19" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><polyline points="22 12 18 12 15 21 9 3 6 12 2 12"/></svg>
          </div>
          <h3>Streaming &amp; Instant Cancellation</h3>
          <p>SSE tokens stream straight through without buffering. Hit Escape in Pi or <code>Ctrl+C</code> in <code>curl</code> and cancellation propagates immediately to stop GPU generation.</p>
        </div>
        <div class="pillar">
          <div class="pillar-icon">
            <svg width="19" height="19" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><circle cx="12" cy="12" r="3"/><path d="M19.4 15a1.65 1.65 0 0 0 .33 1.82l.06.06a2 2 0 1 1-2.83 2.83l-.06-.06a1.65 1.65 0 0 0-1.82-.33 1.65 1.65 0 0 0-1 1.51V21a2 2 0 1 1-4 0v-.09a1.65 1.65 0 0 0-1-1.51 1.65 1.65 0 0 0-1.82.33l-.06.06a2 2 0 1 1-2.83-2.83l.06-.06a1.65 1.65 0 0 0 .33-1.82 1.65 1.65 0 0 0-1.51-1H3a2 2 0 1 1 0-4h.09a1.65 1.65 0 0 0 1.51-1 1.65 1.65 0 0 0-.33-1.82l-.06-.06a2 2 0 1 1 2.83-2.83l.06.06a1.65 1.65 0 0 0 1.82.33H9a1.65 1.65 0 0 0 1-1.51V3a2 2 0 1 1 4 0v.09a1.65 1.65 0 0 0 1 1.51 1.65 1.65 0 0 0 1.82-.33l.06-.06a2 2 0 1 1 2.83 2.83l-.06.06a1.65 1.65 0 0 0-.33 1.82V9a1.65 1.65 0 0 0 1.51 1H21a2 2 0 1 1 0 4h-.09a1.65 1.65 0 0 0-1.51 1z"/></svg>
          </div>
          <h3>Live Model Discovery</h3>
          <p>Workers poll your backend&rsquo;s <code>/v1/models</code> and publish changes dynamically. Swap a GGUF on your workstation and Pi picks it up automatically.</p>
        </div>
      </div>
    </div>
  </section>

  <!-- ═══ Spotlight 1: api.modelrelay.io + curl ═══ -->
  <section class="section section-alt" id="api">
    <div class="container">
      <div class="section-head">
        <div class="eyebrow">1 &middot; The Endpoint</div>
        <h2><code>https://api.modelrelay.io</code> &mdash; verify with <code>curl</code></h2>
        <p>One clean HTTPS boundary speaking OpenAI Chat Completions, OpenAI Responses, Anthropic Messages, and <code>/v1/models</code>.</p>
      </div>

      <div class="split-2">
        <div class="ep-list">
          <div class="ep-row">
            <span class="ep-method get">GET</span>
            <div>
              <code>https://api.modelrelay.io/v1/models</code>
              <p>Lists every live model currently advertised by your authenticated workers.</p>
            </div>
          </div>
          <div class="ep-row">
            <span class="ep-method post">POST</span>
            <div>
              <code>https://api.modelrelay.io/v1/chat/completions</code>
              <p>OpenAI Chat Completions with unbuffered SSE streaming and cancellation.</p>
            </div>
          </div>
          <div class="ep-row">
            <span class="ep-method post">POST</span>
            <div>
              <code>https://api.modelrelay.io/v1/responses</code>
              <p>OpenAI Responses API routed to your workers with identical auth.</p>
            </div>
          </div>
          <div class="ep-row">
            <span class="ep-method post">POST</span>
            <div>
              <code>https://api.modelrelay.io/v1/messages</code>
              <p>Anthropic Messages API using <code>x-api-key</code> or <code>Authorization: Bearer</code>.</p>
            </div>
          </div>
          <div style="margin-top:6px;">
            <a href="/docs/api" style="font-size:0.9rem;font-weight:600;">Full API reference &amp; routing headers &rarr;</a>
          </div>
        </div>

        <div class="home-tabs" data-tab-scope="curl">
          <div class="home-tab-bar">
            <button type="button" class="home-tab active" data-tab-btn="c1">List Models</button>
            <button type="button" class="home-tab" data-tab-btn="c2">Chat</button>
            <button type="button" class="home-tab" data-tab-btn="c3">Stream</button>
            <button type="button" class="home-tab" data-tab-btn="c4">Responses</button>
            <button type="button" class="home-tab" data-tab-btn="c5">Anthropic</button>
          </div>
          <div class="home-panel active" data-tab-pane="c1">
            <button type="button" class="copy-btn" data-copy-target="home-c1">Copy</button>
<pre><code id="home-c1">curl https://api.modelrelay.io/v1/models \
  -H "Authorization: Bearer $MODELRELAY_API_KEY"</code></pre>
          </div>
          <div class="home-panel" data-tab-pane="c2">
            <button type="button" class="copy-btn" data-copy-target="home-c2">Copy</button>
<pre><code id="home-c2">curl https://api.modelrelay.io/v1/chat/completions \
  -H "Content-Type: application/json" \
  -H "Authorization: Bearer $MODELRELAY_API_KEY" \
  -d '{{
    "model": "qwen3.5-27b",
    "messages": [{{"role": "user", "content": "Hello!"}}]
  }}'</code></pre>
          </div>
          <div class="home-panel" data-tab-pane="c3">
            <button type="button" class="copy-btn" data-copy-target="home-c3">Copy</button>
<pre><code id="home-c3">curl -N https://api.modelrelay.io/v1/chat/completions \
  -H "Content-Type: application/json" \
  -H "Authorization: Bearer $MODELRELAY_API_KEY" \
  -d '{{
    "model": "qwen3.5-27b",
    "stream": true,
    "messages": [{{"role": "user", "content": "Stream a haiku."}}]
  }}'</code></pre>
          </div>
          <div class="home-panel" data-tab-pane="c4">
            <button type="button" class="copy-btn" data-copy-target="home-c4">Copy</button>
<pre><code id="home-c4">curl https://api.modelrelay.io/v1/responses \
  -H "Content-Type: application/json" \
  -H "Authorization: Bearer $MODELRELAY_API_KEY" \
  -d '{{
    "model": "qwen3.5-27b",
    "input": "Summarize the ModelRelay WebSocket protocol."
  }}'</code></pre>
          </div>
          <div class="home-panel" data-tab-pane="c5">
            <button type="button" class="copy-btn" data-copy-target="home-c5">Copy</button>
<pre><code id="home-c5">curl https://api.modelrelay.io/v1/messages \
  -H "Content-Type: application/json" \
  -H "x-api-key: $MODELRELAY_API_KEY" \
  -H "anthropic-version: 2023-06-01" \
  -d '{{
    "model": "qwen3.5-27b",
    "max_tokens": 256,
    "messages": [{{"role": "user", "content": "Hello!"}}]
  }}'</code></pre>
          </div>
        </div>
      </div>
    </div>
  </section>

  <!-- ═══ Spotlight 2: Pi Coding Agent ═══ -->
  <section class="section" id="pi-spotlight">
    <div class="container">
      <div class="section-head">
        <div class="eyebrow">2 &middot; First-Class Pi Integration</div>
        <h2>Add ModelRelay as a Pi provider, not a static model list</h2>
        <p>Register <code>modelrelay</code> once in <code>~/.pi/agent/extensions/modelrelay.ts</code> and keep your API key cleanly in <code>~/.pi/agent/auth.json</code>. Pi discovers every live model on your GPU fleet automatically.</p>
      </div>

      <div class="split-2">
        <div class="card" style="margin-bottom:0;">
          <div style="display:flex;justify-content:space-between;align-items:center;margin-bottom:10px;">
            <h2 style="margin:0;">1. Provider Extension</h2>
            <span class="badge badge-accent">Thinking &amp; Context Aware</span>
          </div>
          <p style="font-size:0.88rem;margin-bottom:12px;">Drop <code>modelrelay.ts</code> into <code>~/.pi/agent/extensions/</code>. Discovers live <code>/v1/models</code> and conveys <code>thinkingLevelMap</code>, <code>contextWindow</code>, <code>maxTokens</code>, and local-backend <code>compat</code> straight to Pi.</p>
          <div class="code-shell" style="margin:0;">
            <div class="code-shell-header"><span>~/.pi/agent/extensions/modelrelay.ts</span><a href="/integrate#pi" style="font-size:0.76rem;">Configure &amp; copy &rarr;</a></div>
            <pre><code>pi.registerProvider("modelrelay", {{
  name: "ModelRelay",
  baseUrl: "https://api.modelrelay.io/v1",
  api: "openai-completions",
  async refreshModels({{ credential, signal }}) {{
    // Returns live models with reasoning: true,
    // thinkingLevelMap (off:"none", low/medium/high),
    // contextWindow, maxTokens, and local compat
  }},
}});</code></pre>
          </div>
        </div>

        <div class="card" style="margin-bottom:0;">
          <div style="display:flex;justify-content:space-between;align-items:center;margin-bottom:10px;">
            <h2 style="margin:0;">2. Token in <code>auth.json</code></h2>
            <span class="badge badge-active">Or /login modelrelay</span>
          </div>
          <p style="font-size:0.88rem;margin-bottom:12px;">Keep secrets out of <code>models.json</code>. Put your key in <code>~/.pi/agent/auth.json</code> (or run <code>/login modelrelay</code> inside Pi) and open <code>/model</code>.</p>
          <div class="code-shell" style="margin-bottom:14px;">
            <div class="code-shell-header"><span>~/.pi/agent/auth.json</span><button type="button" class="copy-btn" data-copy-target="home-auth-json">Copy</button></div>
            <pre><code id="home-auth-json">{{
  "modelrelay": {{
    "type": "api_key",
    "key": "mr_live_your_api_key"
  }}
}}</code></pre>
          </div>
          <div style="display:flex;gap:10px;flex-wrap:wrap;">
            <a href="/integrate#pi" class="btn btn-sm">Complete Pi Setup &rarr;</a>
            <a href="/docs/pi" class="btn btn-sm btn-secondary">Read Pi Docs</a>
          </div>
        </div>
      </div>
    </div>
  </section>

  <!-- ═══ Desktop App & Downloads (#download) ═══ -->
  <section class="section section-alt" id="download">
    <div class="container desktop-grid">
      <div class="window-frame">
        <div class="window-bar">
          <span>ModelRelay Desktop</span>
          <span style="color:var(--success);">&#9679; Connected &middot; api.modelrelay.io</span>
        </div>
        <img src="/assets/desktop-dashboard.png" alt="ModelRelay Desktop dashboard showing connected status, active relay server, and live models" width="1200" height="900" loading="lazy">
      </div>

      <div>
        <div class="eyebrow">Desktop App &amp; CLI</div>
        <h2 style="font-size:2rem;font-weight:800;letter-spacing:-0.03em;line-height:1.14;color:var(--text-strong);margin-bottom:12px;">One-click tray worker for every machine</h2>
        <p style="color:var(--text-secondary);margin-bottom:18px;">Run a native system-tray worker on macOS, Linux, or Windows &mdash; live connection status, automatic model refresh, Launch at Login, and signed in-place updates. Prefer headless servers? Grab the standalone CLI binary or Docker image.</p>

        <div style="margin-bottom:16px;">
          <a id="dl-primary-btn" href="/download/desktop/macos" class="btn btn-lg">Download Desktop App &rarr;</a>
        </div>

        <div class="dl-cards">
          <a href="/download/desktop/macos" class="dl-card" data-os="macos">
            <strong>macOS</strong>
            <span>Apple Silicon .dmg</span>
          </a>
          <a href="/download/desktop/linux" class="dl-card" data-os="linux">
            <strong>Linux</strong>
            <span>.deb &middot; .AppImage</span>
          </a>
          <a href="/download/desktop/windows" class="dl-card" data-os="windows">
            <strong>Windows</strong>
            <span>.exe &middot; .msi</span>
          </a>
        </div>

        <p style="font-size:0.86rem;color:var(--text-muted);">
          Need Intel macOS, AppImage, MSI, CLI binaries, or Docker? <a href="/downloads" style="font-weight:600;">Browse all downloads &rarr;</a>
        </p>
      </div>
    </div>
  </section>

  <!-- ═══ Bottom CTA ═══ -->
  <section class="section" style="border-bottom:none;">
    <div class="container">
      <div class="cta-banner">
        {account_cta}
      </div>
    </div>
  </section>
"##
    );

    let extra_head = format!("{LANDING_JSONLD}<style>{LANDING_CSS}</style>");
    let extra_body_end = format!("<script>{LANDING_JS}</script>");

    Html(modelrelay_web::templates::page_shell_full(
        "ModelRelay — One Endpoint for All Your GPU Boxes",
        "/",
        &main_html,
        logged_in,
        &extra_head,
        &extra_body_end,
    ))
    .into_response()
}
