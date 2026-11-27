use std::sync::Arc;

use axum::extract::State;
use axum::response::{Html, IntoResponse, Response};

use crate::state::CloudState;

use super::{OptionalSession, updater};

const DOWNLOADS_CSS: &str = r"
  .dl-hero {
    padding: 56px 0 44px; border-bottom: 1px solid var(--border);
    position: relative; overflow: hidden;
  }
  .dl-hero::before {
    content: ''; position: absolute; top: -160px; left: 50%; transform: translateX(-50%);
    width: 820px; height: 420px; pointer-events: none;
    background: radial-gradient(ellipse at center, rgba(99, 102, 241, 0.14) 0%, transparent 70%);
  }
  .dl-hero-inner { position: relative; z-index: 1; display: flex; justify-content: space-between; align-items: flex-end; gap: 24px; flex-wrap: wrap; }
  .dl-hero h1 { font-size: 2.35rem; font-weight: 800; letter-spacing: -0.03em; margin: 8px 0 10px; color: var(--text-strong); }
  .dl-hero p { color: var(--text-secondary); max-width: 600px; font-size: 1rem; }

  .dl-section { padding: 52px 0; border-bottom: 1px solid var(--border); }
  .dl-section h2 { font-size: 1.5rem; font-weight: 800; letter-spacing: -0.025em; margin-bottom: 8px; color: var(--text-strong); }
  .dl-section > .container > p { color: var(--text-secondary); margin-bottom: 24px; max-width: 680px; }

  .os-grid { display: grid; grid-template-columns: repeat(3, 1fr); gap: 18px; margin-bottom: 34px; }
  .os-card {
    background: var(--bg-surface); border: 1px solid var(--border);
    border-radius: var(--radius-lg); padding: 24px; display: flex; flex-direction: column;
    box-shadow: var(--shadow-card);
  }
  .os-card.active-os { border-color: var(--accent); box-shadow: 0 0 0 1px var(--accent-border), var(--shadow-card); }
  .os-card-head { display: flex; justify-content: space-between; align-items: center; margin-bottom: 10px; }
  .os-card h3 { font-size: 1.15rem; font-weight: 700; color: var(--text-strong); }
  .os-card p { font-size: 0.86rem; color: var(--text-secondary); margin-bottom: 18px; flex: 1; }
  .os-links { display: flex; flex-direction: column; gap: 8px; }
  .os-links .btn { width: 100%; justify-content: space-between; }

  /* Interactive Desktop Preview */
  .preview-shell {
    background: var(--bg-surface); border: 1px solid var(--border-strong);
    border-radius: var(--radius-xl); overflow: hidden; box-shadow: var(--shadow-card);
  }
  .preview-bar {
    display: flex; justify-content: space-between; align-items: center; gap: 12px; flex-wrap: wrap;
    padding: 12px 18px; border-bottom: 1px solid var(--border);
    background: var(--bg-subtle);
  }
  .preview-tabs { display: flex; gap: 6px; }
  .preview-tab {
    padding: 5px 12px; border-radius: 7px; border: 1px solid transparent;
    background: transparent; color: var(--text-muted); font-size: 0.82rem; font-weight: 600;
    cursor: pointer; font-family: inherit;
  }
  .preview-tab:hover { color: var(--text-strong); }
  .preview-tab.active { background: var(--accent-soft); border-color: var(--accent-border); color: var(--accent-fg); }
  .preview-stage { background: var(--bg-elevated); padding: 22px; display: flex; justify-content: center; }
  .preview-img { display: none; max-width: 820px; width: 100%; height: auto; border-radius: 10px; border: 1px solid var(--border-strong); box-shadow: var(--shadow-hero); }
  .preview-img.active { display: block; }

  /* CLI & Docker */
  .cli-tabs {
    background: var(--bg-surface); border: 1px solid var(--border);
    border-radius: var(--radius-lg); overflow: hidden; margin-bottom: 24px;
    box-shadow: var(--shadow-card);
  }
  .cli-bar { display: flex; gap: 2px; overflow-x: auto; padding: 6px 8px 0; border-bottom: 1px solid var(--border); background: var(--bg-subtle); }
  .cli-tab {
    padding: 9px 14px; background: transparent; border: none; border-bottom: 2px solid transparent;
    color: var(--text-muted); font-size: 0.84rem; font-weight: 600; cursor: pointer;
    font-family: inherit; white-space: nowrap;
  }
  .cli-tab:hover { color: var(--text-strong); }
  .cli-tab.active { color: var(--text-strong); border-bottom-color: var(--accent-bright); }
  .cli-pane { display: none; padding: 20px; }
  .cli-pane.active { display: block; }

  .trust-grid { display: grid; grid-template-columns: repeat(3, 1fr); gap: 16px; }
  .trust-item {
    background: var(--bg-surface); border: 1px solid var(--border);
    border-radius: var(--radius-md); padding: 18px; box-shadow: var(--shadow-card);
  }
  .trust-item strong { display: block; color: var(--text-strong); font-size: 0.92rem; margin-bottom: 4px; }
  .trust-item p { font-size: 0.84rem; color: var(--text-secondary); margin: 0; }

  @media (max-width: 860px) {
    .os-grid, .trust-grid { grid-template-columns: 1fr; }
    .dl-hero h1 { font-size: 1.9rem; }
  }
