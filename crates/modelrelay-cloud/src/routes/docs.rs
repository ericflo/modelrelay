use std::fmt::Write;

use axum::extract::Path;
use axum::http::StatusCode;
use axum::response::{Html, IntoResponse, Response};

use super::OptionalSession;

const DOCS_CSS: &str = r"
  .docs-layout {
    display: grid; grid-template-columns: 240px minmax(0, 1fr); gap: 44px;
    padding: 40px 0 80px; align-items: start;
  }
  .docs-sidebar {
    position: sticky; top: 88px;
    background: var(--bg-surface); border: 1px solid var(--border);
    border-radius: var(--radius-lg); padding: 18px 14px;
    box-shadow: var(--shadow-card);
  }
  .docs-side-label {
    font-size: 0.7rem; font-weight: 700; text-transform: uppercase;
    letter-spacing: 0.08em; color: var(--text-muted); padding: 4px 10px 8px;
  }
  .docs-nav { display: flex; flex-direction: column; gap: 3px; }
  .docs-nav a {
    display: flex; align-items: center; justify-content: space-between;
    padding: 8px 10px; border-radius: 8px;
    font-size: 0.88rem; font-weight: 500; color: var(--text-secondary);
    text-decoration: none; transition: background 0.15s, color 0.15s;
  }
  .docs-nav a:hover { background: var(--bg-hover); color: var(--text-strong); text-decoration: none; }
  .docs-nav a.active { background: var(--accent-soft); color: var(--accent-fg); font-weight: 600; }
  .docs-side-ext {
    margin-top: 14px; padding-top: 12px; border-top: 1px solid var(--border);
    display: flex; flex-direction: column; gap: 4px;
  }

  .docs-article { min-width: 0; }
  .docs-eyebrow {
    font-size: 0.74rem; font-weight: 700; text-transform: uppercase;
    letter-spacing: 0.09em; color: var(--accent-bright); margin-bottom: 8px;
  }
  .docs-article h1 {
    font-size: 2.2rem; font-weight: 800; letter-spacing: -0.03em; line-height: 1.15;
    color: var(--text-strong); margin-bottom: 12px;
  }
  .docs-lead { font-size: 1.04rem; color: var(--text-secondary); margin-bottom: 28px; max-width: 720px; }
  .docs-article h2 {
    font-size: 1.35rem; font-weight: 700; letter-spacing: -0.02em; color: var(--text-strong);
    margin: 38px 0 12px; padding-top: 24px; border-top: 1px solid var(--border);
  }
  .docs-article h2:first-of-type { margin-top: 10px; }
  .docs-article h3 { font-size: 1.04rem; font-weight: 700; color: var(--text-strong); margin: 22px 0 8px; }
  .docs-article p { color: var(--text-secondary); margin-bottom: 14px; line-height: 1.72; }
  .docs-article ul, .docs-article ol { color: var(--text-secondary); margin: 0 0 16px 20px; line-height: 1.75; }
  .docs-article li { margin-bottom: 6px; }
  .docs-article strong { color: var(--text-strong); }

  .doc-table-wrap { overflow-x: auto; margin: 14px 0 22px; border: 1px solid var(--border); border-radius: var(--radius-md); }
  .doc-table { width: 100%; border-collapse: collapse; font-size: 0.87rem; background: var(--bg-surface); }
  .doc-table th {
    text-align: left; padding: 10px 14px; font-size: 0.74rem; font-weight: 700;
    text-transform: uppercase; letter-spacing: 0.06em; color: var(--text-muted);
    border-bottom: 1px solid var(--border); background: var(--bg-subtle);
  }
  .doc-table td { padding: 10px 14px; border-bottom: 1px solid var(--border); color: var(--text-secondary); vertical-align: top; }
  .doc-table tr:last-child td { border-bottom: none; }

  .callout {
    padding: 15px 18px; border-radius: var(--radius-md); margin: 18px 0;
    background: var(--accent-soft); border: 1px solid var(--accent-border);
    color: var(--callout-fg); font-size: 0.9rem; line-height: 1.65;
  }
  .callout strong { color: var(--text-strong); }
  .callout-warn {
    background: var(--warning-soft); border-color: rgba(217, 119, 6, 0.35); color: var(--warning-fg);
  }

  .doc-cards { display: grid; grid-template-columns: repeat(2, 1fr); gap: 14px; margin: 20px 0 28px; }
  .doc-card {
    display: block; padding: 18px 20px; background: var(--bg-surface);
    border: 1px solid var(--border); border-radius: var(--radius-md);
    text-decoration: none; transition: border-color 0.15s, transform 0.15s;
    box-shadow: var(--shadow-card);
  }
  .doc-card:hover { border-color: var(--accent-border); transform: translateY(-2px); text-decoration: none; }
  .doc-card h3 { margin: 0 0 6px; font-size: 1rem; color: var(--text-strong); }
  .doc-card p { margin: 0; font-size: 0.85rem; color: var(--text-secondary); }

  .docs-pager {
    display: flex; justify-content: space-between; gap: 16px; flex-wrap: wrap;
    margin-top: 44px; padding-top: 24px; border-top: 1px solid var(--border);
  }
  .pager-link {
    display: flex; flex-direction: column; gap: 3px; padding: 12px 16px;
    background: var(--bg-surface); border: 1px solid var(--border); border-radius: var(--radius-md);
    min-width: 190px; text-decoration: none; transition: border-color 0.15s;
  }
  .pager-link:hover { border-color: var(--accent-border); text-decoration: none; }
  .pager-dir { font-size: 0.72rem; text-transform: uppercase; letter-spacing: 0.08em; color: var(--text-muted); }
  .pager-title { font-size: 0.92rem; font-weight: 600; color: var(--text-strong); }

  @media (max-width: 860px) {
    .docs-layout { grid-template-columns: 1fr; gap: 24px; padding: 28px 0 56px; }
    .docs-sidebar { position: static; }
    .docs-nav { flex-direction: row; flex-wrap: wrap; }
    .doc-cards { grid-template-columns: 1fr; }
    .docs-article h1 { font-size: 1.75rem; }
  }
";

struct DocPage {
    slug: &'static str,
    path: &'static str,
    nav_label: &'static str,
    title: &'static str,
    body: &'static str,
}

const PAGES: &[DocPage] = &[
    DocPage {
        slug: "",
        path: "/docs",
        nav_label: "Overview & Quickstart",
        title: "Documentation",
        body: DOC_OVERVIEW,
    },
    DocPage {
        slug: "api",
        path: "/docs/api",
        nav_label: "api.modelrelay.io",
        title: "api.modelrelay.io Reference",
        body: DOC_API,
    },
    DocPage {
        slug: "pi",
        path: "/docs/pi",
        nav_label: "Pi Coding Agent",
        title: "Pi Coding Agent Setup",
        body: DOC_PI,
    },
    DocPage {
        slug: "workers",
        path: "/docs/workers",
        nav_label: "Desktop & Workers",
        title: "Desktop App & Workers",
        body: DOC_WORKERS,
    },
    DocPage {
        slug: "self-hosting",
        path: "/docs/self-hosting",
        nav_label: "Self-Hosting",
        title: "Self-Hosting ModelRelay",
        body: DOC_SELF_HOST,
    },
    DocPage {
        slug: "architecture",
        path: "/docs/architecture",
        nav_label: "Architecture & Protocol",
        title: "Architecture & Protocol",
        body: DOC_ARCHITECTURE,
    },
];

async fn is_logged_in(session: Option<tower_sessions::Session>) -> bool {
    match session {
        Some(s) => s.get::<String>("user_id").await.ok().flatten().is_some(),
        None => false,
    }
}

fn render_doc(idx: usize, logged_in: bool) -> Html<String> {
    let page = &PAGES[idx];
    let mut sidebar_links = String::new();
    for (i, item) in PAGES.iter().enumerate() {
        let cls = if i == idx { " class=\"active\"" } else { "" };
        let _ = write!(
            sidebar_links,
            "<a href=\"{}\"{cls}>{}</a>",
            item.path, item.nav_label
        );
    }

    let prev_html = if idx > 0 {
        let prev = &PAGES[idx - 1];
        format!(
            r#"<a href="{}" class="pager-link"><span class="pager-dir">&larr; Previous</span><span class="pager-title">{}</span></a>"#,
            prev.path, prev.nav_label
        )
    } else {
        "<div></div>".to_string()
    };

    let next_html = if idx + 1 < PAGES.len() {
        let next = &PAGES[idx + 1];
        format!(
            r#"<a href="{}" class="pager-link" style="text-align:right;"><span class="pager-dir">Next &rarr;</span><span class="pager-title">{}</span></a>"#,
            next.path, next.nav_label
        )
    } else {
        "<div></div>".to_string()
    };

    let main_html = format!(
        r#"<div class="container">
  <div class="docs-layout">
    <aside class="docs-sidebar" aria-label="Documentation navigation">
      <div class="docs-side-label">Documentation</div>
      <nav class="docs-nav">
        {sidebar_links}
      </nav>
      <div class="docs-side-ext">
        <a href="/AGENTS.md" class="nav-item" style="font-size:0.84rem;padding:7px 10px;">AGENTS.md (For Coding Agents) &rarr;</a>
        <a href="/integrate" class="nav-item" style="font-size:0.84rem;padding:7px 10px;">Interactive Pi &amp; API Tool &rarr;</a>
        <a href="/downloads" class="nav-item" style="font-size:0.84rem;padding:7px 10px;">Downloads &rarr;</a>
        <a href="https://github.com/ericflo/modelrelay" target="_blank" rel="noopener" class="nav-item" style="font-size:0.84rem;padding:7px 10px;">GitHub Repository &nearr;</a>
      </div>
    </aside>

    <article class="docs-article">
      {body}
      <div class="docs-pager">
        {prev_html}
        {next_html}
      </div>
    </article>
  </div>
</div>"#,
        body = page.body
    );

    let extra_head = format!("<style>{DOCS_CSS}</style>");
    Html(modelrelay_web::templates::page_shell_full(
        page.title,
        page.path,
        &main_html,
        logged_in,
        &extra_head,
        "",
    ))
}

/// GET /docs — Documentation home / quickstart.
pub async fn index(OptionalSession(session): OptionalSession) -> Response {
    let logged_in = is_logged_in(session).await;
    render_doc(0, logged_in).into_response()
}

/// GET /docs/{slug} — Individual documentation page.
pub async fn page(OptionalSession(session): OptionalSession, Path(slug): Path<String>) -> Response {
    let logged_in = is_logged_in(session).await;
    if let Some(idx) = PAGES.iter().position(|p| p.slug == slug) {
        return render_doc(idx, logged_in).into_response();
    }

    let body = r#"<div style="text-align:center;padding:48px 16px;">
      <h1 style="font-size:3.5rem;font-weight:800;color:var(--accent-bright);margin-bottom:8px;">404</h1>
      <p style="color:var(--text-secondary);margin-bottom:24px;">That documentation page doesn&rsquo;t exist.</p>
      <a href="/docs" class="btn">Back to Documentation &rarr;</a>
    </div>"#;
    (
        StatusCode::NOT_FOUND,
        Html(modelrelay_web::templates::page_shell_custom(
            "404 — Doc Not Found",
            "/docs",
            body,
            logged_in,
            "",
            "",
        )),
    )
        .into_response()
}

const DOC_OVERVIEW: &str = r#"
<div class="docs-eyebrow">Documentation</div>
<h1>ModelRelay Overview &amp; Quickstart</h1>
<p class="docs-lead">ModelRelay puts one authenticated HTTPS endpoint &mdash; <code>https://api.modelrelay.io</code> &mdash; in front of your local GPU machines. Workers connect <em>out</em> over WebSocket, so you never open firewall ports or update client URLs again.</p>