";

const DOWNLOADS_JS: &str = r#"
(function() {
  var ua = navigator.userAgent.toLowerCase();
  var os = 'linux';
  var label = 'Download for Linux (.deb)';
  var href = '/download/desktop/linux';
  if (ua.indexOf('mac') !== -1) {
    os = 'macos'; label = 'Download for macOS (Apple Silicon)'; href = '/download/desktop/macos';
  } else if (ua.indexOf('win') !== -1) {
    os = 'windows'; label = 'Download for Windows (.exe)'; href = '/download/desktop/windows';
  }
  var cta = document.getElementById('dl-auto-cta');
  if (cta) { cta.textContent = label; cta.setAttribute('href', href); }
  var card = document.querySelector('.os-card[data-os="' + os + '"]');
  if (card) {
    card.classList.add('active-os');
    var badge = card.querySelector('.os-badge');
    if (badge) { badge.textContent = 'Detected OS'; badge.className = 'badge badge-active os-badge'; }
  }

  document.querySelectorAll('.preview-tab').forEach(function(btn) {
    btn.addEventListener('click', function() {
      var id = btn.getAttribute('data-shot');
      document.querySelectorAll('.preview-tab').forEach(function(b) { b.classList.toggle('active', b === btn); });
      document.querySelectorAll('.preview-img').forEach(function(img) { img.classList.toggle('active', img.id === id); });
    });
  });

  document.querySelectorAll('.cli-tab').forEach(function(btn) {
    btn.addEventListener('click', function() {
      var key = btn.getAttribute('data-cli');
      document.querySelectorAll('.cli-tab').forEach(function(b) { b.classList.toggle('active', b === btn); });
      document.querySelectorAll('.cli-pane').forEach(function(p) { p.classList.toggle('active', p.getAttribute('data-cli-pane') === key); });
    });
  });
})();
"#;

/// GET /downloads — dedicated Downloads hub (Desktop app, CLI binaries, Docker).
#[allow(clippy::too_many_lines)]
pub async fn page(
    OptionalSession(session): OptionalSession,
    State(state): State<Arc<CloudState>>,
) -> Response {
    let logged_in = match session {
        Some(s) => s.get::<String>("user_id").await.ok().flatten().is_some(),
        None => false,
    };

    let version = updater::cached_release_version()
        .await
        .unwrap_or_else(|| env!("CARGO_PKG_VERSION").to_string());
    let _ = &state;
    let ver_escaped = modelrelay_web::templates::html_escape(&version);

    let main_html = format!(
        r#"
  <section class="dl-hero">
    <div class="container dl-hero-inner">
      <div>
        <span class="badge badge-accent">Latest Release &middot; v{ver_escaped}</span>
        <h1>Download ModelRelay</h1>
        <p>Install the native Desktop tray worker for one-click setup, or run headless <code>modelrelay-worker</code> and <code>modelrelay-server</code> binaries on Linux, macOS, Windows, or Docker.</p>
      </div>
      <div style="display:flex;gap:10px;flex-wrap:wrap;">
        <a id="dl-auto-cta" href="/download/desktop/macos" class="btn btn-lg">Download Desktop App &rarr;</a>
        <a href="https://github.com/ericflo/modelrelay/releases" target="_blank" rel="noopener" class="btn btn-lg btn-secondary">All GitHub Releases</a>
      </div>
    </div>
  </section>

  <!-- ═══ Desktop App ═══ -->
  <section class="dl-section">
    <div class="container">
      <h2>ModelRelay Desktop</h2>
      <p>Native system-tray application wrapping <code>modelrelay-worker</code>. Connects your local llama.cpp, Ollama, vLLM, or LM Studio server to <code>api.modelrelay.io</code> with live model discovery, Launch at Login, and signed in-place auto-updates.</p>

      <div class="os-grid">
        <div class="os-card" data-os="macos">
          <div class="os-card-head">
            <h3>macOS</h3>
            <span class="badge os-badge">Signed &amp; Notarized</span>
          </div>
          <p>Apple Developer ID signed and notarized DMG installers for Apple Silicon (M1&ndash;M4) and Intel Macs.</p>
          <div class="os-links">
            <a href="/download/desktop/macos" class="btn"><span>Apple Silicon (.dmg)</span><span>&darr;</span></a>
            <a href="/download/desktop/macos-intel" class="btn btn-secondary"><span>Intel x86_64 (.dmg)</span><span>&darr;</span></a>
          </div>
        </div>

        <div class="os-card" data-os="linux">
          <div class="os-card-head">
            <h3>Linux</h3>
            <span class="badge os-badge">x86_64</span>
          </div>
          <p>Native tray integration for Debian/Ubuntu (<code>.deb</code>) and portable <code>.AppImage</code> with automatic updater support.</p>
          <div class="os-links">
            <a href="/download/desktop/linux-deb" class="btn"><span>Debian / Ubuntu (.deb)</span><span>&darr;</span></a>
            <a href="/download/desktop/linux-appimage" class="btn btn-secondary"><span>Universal (.AppImage)</span><span>&darr;</span></a>
          </div>
        </div>

        <div class="os-card" data-os="windows">
          <div class="os-card-head">
            <h3>Windows</h3>
            <span class="badge os-badge">x86_64</span>
          </div>
          <p>NSIS <code>.exe</code> setup wizard and enterprise-friendly <code>.msi</code> package for Windows 10 &amp; 11 GPU rigs.</p>
          <div class="os-links">
            <a href="/download/desktop/windows" class="btn"><span>Windows Installer (.exe)</span><span>&darr;</span></a>
            <a href="/download/desktop/windows-msi" class="btn btn-secondary"><span>MSI Package (.msi)</span><span>&darr;</span></a>
          </div>
        </div>
      </div>

      <div class="preview-shell">
        <div class="preview-bar">
          <div class="preview-tabs">
            <button type="button" class="preview-tab active" data-shot="shot-dash">Dashboard</button>
            <button type="button" class="preview-tab" data-shot="shot-onboard">Onboarding</button>
            <button type="button" class="preview-tab" data-shot="shot-settings">Settings</button>
          </div>
          <span style="font-size:0.78rem;color:var(--text-muted);">Live tray worker &middot; signed auto-updates via modelrelay.io</span>
        </div>
        <div class="preview-stage">
          <img id="shot-dash" class="preview-img active" src="/assets/desktop-dashboard.png" alt="ModelRelay Desktop Dashboard" width="1200" height="900">
          <img id="shot-onboard" class="preview-img" src="/assets/desktop-onboarding.png" alt="ModelRelay Desktop Onboarding Wizard" width="1200" height="900" loading="lazy">
          <img id="shot-settings" class="preview-img" src="/assets/desktop-settings.png" alt="ModelRelay Desktop Settings" width="1200" height="900" loading="lazy">
        </div>
      </div>
    </div>
  </section>

  <!-- ═══ Headless CLI Binaries ═══ -->
  <section class="dl-section" id="cli">
    <div class="container">
      <h2>Headless CLI Binaries</h2>
      <p>Single static-style binaries for GPU servers, Linux boxes, and self-hosted relays &mdash; no runtime dependencies required.</p>

      <div class="cli-tabs">
        <div class="cli-bar">
          <button type="button" class="cli-tab active" data-cli="linux-amd64">Linux x86_64</button>
          <button type="button" class="cli-tab" data-cli="linux-arm64">Linux arm64</button>
          <button type="button" class="cli-tab" data-cli="darwin-arm64">macOS Apple Silicon</button>
          <button type="button" class="cli-tab" data-cli="windows-amd64">Windows x86_64</button>
          <button type="button" class="cli-tab" data-cli="docker">Docker</button>
        </div>

        <div class="cli-pane active" data-cli-pane="linux-amd64">
          <div class="code-shell" style="margin:0;">
            <div class="code-shell-header"><span>modelrelay-worker (Linux x86_64)</span><button type="button" class="copy-btn" data-copy-target="cli-l64">Copy</button></div>
            <pre><code id="cli-l64"># Download worker binary
curl -fsSL https://github.com/ericflo/modelrelay/releases/latest/download/modelrelay-worker-linux-amd64 \
  -o modelrelay-worker &amp;&amp; chmod +x modelrelay-worker

# Connect local backend (llama-server / Ollama / vLLM) to api.modelrelay.io
./modelrelay-worker \
  --proxy-url https://api.modelrelay.io \
  --worker-secret "$MODELRELAY_API_KEY" \
  --backend-url http://127.0.0.1:8000 \
  --models "*"</code></pre>
          </div>
        </div>

        <div class="cli-pane" data-cli-pane="linux-arm64">
          <div class="code-shell" style="margin:0;">
            <div class="code-shell-header"><span>modelrelay-worker (Linux arm64)</span><button type="button" class="copy-btn" data-copy-target="cli-larm">Copy</button></div>
            <pre><code id="cli-larm">curl -fsSL https://github.com/ericflo/modelrelay/releases/latest/download/modelrelay-worker-linux-arm64 \
  -o modelrelay-worker &amp;&amp; chmod +x modelrelay-worker</code></pre>
          </div>
        </div>

        <div class="cli-pane" data-cli-pane="darwin-arm64">
          <div class="code-shell" style="margin:0;">
            <div class="code-shell-header"><span>modelrelay-worker (macOS Apple Silicon)</span><button type="button" class="copy-btn" data-copy-target="cli-marm">Copy</button></div>
            <pre><code id="cli-marm">curl -fsSL https://github.com/ericflo/modelrelay/releases/latest/download/modelrelay-worker-darwin-arm64 \
  -o modelrelay-worker &amp;&amp; chmod +x modelrelay-worker</code></pre>
          </div>
        </div>

        <div class="cli-pane" data-cli-pane="windows-amd64">
          <div class="code-shell" style="margin:0;">
            <div class="code-shell-header"><span>modelrelay-worker (Windows PowerShell)</span><button type="button" class="copy-btn" data-copy-target="cli-w64">Copy</button></div>
            <pre><code id="cli-w64">curl.exe -L -o modelrelay-worker.exe `
  https://github.com/ericflo/modelrelay/releases/latest/download/modelrelay-worker-windows-amd64.exe</code></pre>
          </div>
        </div>

        <div class="cli-pane" data-cli-pane="docker">
          <div class="code-shell" style="margin:0;">
            <div class="code-shell-header"><span>Docker &mdash; GHCR images</span><button type="button" class="copy-btn" data-copy-target="cli-docker">Copy</button></div>
            <pre><code id="cli-docker"># Run worker container
docker run -d --restart unless-stopped \
  -e PROXY_URL=https://api.modelrelay.io \
  -e WORKER_SECRET="$MODELRELAY_API_KEY" \
  -e BACKEND_URL=http://host.docker.internal:8000 \
  -e MODELS="*" \
  ghcr.io/ericflo/modelrelay/modelrelay-worker:latest

# Or self-host the relay server
docker run -d -p 8080:8080 \
  -e WORKER_SECRET=my-worker-secret \
  -e LISTEN_ADDR=0.0.0.0:8080 \
  ghcr.io/ericflo/modelrelay/modelrelay-server:latest</code></pre>
          </div>
        </div>
      </div>

      <div class="trust-grid">
        <div class="trust-item">
          <strong>Signed Auto-Updates</strong>
          <p>Desktop updates are verified with minisign signatures before install &mdash; zero manual reinstalls.</p>
        </div>
        <div class="trust-item">
          <strong>Need Step-by-Step Worker Help?</strong>
          <p>Walk through backend setup, config generation, and systemd/launchd services in the <a href="/setup">Setup Wizard &rarr;</a></p>
        </div>
        <div class="trust-item">
          <strong>Ready to Connect Pi?</strong>
          <p>Once your worker is online, register ModelRelay as a Pi provider in 60 seconds: <a href="/integrate#pi">Pi Setup &rarr;</a></p>
        </div>
      </div>
    </div>
  </section>
"#
    );

    let extra_head = format!("<style>{DOWNLOADS_CSS}</style>");
    let extra_body_end = format!("<script>{DOWNLOADS_JS}</script>");

    Html(modelrelay_web::templates::page_shell_full(
        "Downloads",
        "/downloads",
        &main_html,
        logged_in,
        &extra_head,
        &extra_body_end,
    ))
    .into_response()
}