<div class="doc-cards">
  <a href="/docs/api" class="doc-card">
    <h3>api.modelrelay.io Reference &rarr;</h3>
    <p>Endpoints, Bearer / x-api-key auth, routing headers, streaming SSE, and copy-paste <code>curl</code> commands.</p>
  </a>
  <a href="/docs/pi" class="doc-card">
    <h3>Pi Coding Agent Setup &rarr;</h3>
    <p>Register ModelRelay as a live-discovering provider extension and store your token in <code>~/.pi/agent/auth.json</code>.</p>
  </a>
  <a href="/docs/workers" class="doc-card">
    <h3>Desktop App &amp; Workers &rarr;</h3>
    <p>Connect macOS, Linux, or Windows machines running llama.cpp, Ollama, vLLM, or LM Studio.</p>
  </a>
  <a href="/docs/self-hosting" class="doc-card">
    <h3>Self-Hosting &rarr;</h3>
    <p>Deploy <code>modelrelay-server</code> with Docker, binaries, TLS reverse proxies, and admin API keys.</p>
  </a>
</div>

<h2>3-Minute Quickstart</h2>

<h3>1. Connect a GPU worker</h3>
<p>Install <a href="/downloads">ModelRelay Desktop</a> (tray app) or run <code>modelrelay-worker</code> alongside your local backend (<code>llama-server</code>, Ollama, vLLM, or LM Studio):</p>
<div class="code-shell">
  <div class="code-shell-header"><span>start a worker</span><button type="button" class="copy-btn" data-copy-target="doc-qs-1">Copy</button></div>
  <pre><code id="doc-qs-1">./modelrelay-worker \
  --proxy-url https://api.modelrelay.io \
  --worker-secret "$MODELRELAY_API_KEY" \
  --backend-url http://127.0.0.1:8000 \
  --models "*"</code></pre>
</div>

<h3>2. Verify your endpoint with <code>curl</code></h3>
<p>Check that your worker&rsquo;s models are live on <code>api.modelrelay.io</code>:</p>
<div class="code-shell">
  <div class="code-shell-header"><span>list live models</span><button type="button" class="copy-btn" data-copy-target="doc-qs-2">Copy</button></div>
  <pre><code id="doc-qs-2">curl https://api.modelrelay.io/v1/models \
  -H "Authorization: Bearer $MODELRELAY_API_KEY"</code></pre>
</div>

<h3>3. Register ModelRelay as a provider in Pi</h3>
<p>Follow the <a href="/docs/pi">Pi Coding Agent Setup</a> (or the interactive <a href="/integrate#pi">Pi &amp; API page</a> with your API key pre-filled) to drop <code>~/.pi/agent/extensions/modelrelay.ts</code> and add your token to <code>~/.pi/agent/auth.json</code>. Every model loaded on your workers then appears automatically in <code>/model</code>.</p>
"#;

const DOC_API: &str = r#"
<div class="docs-eyebrow">HTTP API Reference</div>
<h1>The <code>api.modelrelay.io</code> Endpoint</h1>
<p class="docs-lead"><code>https://api.modelrelay.io</code> exposes standard OpenAI and Anthropic HTTP endpoints backed by your connected WebSocket workers.</p>

<h2>Authentication</h2>
<p>Every request to <code>/v1/*</code> requires your ModelRelay API key (found in your <a href="/dashboard">Dashboard</a>). ModelRelay accepts either header format:</p>
<ul>
  <li><code>Authorization: Bearer mr_live_...</code> &mdash; standard for OpenAI Chat Completions, Responses, and <code>/v1/models</code>.</li>
  <li><code>x-api-key: mr_live_...</code> &mdash; standard for Anthropic Messages (<code>/v1/messages</code>).</li>
</ul>

<h2>Endpoints</h2>
<div class="doc-table-wrap">
  <table class="doc-table">
    <thead><tr><th>Method &amp; Path</th><th>Protocol</th><th>Notes</th></tr></thead>
    <tbody>
      <tr><td><code>GET /v1/models</code></td><td>OpenAI model list</td><td>Returns live model IDs advertised by your connected workers.</td></tr>
      <tr><td><code>POST /v1/chat/completions</code></td><td>OpenAI Chat Completions</td><td>Unary JSON or SSE streaming (<code>"stream": true</code>).</td></tr>
      <tr><td><code>POST /v1/responses</code></td><td>OpenAI Responses API</td><td>Supports <code>input</code> string/items and SSE event streams.</td></tr>
      <tr><td><code>POST /v1/messages</code></td><td>Anthropic Messages API</td><td>Send <code>x-api-key</code> and <code>anthropic-version: 2023-06-01</code>.</td></tr>
      <tr><td><code>GET /health</code></td><td>Relay health</td><td>Unauthenticated JSON status (<code>status</code>, <code>version</code>, <code>uptime_secs</code>).</td></tr>
    </tbody>
  </table>
</div>

<h2>curl Examples</h2>

<h3>List live models</h3>
<div class="code-shell">
  <div class="code-shell-header"><span>GET /v1/models</span><button type="button" class="copy-btn" data-copy-target="doc-api-models">Copy</button></div>
  <pre><code id="doc-api-models">curl https://api.modelrelay.io/v1/models \
  -H "Authorization: Bearer $MODELRELAY_API_KEY"</code></pre>
</div>

<h3>Chat Completions (non-streaming)</h3>
<div class="code-shell">
  <div class="code-shell-header"><span>POST /v1/chat/completions</span><button type="button" class="copy-btn" data-copy-target="doc-api-chat">Copy</button></div>
  <pre><code id="doc-api-chat">curl https://api.modelrelay.io/v1/chat/completions \
  -H "Content-Type: application/json" \
  -H "Authorization: Bearer $MODELRELAY_API_KEY" \
  -d '{
    "model": "qwen3.5-27b",
    "messages": [{"role": "user", "content": "Explain WebSockets in one sentence."}]
  }'</code></pre>
</div>

<h3>Chat Completions (streaming SSE)</h3>
<p>Pass <code>-N</code> to disable curl output buffering. Cancelling with <code>Ctrl+C</code> drops the HTTP connection and propagates a WebSocket <code>Cancel</code> frame to abort generation on the worker.</p>
<div class="code-shell">
  <div class="code-shell-header"><span>POST /v1/chat/completions (stream)</span><button type="button" class="copy-btn" data-copy-target="doc-api-stream">Copy</button></div>
  <pre><code id="doc-api-stream">curl -N https://api.modelrelay.io/v1/chat/completions \
  -H "Content-Type: application/json" \
  -H "Authorization: Bearer $MODELRELAY_API_KEY" \
  -d '{
    "model": "qwen3.5-27b",
    "stream": true,
    "messages": [{"role": "user", "content": "Write a haiku about local GPUs."}]
  }'</code></pre>
</div>

<h3>OpenAI Responses API</h3>
<div class="code-shell">
  <div class="code-shell-header"><span>POST /v1/responses</span><button type="button" class="copy-btn" data-copy-target="doc-api-resp">Copy</button></div>
  <pre><code id="doc-api-resp">curl https://api.modelrelay.io/v1/responses \
  -H "Content-Type: application/json" \
  -H "Authorization: Bearer $MODELRELAY_API_KEY" \
  -d '{
    "model": "qwen3.5-27b",
    "input": "List three advantages of local inference."
  }'</code></pre>
</div>

<h3>Anthropic Messages API</h3>
<div class="code-shell">
  <div class="code-shell-header"><span>POST /v1/messages</span><button type="button" class="copy-btn" data-copy-target="doc-api-anth">Copy</button></div>
  <pre><code id="doc-api-anth">curl https://api.modelrelay.io/v1/messages \
  -H "Content-Type: application/json" \
  -H "x-api-key: $MODELRELAY_API_KEY" \
  -H "anthropic-version: 2023-06-01" \
  -d '{
    "model": "qwen3.5-27b",
    "max_tokens": 256,
    "messages": [{"role": "user", "content": "Hello!"}]
  }'</code></pre>
</div>

<h2>Routing Headers &amp; Status Codes</h2>
<p>The server reads <code>model</code> from the JSON request body (or falls back to an <code>X-Model</code> header). If you run multiple named worker groups, target a specific provider with <code>X-Provider: home-rig</code> or <code>"provider": "home-rig"</code> in the JSON body.</p>
<div class="doc-table-wrap">
  <table class="doc-table">
    <thead><tr><th>HTTP Status</th><th>Meaning</th></tr></thead>
    <tbody>
      <tr><td><code>400 Bad Request</code></td><td>Missing <code>model</code> field (in JSON body or <code>X-Model</code> header).</td></tr>
      <tr><td><code>401 Unauthorized</code></td><td>Missing, invalid, or revoked API key.</td></tr>
      <tr><td><code>502 Bad Gateway</code></td><td>The worker reached an error or unreachable local backend.</td></tr>
      <tr><td><code>503 Service Unavailable</code></td><td>No connected worker advertises the requested model, or the queue is full.</td></tr>
      <tr><td><code>504 Gateway Timeout</code></td><td>Request timed out waiting in the queue or waiting for the backend.</td></tr>
    </tbody>
  </table>
</div>

<h2 id="oauth">OAuth PKCE for Browser &amp; Web Apps</h2>
<p>Browser applications (such as <a href="https://chat.neuralcolumn.com" target="_blank" rel="noopener">chat.neuralcolumn.com</a>) can connect a user&rsquo;s ModelRelay account with OAuth 2.0 PKCE (<code>S256</code>) so users never copy-paste an API key:</p>
<ol>
  <li>Generate a 32-byte random <code>code_verifier</code>, compute <code>code_challenge = BASE64URL(SHA256(code_verifier))</code>, and redirect the browser to:<br>
    <code>https://modelrelay.io/oauth/authorize?callback_url=https://your.app/&amp;code_challenge=...&amp;code_challenge_method=S256&amp;state=...&amp;key_label=Your+App</code>
  </li>
  <li>After the user approves, ModelRelay redirects back to <code>callback_url?code=...&amp;state=...</code> (or <code>?error=access_denied</code> on cancel).</li>
  <li>Exchange the single-use code directly from the browser via CORS-enabled <code>POST https://modelrelay.io/api/v1/auth/keys</code> (or <code>POST /oauth/token</code>):</li>
</ol>
<div class="code-shell">
  <div class="code-shell-header"><span>POST https://modelrelay.io/api/v1/auth/keys</span><button type="button" class="copy-btn" data-copy-target="doc-api-oauth">Copy</button></div>
  <pre><code id="doc-api-oauth">curl -X POST https://modelrelay.io/api/v1/auth/keys \
  -H "Content-Type: application/json" \
  -d '{
    "code": "mrc_...",
    "code_verifier": "...",
    "code_challenge_method": "S256"
  }'
# Returns: {"key":"mr_live_...","access_token":"mr_live_...","token_type":"Bearer","api_base_url":"https://api.modelrelay.io/v1"}</code></pre>
</div>
"#;

const DOC_PI: &str = r#"
<div class="docs-eyebrow">Recommended Coding Agent</div>
<h1>Pi Coding Agent Setup</h1>
<p class="docs-lead">Configure ModelRelay as a native <strong>provider</strong> in Pi (<code>@earendil-works/pi-coding-agent</code>) with your API token stored in <code>~/.pi/agent/auth.json</code>, plus full thinking-level, reasoning-effort, and context-window conveyance.</p>

<div class="callout">
  <strong>Why a provider extension + <code>auth.json</code>?</strong><br>
  A provider extension calls <code>GET https://api.modelrelay.io/v1/models</code> on refresh so every live model on your GPU workers appears automatically in Pi&rsquo;s <code>/model</code> picker with proper <code>reasoning</code>, <code>thinkingLevelMap</code>, <code>compat</code>, <code>contextWindow</code>, and <code>maxTokens</code> metadata &mdash; while <code>auth.json</code> keeps secrets out of config files and supports <code>/login modelrelay</code> and keychain <code>!command</code> lookups.
</div>

<h2>1. Install Pi</h2>
<div class="code-shell">
  <div class="code-shell-header"><span>macOS / Linux</span><button type="button" class="copy-btn" data-copy-target="doc-pi-install">Copy</button></div>
  <pre><code id="doc-pi-install">curl -fsSL https://pi.dev/install.sh | sh</code></pre>
</div>
<p>On Windows PowerShell run <code>powershell -c "irm https://pi.dev/install.ps1 | iex"</code>, or install via npm (Node 22.19+): <code>npm install -g --ignore-scripts @earendil-works/pi-coding-agent</code>.</p>

<h2>2. Register ModelRelay as a Provider</h2>
<p>Save the extension below to <code>~/.pi/agent/extensions/modelrelay.ts</code> (or customize your context window and thinking mode interactively on <a href="/integrate#pi">Pi &amp; API Setup</a>). Pi loads extensions in that directory automatically on startup and persists the full model metadata &mdash; including <code>thinkingLevelMap</code> and <code>compat</code> &mdash; for instant offline startup:</p>
<div class="code-shell">
  <div class="code-shell-header"><span>~/.pi/agent/extensions/modelrelay.ts</span><button type="button" class="copy-btn" data-copy-target="doc-pi-ext">Copy</button></div>
  <pre><code id="doc-pi-ext">import type { ExtensionAPI } from "@earendil-works/pi-coding-agent";

const DEFAULT_BASE_URL = "https://api.modelrelay.io/v1";
// Match your worker backend context limit (e.g. llama-server -c / vLLM --max-model-len)
const DEFAULT_CONTEXT_WINDOW = Number(process.env.MODELRELAY_CONTEXT_WINDOW) || 131072;
const DEFAULT_MAX_TOKENS = Number(process.env.MODELRELAY_MAX_TOKENS) || 32768;

type RelayModel = {
  id: string;
  context_window?: number;
  context_length?: number;
  max_model_len?: number;
  max_tokens?: number;
  max_completion_tokens?: number;
  reasoning?: boolean;
  meta?: { n_ctx?: number; n_ctx_train?: number };
  architecture?: { input_modalities?: string[] };
};

export default function (pi: ExtensionAPI) {
  pi.registerProvider("modelrelay", {
    name: "ModelRelay",
    baseUrl: DEFAULT_BASE_URL,
    api: "openai-completions",
    async refreshModels({ credential, stored, publish, allowNetwork, baseUrl, signal }) {
      const root = (baseUrl ?? DEFAULT_BASE_URL).replace(/\/+$/, "");
      const cached = (stored?.models ?? [])
        .filter((m) => m.provider === "modelrelay" && "maxTokens" in m)
        .map((m) => ({
          id: m.id,
          name: m.name,
          reasoning: m.reasoning,
          thinkingLevelMap: m.thinkingLevelMap,
          input: m.input,
          inputLimits: m.inputLimits,
          cost: m.cost,
          contextWindow: m.contextWindow,
          maxTokens: m.maxTokens,
          samplingParams: m.samplingParams,
          samplingParamsByThinkingLevel: m.samplingParamsByThinkingLevel,
          compat: m.compat,
        }));

      if (!allowNetwork || credential?.type !== "api_key" || !credential.key) {
        return cached;
      }

      const res = await fetch(`${root}/models`, {
        headers: { Authorization: `Bearer ${credential.key}` },
        signal,
      });
      if (!res.ok) return cached;

      const { data = [] } = (await res.json()) as { data?: RelayModel[] };
      const models = data.map((m) => {
        const contextWindow =
          m.meta?.n_ctx ||
          m.context_window ||
          m.context_length ||
          m.max_model_len ||
          m.meta?.n_ctx_train ||
          DEFAULT_CONTEXT_WINDOW;
        const maxTokens = Math.min(
          m.max_completion_tokens || m.max_tokens || DEFAULT_MAX_TOKENS,
          contextWindow,
        );
        const reasoning = m.reasoning ?? true;
        const hasVision = m.architecture?.input_modalities?.includes("image") === true;

        return {
          id: m.id,
          name: m.id,
          reasoning,
          ...(reasoning && {
            thinkingLevelMap: {
              off: "none",
              minimal: null,
              low: "low",
              medium: "medium",
              high: "high",
            },
          }),
          input: (hasVision ? ["text", "image"] : ["text"]) as ("text" | "image")[],
          cost: { input: 0, output: 0, cacheRead: 0, cacheWrite: 0 },
          contextWindow,
          maxTokens,
          compat: {
            supportsStore: false,
            supportsDeveloperRole: false,
            supportsReasoningEffort: reasoning,
            supportsUsageInStreaming: true,
            supportsStrictMode: false,
            maxTokensField: "max_tokens" as const,
            ...(reasoning && { thinkingFormat: "openai" as const }),
          },
        };
      });

      await publish({
        persist: {
          models: models.map((m) => ({
            ...m,
            api: "openai-completions" as const,
            provider: "modelrelay",
            baseUrl: root,
          })),
          checkedAt: Date.now(),
        },
      });

      return models;
    },
  });
}</code></pre>
</div>

<h2>3. Store Your Token in <code>~/.pi/agent/auth.json</code></h2>
<p>Add a <code>"modelrelay"</code> entry to <code>~/.pi/agent/auth.json</code> (or start Pi and run <code>/login modelrelay</code> to let Pi prompt and write it for you):</p>
<div class="code-shell">
  <div class="code-shell-header"><span>~/.pi/agent/auth.json</span><button type="button" class="copy-btn" data-copy-target="doc-pi-auth">Copy</button></div>
  <pre><code id="doc-pi-auth">{
  "modelrelay": {
    "type": "api_key",
    "key": "mr_live_your_api_key"
  }
}</code></pre>
</div>
<p>Lock down file permissions with <code>chmod 600 ~/.pi/agent/auth.json</code>. To pull the key from macOS Keychain or a password manager instead of storing plaintext, prefix <code>key</code> with <code>!</code> (for example <code>"!security find-generic-password -ws 'modelrelay'"</code>).</p>

<h2>4. Refresh, Select a Model &amp; Set Thinking Level</h2>
<div class="code-shell">
  <div class="code-shell-header"><span>shell</span><button type="button" class="copy-btn" data-copy-target="doc-pi-verify">Copy</button></div>
  <pre><code id="doc-pi-verify">pi update --models
pi --list-models</code></pre>
</div>
<p>Open <code>pi</code>, run <code>/model</code> to pick any ModelRelay model (<code>Ctrl+S</code> saves it as your default), and run <code>/thinking</code> (or press <code>Shift+Tab</code>) to switch between <code>off</code>, <code>low</code>, <code>medium</code>, and <code>high</code>.</p>

<h2 id="thinking">Thinking Levels &amp; Reasoning Effort</h2>
<p>Pi only offers <code>/thinking</code> choices when a model sets <code>reasoning: true</code>, and translates each level into wire parameters using <code>thinkingLevelMap</code> and <code>compat</code>:</p>
<ul>
  <li><strong><code>thinkingLevelMap.off = "none"</code>:</strong> Explicitly sends <code>"reasoning_effort": "none"</code> when you pick <code>/thinking off</code>, actively disabling thinking on <code>llama-server</code> and vLLM instead of leaving backend defaults on.</li>
  <li><strong><code>thinkingLevelMap.minimal = null</code>:</strong> Hides <code>minimal</code> from Pi&rsquo;s selector so strict Jinja chat templates (which only accept <code>low</code>, <code>medium</code>, and <code>high</code>) never reject requests with HTTP 400.</li>
  <li><strong><code>xhigh</code> / <code>max</code>:</strong> Opt-in in Pi &mdash; only offered if you map them to a string in <code>thinkingLevelMap</code>.</li>
  <li><strong><code>compat.thinkingTokenBudgetField</code>:</strong> Because reasoning and final answer share <code>max_tokens</code> on OpenAI-compatible servers, set a budget field to cap reasoning tokens per thinking level (<code>llama.cpp</code>: <code>"thinking_budget_tokens"</code>, vLLM: <code>"thinking_token_budget"</code>, Qwen/SGLang: <code>"thinking_budget"</code>).</li>
</ul>

<div class="doc-table-wrap">
  <table class="doc-table">
    <thead><tr><th>Backend / Model Family</th><th><code>compat.thinkingFormat</code></th><th>Wire Parameters Sent by Pi</th></tr></thead>
    <tbody>
      <tr><td>llama.cpp / vLLM / Ollama / gpt-oss</td><td><code>"openai"</code> (default)</td><td><code>reasoning_effort: "none" | "low" | "medium" | "high"</code> (+ optional <code>thinking_budget_tokens</code> / <code>thinking_token_budget</code>)</td></tr>
      <tr><td>Qwen3 / Qwen3.5 chat templates</td><td><code>"qwen-chat-template"</code></td><td><code>chat_template_kwargs: { enable_thinking, preserve_thinking: true }</code> (+ optional <code>thinking_budget_tokens</code>)</td></tr>
      <tr><td>DeepSeek R1 / V3 / V4</td><td><code>"deepseek"</code></td><td><code>thinking: { type: "enabled" | "disabled" }</code> + <code>reasoning_effort</code> + <code>requiresReasoningContentOnAssistantMessages: true</code></td></tr>
      <tr><td>GLM (ZAI)</td><td><code>"zai"</code></td><td><code>thinking: { type: "enabled", clear_thinking: false }</code> / <code>{ type: "disabled" }</code></td></tr>
    </tbody>
  </table>
</div>

<h2 id="context">Context Window, Output Limits &amp; Per-Model Overrides</h2>
<p>Pi relies on accurate <code>contextWindow</code> and <code>maxTokens</code> values for three things: displaying context usage in the footer, clamping <code>max_tokens</code> so <code>input + max_tokens</code> never exceeds your window, and triggering auto-compaction before your worker hits a KV-cache overflow. Note that <code>compat.supportsUsageInStreaming: true</code> ensures streamed turns return real token usage.</p>
<ul>
  <li>Set <code>MODELRELAY_CONTEXT_WINDOW</code> / <code>MODELRELAY_MAX_TOKENS</code> (or edit the defaults in <code>modelrelay.ts</code>) to match your worker&rsquo;s configured window (e.g. <code>llama-server -c 65536</code> or vLLM <code>--max-model-len</code>).</li>
  <li>For heterogeneous GPU fleets where different models have different context limits, vision support, or thinking formats, add per-model <code>modelOverrides</code> in <code>~/.pi/agent/models.json</code> (no API keys needed &mdash; Pi layers overrides cleanly on top of your extension):</li>
</ul>

<div class="code-shell">
  <div class="code-shell-header"><span>~/.pi/agent/models.json (per-model overrides)</span><button type="button" class="copy-btn" data-copy-target="doc-pi-overrides">Copy</button></div>
  <pre><code id="doc-pi-overrides">{
  "$schema": "https://pi.dev/schemas/models.schema.json",
  "providers": {
    "modelrelay": {
      "baseUrl": "https://api.modelrelay.io/v1",
      "modelOverrides": {
        "qwen3.5-27b": {
          "reasoning": true,
          "input": ["text", "image"],
          "contextWindow": 131072,
          "maxTokens": 32768,
          "thinkingLevelMap": {
            "off": "none",
            "minimal": null,
            "low": "low",
            "medium": "medium",
            "high": "high"
          },
          "compat": {
            "thinkingFormat": "qwen-chat-template",
            "thinkingTokenBudgetField": "thinking_budget_tokens"
          },
          "samplingParamsByThinkingLevel": {
            "off": { "temperature": 0.7, "top_p": 0.8 },
            "high": { "temperature": 1.0, "top_p": 0.95 }
          }
        },
        "llama-3.3-70b": {
          "reasoning": false,
          "contextWindow": 65536,
          "maxTokens": 16384
        }
      }
    }
  }
}</code></pre>
</div>

<h2>Troubleshooting</h2>
<ul>
  <li><strong>No ModelRelay models appear in <code>/model</code>:</strong> Confirm your worker is online with <code>curl https://api.modelrelay.io/v1/models -H "Authorization: Bearer $MODELRELAY_API_KEY"</code>, then run <code>pi update --models</code>.</li>
  <li><strong><code>/thinking</code> only shows <code>off</code>:</strong> Ensure the model has <code>reasoning: true</code> (check <code>pi --list-models</code>) and run <code>/reload</code> + <code>pi update --models</code> so cached catalog entries refresh.</li>
  <li><strong>Backend returns HTTP 400 on a thinking level:</strong> Hide unsupported levels with <code>null</code> in <code>thinkingLevelMap</code> (e.g. <code>"minimal": null</code>) or switch <code>compat.thinkingFormat</code> to match your server&rsquo;s chat template.</li>
  <li><strong>Context overflow before Pi compacts:</strong> Lower <code>contextWindow</code> in <code>modelrelay.ts</code> or <code>modelOverrides</code> to match your worker&rsquo;s actual <code>-c</code> / <code>--max-model-len</code> value.</li>
</ul>
"#;

const DOC_WORKERS: &str = r#"
<div class="docs-eyebrow">GPU Machines</div>
<h1>Desktop App &amp; Worker Daemons</h1>
<p class="docs-lead">A worker sits next to your local inference server (<code>llama-server</code>, Ollama, vLLM, or LM Studio) and bridges it to ModelRelay over an authenticated outbound WebSocket.</p>

<h2>Option A: ModelRelay Desktop App (Recommended)</h2>
<p>Download the native system-tray app from <a href="/downloads">Downloads</a>. Launch it, open <strong>Settings</strong> from the tray menu, and enter:</p>
<ul>
  <li><strong>Backend URL:</strong> e.g. <code>http://127.0.0.1:8000</code> (llama.cpp / vLLM), <code>http://127.0.0.1:11434</code> (Ollama), or <code>http://127.0.0.1:1234</code> (LM Studio).</li>
  <li><strong>Relay Server URL:</strong> <code>https://api.modelrelay.io</code> (or your self-hosted relay URL).</li>
  <li><strong>Worker Secret / API Key:</strong> your ModelRelay API key from the <a href="/dashboard">Dashboard</a>.</li>
  <li><strong>Models:</strong> leave as <code>*</code> to automatically advertise every model served by your local backend.</li>
</ul>
<p>The desktop app checks for signed updates automatically on startup and via <strong>Check for Updates&hellip;</strong>.</p>

<h2>Option B: Headless <code>modelrelay-worker</code> CLI</h2>
<p>For headless Linux servers or multi-GPU hosts, grab a pre-built binary from <a href="/downloads#cli">Downloads</a> and create a <code>config.toml</code>:</p>
<div class="code-shell">
  <div class="code-shell-header"><span>config.toml</span><button type="button" class="copy-btn" data-copy-target="doc-w-toml">Copy</button></div>
  <pre><code id="doc-w-toml">proxy_url = "https://api.modelrelay.io"
worker_secret = "mr_live_your_api_key"
worker_name = "workstation-4090"
backend_url = "http://127.0.0.1:8000"
models = ["*"]
max_concurrency = 4</code></pre>
</div>
<div class="code-shell">
  <div class="code-shell-header"><span>run worker</span><button type="button" class="copy-btn" data-copy-target="doc-w-run">Copy</button></div>
  <pre><code id="doc-w-run">./modelrelay-worker --config config.toml</code></pre>
</div>

<h2>Worker Environment Variables</h2>
<div class="doc-table-wrap">
  <table class="doc-table">
    <thead><tr><th>Env Var</th><th>Default</th><th>Description</th></tr></thead>
    <tbody>
      <tr><td><code>PROXY_URL</code></td><td><code>http://127.0.0.1:8080</code></td><td>Relay server URL (<code>https://api.modelrelay.io</code> for hosted).</td></tr>
      <tr><td><code>WORKER_SECRET</code></td><td><em>required</em></td><td>API key (hosted) or shared worker secret (self-hosted).</td></tr>
      <tr><td><code>BACKEND_URL</code></td><td><code>http://127.0.0.1:8000</code></td><td>Local OpenAI/Anthropic-compatible inference backend.</td></tr>
      <tr><td><code>WORKER_NAME</code></td><td><code>worker</code></td><td>Friendly label displayed in dashboards and logs.</td></tr>
      <tr><td><code>MODELS</code></td><td><code>*</code></td><td>Comma-separated model IDs, or <code>*</code> for automatic backend <code>/v1/models</code> discovery.</td></tr>
      <tr><td><code>LOG_LEVEL</code></td><td><code>info</code></td><td><code>trace</code>, <code>debug</code>, <code>info</code>, <code>warn</code>, or <code>error</code>.</td></tr>
    </tbody>
  </table>
</div>

<h2>Graceful Draining &amp; Persistence</h2>
<p>Sending <code>SIGTERM</code> (or stopping via <code>systemctl stop</code> / <code>Stop-Service</code>) puts the worker into graceful drain: the relay stops dispatching new requests to it while letting in-flight streams finish cleanly before disconnecting. Use the <a href="/setup">Setup Wizard</a> for copy-paste <code>systemd</code>, <code>launchd</code>, and Windows Service configs.</p>
"#;

const DOC_SELF_HOST: &str = r#"
<div class="docs-eyebrow">Open Source</div>
<h1>Self-Hosting ModelRelay</h1>
<p class="docs-lead">ModelRelay is MIT-licensed. You can host the entire relay server on any small VPS or internal server with Docker or a single Rust binary.</p>

<h2>Quickstart with Docker</h2>
<div class="code-shell">
  <div class="code-shell-header"><span>run modelrelay-server</span><button type="button" class="copy-btn" data-copy-target="doc-sh-docker">Copy</button></div>
  <pre><code id="doc-sh-docker">docker run -d --name modelrelay-server \
  -p 8080:8080 \
  -e LISTEN_ADDR=0.0.0.0:8080 \
  -e WORKER_SECRET=change-me-worker-secret \
  -e MODELRELAY_ADMIN_TOKEN=change-me-admin-token \
  -e MODELRELAY_REQUIRE_API_KEYS=true \
  ghcr.io/ericflo/modelrelay/modelrelay-server:latest</code></pre>
</div>

<h2>Create a Client API Key</h2>
<p>When <code>MODELRELAY_REQUIRE_API_KEYS=true</code>, create keys via the Admin API (or the built-in web UI):</p>
<div class="code-shell">
  <div class="code-shell-header"><span>POST /admin/keys</span><button type="button" class="copy-btn" data-copy-target="doc-sh-key">Copy</button></div>
  <pre><code id="doc-sh-key">curl -s -X POST http://localhost:8080/admin/keys \
  -H "Authorization: Bearer change-me-admin-token" \
  -H "Content-Type: application/json" \
  -d '{"name": "pi-laptop"}'</code></pre>
</div>

<h2>Server Configuration Reference</h2>
<div class="doc-table-wrap">
  <table class="doc-table">
    <thead><tr><th>Env Var</th><th>Default</th><th>Description</th></tr></thead>
    <tbody>
      <tr><td><code>LISTEN_ADDR</code></td><td><code>127.0.0.1:8080</code></td><td>HTTP bind address.</td></tr>
      <tr><td><code>WORKER_SECRET</code></td><td><em>required</em></td><td>Shared secret authenticating worker WebSocket handshakes.</td></tr>
      <tr><td><code>MODELRELAY_ADMIN_TOKEN</code></td><td><em>none</em></td><td>Bearer token enabling <code>/admin/workers</code>, <code>/admin/stats</code>, and <code>/admin/keys</code>.</td></tr>
      <tr><td><code>MODELRELAY_REQUIRE_API_KEYS</code></td><td><code>false</code></td><td>Require valid Bearer / <code>x-api-key</code> tokens on all <code>/v1/*</code> requests.</td></tr>
      <tr><td><code>MAX_QUEUE_LEN</code></td><td><code>100</code></td><td>Maximum queued requests before returning <code>503</code>.</td></tr>
      <tr><td><code>QUEUE_TIMEOUT_SECS</code></td><td><code>30</code></td><td>Max seconds a request waits in queue before returning <code>504</code>.</td></tr>
      <tr><td><code>REQUEST_TIMEOUT_SECS</code></td><td><code>300</code></td><td>Unary timeout / streaming chunk idle timeout in seconds.</td></tr>
    </tbody>
  </table>
</div>

<h2>TLS &amp; Reverse Proxy (nginx / Caddy)</h2>
<p>Terminate TLS in front of <code>modelrelay-server</code> with two critical settings: forward WebSocket <code>Upgrade</code> headers on <code>/v1/worker/connect</code> with long read timeouts, and set <code>proxy_buffering off</code> on <code>/v1/</code> so SSE streams arrive token-by-token:</p>
<div class="code-shell">
  <div class="code-shell-header"><span>nginx snippet</span><button type="button" class="copy-btn" data-copy-target="doc-sh-nginx">Copy</button></div>
  <pre><code id="doc-sh-nginx">location /v1/worker/connect {
    proxy_pass http://127.0.0.1:8080;
    proxy_http_version 1.1;
    proxy_set_header Upgrade $http_upgrade;
    proxy_set_header Connection "upgrade";
    proxy_read_timeout 86400s;
    proxy_send_timeout 86400s;
}

location /v1/ {
    proxy_pass http://127.0.0.1:8080;
    proxy_buffering off;
    proxy_cache off;
    proxy_read_timeout 300s;
}</code></pre>
</div>
<p>Using Caddy? Caddy handles WebSockets and unbuffered streaming automatically with <code>reverse_proxy 127.0.0.1:8080</code>.</p>
"#;

const DOC_ARCHITECTURE: &str = r#"
<div class="docs-eyebrow">Internals &amp; Reliability</div>
<h1>Architecture &amp; Protocol</h1>
<p class="docs-lead">ModelRelay is written in Rust (Tokio + Axum) and formally verified with a TLA+ protocol specification covering concurrency, routing, cancellation, and liveness.</p>

<h2>System Components</h2>
<ul>
  <li><strong><code>modelrelay-server</code>:</strong> Central async HTTP router, per-provider FIFO queue manager, load-aware round-robin dispatcher, WebSocket hub, and RAII cancellation guard.</li>
  <li><strong><code>modelrelay-worker</code>:</strong> Lightweight daemon that maintains an authenticated outbound WebSocket, refreshes models from the local backend, streams opaque response chunks back, reports live load in heartbeat pongs, and aborts backend HTTP tasks on cancel.</li>
  <li><strong><code>modelrelay-protocol</code>:</strong> Flat JSON tagged-enum WebSocket protocol (~12 message types: <code>Register</code>, <code>RegisterAck</code>, <code>Request</code>, <code>ResponseChunk</code>, <code>ResponseComplete</code>, <code>Cancel</code>, <code>Ping</code>, <code>Pong</code>, <code>ModelsUpdate</code>, <code>ModelsRefresh</code>, <code>GracefulShutdown</code>, <code>Error</code>).</li>
  <li><strong><code>modelrelay-desktop</code>:</strong> Tauri v2 tray wrapper around <code>modelrelay-worker</code> with signed auto-updates.</li>
</ul>

<h2>Reliability Guarantees</h2>
<ul>
  <li><strong>End-to-end RAII cancellation:</strong> Dropping an HTTP response future (client disconnect or timeout) immediately triggers a <code>Cancel</code> frame over WebSocket so the worker aborts its local backend HTTP call.</li>
  <li><strong>Automatic requeue on worker crash:</strong> If a worker disconnects before streaming begins, in-flight requests are requeued up to 3 attempts onto remaining healthy workers (never requeuing mid-stream to prevent corrupted output).</li>
  <li><strong>Tenant isolation:</strong> In multi-tenant hosted mode, each API key routes strictly to workers registered under that key &mdash; verified by formal TLA+ negative control <code>UnsafeCrossTenantDispatch</code>.</li>
  <li><strong>Formal TLA+ model checking:</strong> Every production build checks <code>spec/ModelRelay.tla</code> and 13 negative-control mutations alongside Rust unit, integration, and adversarial contract tests.</li>
</ul>

<p>Explore the complete wire walkthrough, behavior contract, and TLA+ specs in the <a href="https://github.com/ericflo/modelrelay/tree/main/docs" target="_blank" rel="noopener">GitHub repository</a>.</p>
"#;
