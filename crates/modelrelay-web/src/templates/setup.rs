use super::{CloudWizardConfig, cloud_config_script, html_escape, page_shell_custom};

const DEFAULT_RELAY_URL: &str = "https://api.modelrelay.io";
const DEFAULT_SITE_URL: &str = "https://modelrelay.io";

/// Build the worker setup page (default public URLs, logged-out).
#[must_use]
pub fn setup_wizard_page() -> String {
    setup_wizard_page_with_urls(None, DEFAULT_RELAY_URL, DEFAULT_SITE_URL)
}

/// Build the worker setup page, optionally pre-configured for logged-in cloud users.
#[must_use]
pub fn setup_wizard_page_with_config(cloud_config: Option<&CloudWizardConfig>) -> String {
    let relay_url = cloud_config.map_or(DEFAULT_RELAY_URL, |c| c.server_url.as_str());
    setup_wizard_page_with_urls(cloud_config, relay_url, DEFAULT_SITE_URL)
}

/// Build the worker setup page with explicit public relay and site URLs
/// (allowing self-hosted instances to override the default `api.modelrelay.io` / `modelrelay.io`).
#[must_use]
#[allow(clippy::too_many_lines)]
pub fn setup_wizard_page_with_urls(
    cloud_config: Option<&CloudWizardConfig>,
    relay_url: &str,
    site_url: &str,
) -> String {
    let relay_url = relay_url.trim().trim_end_matches('/');
    let relay_url = if relay_url.is_empty() {
        DEFAULT_RELAY_URL
    } else {
        relay_url
    };
    let relay_host = relay_url
        .strip_prefix("https://")
        .or_else(|| relay_url.strip_prefix("http://"))
        .unwrap_or(relay_url);

    let site_url = site_url.trim().trim_end_matches('/');
    let site_url = if site_url.is_empty() {
        DEFAULT_SITE_URL
    } else {
        site_url
    };
    let site_host = site_url
        .strip_prefix("https://")
        .or_else(|| site_url.strip_prefix("http://"))
        .unwrap_or(site_url);

    let esc_relay_url = html_escape(relay_url);
    let esc_relay_host = html_escape(relay_host);
    let esc_site_url = html_escape(site_url);
    let esc_site_host = html_escape(site_host);

    let setup_css = r"
    .setup-hero { margin-bottom: 24px; }
    .setup-hero .sec-eyebrow {
      font-size: 0.72rem; font-weight: 700; text-transform: uppercase;
      letter-spacing: 0.08em; color: var(--accent-bright); margin-bottom: 6px;
    }
    .setup-hero h1 {
      font-size: 2.15rem; font-weight: 800; letter-spacing: -0.03em; line-height: 1.12;
      margin-bottom: 10px; color: var(--text-strong);
    }
    .setup-hero p { color: var(--text-secondary); font-size: 1rem; max-width: 720px; margin-bottom: 16px; }
    .setup-jump { display: flex; gap: 10px; flex-wrap: wrap; }

    .cloud-banner {
      display: none; align-items: center; justify-content: space-between; gap: 12px; flex-wrap: wrap;
      background: var(--success-soft); border: 1px solid rgba(5, 150, 105, 0.35);
      border-radius: var(--radius-md); padding: 12px 16px; margin-bottom: 22px;
      font-size: 0.86rem; color: var(--success-fg);
    }
    .cloud-banner.visible { display: flex; }

    /* Section 1: Agentic Bootstrap Card (matches homepage #agents) */
    .agent-banner {
      background: var(--bg-surface); border: 1px solid var(--accent-border);
      border-radius: var(--radius-xl); padding: 26px; margin-bottom: 28px;
      box-shadow: var(--shadow-card); position: relative; overflow: hidden;
    }
    .agent-banner::before {
      content: ''; position: absolute; top: 0; left: 0; right: 0; height: 3px;
      background: linear-gradient(90deg, var(--accent), var(--cyan));
    }
    .agent-top {
      display: flex; justify-content: space-between; align-items: flex-end; gap: 16px;
      flex-wrap: wrap; margin-bottom: 18px;
    }
    .agent-top .eyebrow {
      font-size: 0.72rem; font-weight: 700; text-transform: uppercase;
      letter-spacing: 0.09em; color: var(--accent-bright); margin-bottom: 6px;
    }
    .agent-top h2 {
      font-size: 1.48rem; font-weight: 800; letter-spacing: -0.025em; line-height: 1.18;
      margin-bottom: 6px; color: var(--text-strong);
    }
    .agent-top p { color: var(--text-secondary); font-size: 0.92rem; max-width: 680px; margin: 0; }
    .agent-links { display: flex; gap: 10px; flex-wrap: wrap; align-items: center; }

    .prompt-box {
      background: var(--bg-code); border: 1px solid var(--code-border);
      border-radius: var(--radius-lg); overflow: hidden;
    }
    .prompt-raw { position: relative; }
    .prompt-raw pre {
      margin: 0; padding: 20px 136px 20px 20px;
      white-space: pre-wrap; overflow-wrap: anywhere; overflow-x: hidden;
      font-size: 0.85rem; line-height: 1.66; color: var(--code-fg);
    }
    .prompt-raw .copy-btn { position: absolute; top: 14px; right: 14px; }
    .prompt-note {
      padding: 10px 18px; border-top: 1px solid rgba(148, 163, 184, 0.16);
      background: var(--bg-code-head); font-size: 0.8rem; color: var(--code-muted);
      display: flex; justify-content: space-between; align-items: center; gap: 10px; flex-wrap: wrap;
    }
    .agent-oneliners {
      padding: 14px 18px 18px;
      background: var(--bg-subtle);
      border-top: 1px solid var(--border);
    }
    .agent-oneliners-label {
      font-size: 0.72rem; font-weight: 700; text-transform: uppercase; letter-spacing: 0.08em;
      color: var(--text-muted); margin-bottom: 10px;
    }
    .oneliner-list { display: flex; flex-direction: column; gap: 8px; }
    .oneliner-row {
      display: grid; grid-template-columns: 98px minmax(0, 1fr) auto; gap: 10px; align-items: center;
      padding: 8px 10px 8px 12px;
      background: var(--bg-code); border: 1px solid var(--code-border); border-radius: var(--radius-md);
    }
    .oneliner-client { font-size: 0.74rem; font-weight: 700; color: #c7d2fe; white-space: nowrap; }
    .oneliner-code {
      margin: 0; min-width: 0; overflow-x: auto; white-space: nowrap;
      font-family: var(--font-mono); font-size: 0.78rem; color: var(--code-fg);
      background: none; border: none; padding: 2px 0;
    }
    .oneliner-row .copy-btn { position: static; flex-shrink: 0; }

    /* Desktop Shortcut */
    .desktop-shortcut {
      display: flex; align-items: center; justify-content: space-between; gap: 18px; flex-wrap: wrap;
      background: var(--bg-surface); border: 1px solid var(--border);
      border-radius: var(--radius-lg); padding: 20px 22px; margin-bottom: 34px;
      box-shadow: var(--shadow-card);
    }
    .desktop-shortcut h2 { font-size: 1.05rem; font-weight: 700; margin-bottom: 4px; color: var(--text-strong); }
    .desktop-shortcut p { font-size: 0.88rem; color: var(--text-secondary); margin: 0; max-width: 560px; }
    .desktop-actions { display: flex; gap: 10px; flex-wrap: wrap; }

    /* Manual Section */
    .setup-section { margin-bottom: 36px; scroll-margin-top: 84px; }
    .sec-head {
      display: flex; justify-content: space-between; align-items: flex-end; gap: 16px;
      flex-wrap: wrap; margin-bottom: 18px;
    }
    .sec-eyebrow {
      font-size: 0.72rem; font-weight: 700; text-transform: uppercase;
      letter-spacing: 0.08em; color: var(--accent-bright); margin-bottom: 6px;
    }
    .sec-head h2 {
      font-size: 1.55rem; font-weight: 800; letter-spacing: -0.025em;
      color: var(--text-strong); margin: 0;
    }

    /* Config Card */
    .config-card {
      background: var(--bg-surface); border: 1px solid var(--border);
      border-radius: var(--radius-lg); padding: 20px; margin-bottom: 22px;
      box-shadow: var(--shadow-card);
    }
    .pill-group { margin-bottom: 14px; }
    .pill-label {
      display: block; font-size: 0.73rem; font-weight: 700; text-transform: uppercase;
      letter-spacing: 0.06em; color: var(--text-muted); margin-bottom: 8px;
    }
    .pill-bar { display: flex; gap: 8px; flex-wrap: wrap; }
    .pill-btn {
      padding: 7px 13px; background: var(--bg-input); border: 1px solid var(--border-strong);
      border-radius: 8px; color: var(--text-secondary); cursor: pointer;
      font-size: 0.82rem; font-weight: 600; font-family: inherit; transition: all 0.15s;
    }
    .pill-btn:hover { border-color: var(--accent-border); color: var(--text-strong); }
    .pill-btn.active { background: var(--accent); border-color: var(--accent); color: #fff; }

    .config-grid {
      display: grid; grid-template-columns: repeat(2, minmax(0, 1fr)); gap: 14px;
      margin-top: 16px; padding-top: 16px; border-top: 1px solid var(--border);
    }
    .cfg-field { min-width: 0; }
    .cfg-field label {
      display: block; font-size: 0.75rem; font-weight: 600; text-transform: uppercase;
      letter-spacing: 0.06em; color: var(--text-muted); margin-bottom: 6px;
    }
    .cfg-field input {
      width: 100%; min-width: 0; padding: 9px 12px; background: var(--bg-input);
      border: 1px solid var(--border-strong); border-radius: 8px; color: var(--text-primary);
      font-family: var(--font-mono); font-size: 0.84rem;
    }
    .cfg-field input:focus { outline: none; border-color: var(--accent); box-shadow: 0 0 0 3px var(--accent-soft); }
    .cfg-input-wrap { position: relative; display: flex; align-items: center; }
    .cfg-input-wrap input { padding-right: 60px; }
    .cfg-toggle {
      position: absolute; right: 8px; padding: 3px 8px; font-size: 0.72rem;
      background: var(--bg-elevated); color: var(--text-secondary); border: 1px solid var(--border);
      border-radius: 5px; cursor: pointer; font-family: inherit;
    }

    /* Numbered Step Cards (matches /integrate .pi-step) */
    .step-stack { display: flex; flex-direction: column; gap: 16px; }
    .step-card {
      background: var(--bg-surface); border: 1px solid var(--border);
      border-radius: var(--radius-lg); padding: 22px; box-shadow: var(--shadow-card);
      min-width: 0;
    }
    .step-head { display: flex; align-items: center; gap: 12px; margin-bottom: 10px; }
    .step-num {
      width: 28px; height: 28px; border-radius: 8px; flex-shrink: 0;
      display: inline-flex; align-items: center; justify-content: center;
      background: var(--accent-soft); border: 1px solid var(--accent-border);
      color: var(--accent-fg); font-size: 0.8rem; font-weight: 700;
    }
    .step-head h3 { font-size: 1.05rem; font-weight: 700; color: var(--text-strong); margin: 0; }
    .step-card > p { font-size: 0.88rem; color: var(--text-secondary); margin: 0 0 14px; }

    /* Tabs + Code Shell */
    .tab-group {
      background: var(--bg-surface); border: 1px solid var(--border);
      border-radius: var(--radius-md); overflow: hidden;
    }
    .tab-bar {
      display: flex; gap: 2px; overflow-x: auto; padding: 6px 8px 0;
      background: var(--bg-subtle); border-bottom: 1px solid var(--border);
    }
    .tab-btn {
      padding: 8px 13px; background: transparent; border: none; border-bottom: 2px solid transparent;
      color: var(--text-muted); font-size: 0.82rem; font-weight: 600; font-family: inherit;
      cursor: pointer; white-space: nowrap;
    }
    .tab-btn:hover { color: var(--text-strong); }
    .tab-btn.active { color: var(--text-strong); border-bottom-color: var(--accent-bright); }
    .tab-panel { display: none; padding: 14px; }
    .tab-panel.active { display: block; }
    .tab-note { font-size: 0.84rem; color: var(--text-secondary); margin: 0 0 12px; }

    .code-shell {
      background: var(--bg-code); border: 1px solid var(--code-border);
      border-radius: var(--radius-md); overflow: hidden;
    }
    .code-shell-header {
      display: flex; align-items: center; justify-content: space-between; gap: 10px;
      padding: 8px 14px; background: var(--bg-code-head);
      border-bottom: 1px solid rgba(148, 163, 184, 0.16);
      font-family: var(--font-mono); font-size: 0.74rem; color: var(--code-muted);
      flex-wrap: wrap;
    }
    .code-shell-header .copy-btn { position: static; }
    .code-shell pre {
      margin: 0; padding: 16px; overflow-x: auto;
      font-size: 0.81rem; line-height: 1.62; color: var(--code-fg);
    }

    .opt-box {
      margin-top: 14px; background: var(--bg-subtle); border: 1px solid var(--border);
      border-radius: var(--radius-md); overflow: hidden;
    }
    .opt-box summary {
      padding: 12px 16px; cursor: pointer; font-size: 0.86rem; font-weight: 600;
      color: var(--text-strong); list-style: none;
      display: flex; align-items: center; justify-content: space-between; gap: 8px;
    }
    .opt-box summary::-webkit-details-marker { display: none; }
    .opt-box summary::after { content: '+'; color: var(--accent-bright); font-size: 1.05rem; }
    .opt-box[open] summary::after { content: '\2212'; }
    .opt-body { padding: 0 16px 16px; border-top: 1px solid var(--border); }
    .opt-body p { font-size: 0.84rem; color: var(--text-secondary); margin: 12px 0; }

    /* Verify Controls */
    .verify-row {
      display: flex; align-items: center; gap: 10px; flex-wrap: wrap; margin-bottom: 12px;
    }
    .status-pill {
      display: inline-flex; align-items: center; gap: 8px; padding: 8px 13px;
      background: var(--bg-subtle); border: 1px solid var(--border);
      border-radius: var(--radius-md); font-size: 0.84rem; color: var(--text-secondary);
    }
    .status-dot { width: 9px; height: 9px; border-radius: 50%; background: var(--text-muted); flex-shrink: 0; }
    .status-dot.searching { background: var(--warning); animation: mr-pulse 1.4s ease-in-out infinite; }
    .status-dot.connected { background: var(--success); box-shadow: 0 0 8px rgba(52, 211, 153, 0.55); }
    @keyframes mr-pulse { 0%, 100% { opacity: 1; } 50% { opacity: 0.35; } }

    .discover-chips { display: flex; gap: 6px; flex-wrap: wrap; margin: 10px 0 14px; }
    .model-chip {
      padding: 5px 10px; border-radius: 999px; font-family: var(--font-mono); font-size: 0.76rem;
      background: var(--accent-soft); border: 1px solid var(--accent-border); color: var(--accent-fg);
      cursor: pointer; transition: background 0.15s; max-width: 100%; overflow-wrap: anywhere; text-align: left;
    }
    .model-chip:hover { background: var(--accent); color: #fff; }
    .model-chip-meta { opacity: 0.78; margin-left: 6px; font-size: 0.71rem; }

    .test-row { display: flex; gap: 10px; flex-wrap: wrap; align-items: center; margin-bottom: 12px; }
    .test-row input {
      flex: 1; min-width: 180px; padding: 9px 12px; background: var(--bg-input);
      border: 1px solid var(--border-strong); border-radius: 8px; color: var(--text-primary);
      font-family: var(--font-mono); font-size: 0.84rem;
    }
    .test-output {
      display: none; background: var(--bg-code); border: 1px solid var(--code-border);
      border-radius: var(--radius-md); padding: 14px; margin-bottom: 14px;
      font-family: var(--font-mono); font-size: 0.82rem; color: var(--code-fg);
      max-height: 240px; overflow: auto; white-space: pre-wrap; overflow-wrap: anywhere;
    }
    .test-output.visible { display: block; }

    .next-banner {
      margin-top: 24px; padding: 24px; text-align: center;
      background: linear-gradient(135deg, var(--accent-soft), var(--cyan-soft));
      border: 1px solid var(--accent-border); border-radius: var(--radius-lg);
    }
    .next-banner h3 { font-size: 1.15rem; font-weight: 700; color: var(--text-strong); margin-bottom: 6px; }
    .next-banner p { font-size: 0.9rem; color: var(--text-secondary); margin: 0 auto 16px; max-width: 540px; }
    .next-actions { display: flex; gap: 10px; justify-content: center; flex-wrap: wrap; }

    @media (max-width: 760px) {
      .config-grid { grid-template-columns: 1fr; }
    }
    @media (max-width: 600px) {
      .setup-hero h1 { font-size: 1.7rem; }
      .agent-banner { padding: 18px 16px; }
      .agent-top h2 { font-size: 1.28rem; }
      .prompt-raw pre { padding: 50px 14px 16px 14px; font-size: 0.81rem; }
      .oneliner-row { grid-template-columns: minmax(0, 1fr) auto; gap: 6px 10px; }
      .oneliner-client { grid-column: 1 / -1; }
      .step-card { padding: 18px 14px; }
      .config-card { padding: 16px 14px; }
      .desktop-shortcut { padding: 18px 16px; }
      .tab-panel { padding: 12px; }
    }
    ";

    let setup_js = format!(
        r#"
(function() {{
  const DEFAULT_RELAY = "{js_relay_url}";
  const $ = s => document.querySelector(s);
  const $$ = s => document.querySelectorAll(s);
  const cloudCfg = window.__mrCloudConfig || null;

  let selectedOs = 'linux-amd64';
  let selectedBackend = 'llamacpp';
  const ua = navigator.userAgent.toLowerCase();
  if (ua.includes('mac')) selectedOs = 'macos';
  else if (ua.includes('win')) selectedOs = 'windows';

  const relayInput = $('#cfg-server-url');
  const secretInput = $('#cfg-worker-secret');
  const nameInput = $('#cfg-worker-name');
  const backendUrlInput = $('#cfg-backend-url');
  const testModelInput = $('#test-model');

  const BACKEND_PORTS = {{
    llamacpp: '8000',
    vllm: '8000',
    lmstudio: '1234',
    ollama: '11434',
    sglang: '30000',
  }};

  const BACKEND_START_CMDS = {{
    llamacpp: 'llama-server -m your-model.gguf --host 127.0.0.1 --port 8000 --jinja',
    vllm: 'vllm serve Qwen/Qwen3-32B --host 127.0.0.1 --port 8000',
    lmstudio: '# Open LM Studio -> Developer tab -> load a model -> Start Server on port 1234',
    ollama: 'ollama pull qwen3:32b\nollama serve',
    sglang: 'python -m sglang.launch_server --model-path Qwen/Qwen3-32B --host 127.0.0.1 --port 30000',
  }};

  if (cloudCfg) {{
    const banner = $('#setup-cloud-banner');
    if (banner) banner.classList.add('visible');
    if (cloudCfg.serverUrl) relayInput.value = cloudCfg.serverUrl;
    if (cloudCfg.workerSecret) secretInput.value = cloudCfg.workerSecret;
    else if (cloudCfg.apiKey) secretInput.value = cloudCfg.apiKey;
  }}
  if (!relayInput.value) relayInput.value = localStorage.getItem('mr_server_url') || DEFAULT_RELAY;
  if (!secretInput.value) secretInput.value = localStorage.getItem('mr_test_api_key') || '';
  if (!nameInput.value) nameInput.value = localStorage.getItem('mr_worker_name') || 'my-gpu-box';
  if (!backendUrlInput.value) backendUrlInput.value = 'http://127.0.0.1:8000';
  if (!testModelInput.value) testModelInput.value = localStorage.getItem('mr_model_name') || '';

  const toggleBtn = $('#toggle-worker-secret');
  if (toggleBtn) {{
    toggleBtn.addEventListener('click', () => {{
      const isPwd = secretInput.type === 'password';
      secretInput.type = isPwd ? 'text' : 'password';
      toggleBtn.textContent = isPwd ? 'Hide' : 'Show';
    }});
  }}

  // Update desktop download button for OS
  const dlBtn = $('#setup-dl-btn');
  if (dlBtn) {{
    if (ua.includes('mac')) {{ dlBtn.href = '/download/desktop/macos'; dlBtn.textContent = 'Download for macOS →'; }}
    else if (ua.includes('win')) {{ dlBtn.href = '/download/desktop/windows'; dlBtn.textContent = 'Download for Windows →'; }}
    else {{ dlBtn.href = '/download/desktop/linux'; dlBtn.textContent = 'Download for Linux →'; }}
  }}

  function sv() {{ return (relayInput.value.trim() || DEFAULT_RELAY).replace(/\/+$/, ''); }}
  function sk() {{ return secretInput.value.trim() || 'mr_live_your_api_key'; }}
  function wn() {{ return nameInput.value.trim() || 'my-gpu-box'; }}
  function bu() {{ return (backendUrlInput.value.trim() || 'http://127.0.0.1:8000').replace(/\/+$/, ''); }}
  function tm() {{ return testModelInput.value.trim() || 'your-model'; }}
  function shQuote(v) {{ return "'" + v.replace(/'/g, "'\"'\"'") + "'"; }}

  function renderAll() {{
    const relay = sv();
    const secret = sk();
    const name = wn();
    const backend = bu();
    const model = tm();
    const dlBase = 'https://github.com/ericflo/modelrelay/releases/latest/download';

    // Step 1: Install binary / Docker
    const installTitle = $('#install-shell-title');
    const installCode = $('#install-cmd');
    if (selectedOs === 'linux-amd64') {{
      installTitle.textContent = 'Linux x86_64 — install to /usr/local/bin';
      installCode.textContent =
        'sudo curl -fsSL ' + dlBase + '/modelrelay-worker-linux-amd64 \\\n' +
        '  -o /usr/local/bin/modelrelay-worker && sudo chmod 0755 /usr/local/bin/modelrelay-worker';
    }} else if (selectedOs === 'linux-arm64') {{
      installTitle.textContent = 'Linux arm64 — install to /usr/local/bin';
      installCode.textContent =
        'sudo curl -fsSL ' + dlBase + '/modelrelay-worker-linux-arm64 \\\n' +
        '  -o /usr/local/bin/modelrelay-worker && sudo chmod 0755 /usr/local/bin/modelrelay-worker';
    }} else if (selectedOs === 'macos') {{
      installTitle.textContent = 'macOS Apple Silicon — install to /usr/local/bin';
      installCode.textContent =
        'sudo curl -fsSL ' + dlBase + '/modelrelay-worker-darwin-arm64 \\\n' +
        '  -o /usr/local/bin/modelrelay-worker && sudo chmod 0755 /usr/local/bin/modelrelay-worker';
    }} else if (selectedOs === 'windows') {{
      installTitle.textContent = 'Windows PowerShell';
      installCode.textContent =
        'New-Item -ItemType Directory -Force "$env:LOCALAPPDATA\\modelrelay" | Out-Null\n' +
        'Invoke-WebRequest -Uri "' + dlBase + '/modelrelay-worker-windows-amd64.exe" \\\n' +
        '  -OutFile "$env:LOCALAPPDATA\\modelrelay\\modelrelay-worker.exe"';
    }} else {{
      installTitle.textContent = 'Docker — pull official worker image';
      installCode.textContent = 'docker pull ghcr.io/ericflo/modelrelay/modelrelay-worker:latest';
    }}

    const backendStart = $('#backend-start-cmd');
    if (backendStart) backendStart.textContent = BACKEND_START_CMDS[selectedBackend] || BACKEND_START_CMDS.llamacpp;

    // Step 2: TOML, 1-command paste, env vars
    const toml =
      'proxy_url = "' + relay + '"\n' +
      'worker_secret = "' + secret + '"\n' +
      'worker_name = "' + name + '"\n' +
      'provider = "local"\n' +
      'backend_url = "' + backend + '"\n' +
      'models = ["*"]\n' +
      'max_concurrency = 4\n' +
      'models_poll_interval_secs = 30';
    $('#config-toml').textContent = toml;

    const envText =
      'export PROXY_URL=' + shQuote(relay) + '\n' +
      'export WORKER_SECRET=' + shQuote(secret) + '\n' +
      'export WORKER_NAME=' + shQuote(name) + '\n' +
      'export BACKEND_URL=' + shQuote(backend) + '\n' +
      'export MODELS="*"\n' +
      'export MAX_CONCURRENCY=4';
    $('#config-env').textContent = envText;

    const runTitle = $('#oneline-shell-title');
    const runCode = $('#config-oneline');
    if (selectedOs === 'windows') {{
      runTitle.textContent = 'Windows PowerShell — write worker.toml & start';
      runCode.textContent =
        'New-Item -ItemType Directory -Force "$env:USERPROFILE\\.config\\modelrelay" | Out-Null\n' +
        '@\'\n' + toml + '\n\'@ | Set-Content -Encoding UTF8 "$env:USERPROFILE\\.config\\modelrelay\\worker.toml"\n' +
        '& "$env:LOCALAPPDATA\\modelrelay\\modelrelay-worker.exe" --config "$env:USERPROFILE\\.config\\modelrelay\\worker.toml"';
    }} else if (selectedOs === 'docker') {{
      runTitle.textContent = 'Docker — write chmod 600 worker.toml & run';
      runCode.textContent =
        'mkdir -p ~/.config/modelrelay && (umask 077 && cat > ~/.config/modelrelay/worker.toml <<\'EOF\'\n' +
        toml + '\nEOF\n)\n' +
        'docker run -d --name modelrelay-worker --restart unless-stopped --network host \\\n' +
        '  -v ~/.config/modelrelay/worker.toml:/etc/modelrelay/worker.toml:ro \\\n' +
        '  ghcr.io/ericflo/modelrelay/modelrelay-worker:latest --config /etc/modelrelay/worker.toml';
    }} else {{
      runTitle.textContent = 'run in terminal — writes chmod 600 ~/.config/modelrelay/worker.toml & starts';
      runCode.textContent =
        'mkdir -p ~/.config/modelrelay && (umask 077 && cat > ~/.config/modelrelay/worker.toml <<\'EOF\'\n' +
        toml + '\nEOF\n)\n' +
        'modelrelay-worker --config ~/.config/modelrelay/worker.toml';
    }}

    // Step 3: Persistence (uses live config!)
    const persistTitle = $('#persist-shell-title');
    const persistCode = $('#persist-cmd');
    const persistNote = $('#persist-note');
    if (selectedOs === 'linux-amd64' || selectedOs === 'linux-arm64') {{
      persistTitle.textContent = 'Linux — systemd user service (no root required)';
      persistNote.innerHTML = 'Runs automatically on boot using <code>~/.config/modelrelay/worker.toml</code>. For headless servers, also run <code>loginctl enable-linger "$USER"</code>.';
      persistCode.textContent =
        'mkdir -p ~/.config/systemd/user\n' +
        'cat > ~/.config/systemd/user/modelrelay-worker.service <<\'EOF\'\n' +
        '[Unit]\n' +
        'Description=ModelRelay GPU Worker\n' +
        'After=network-online.target\n' +
        'Wants=network-online.target\n\n' +
        '[Service]\n' +
        'Type=simple\n' +
        'ExecStart=/usr/local/bin/modelrelay-worker --config %h/.config/modelrelay/worker.toml\n' +
        'Restart=always\n' +
        'RestartSec=3\n' +
        'KillSignal=SIGTERM\n' +
        'TimeoutStopSec=20\n\n' +
        '[Install]\n' +
        'WantedBy=default.target\n' +
        'EOF\n' +
        'systemctl --user daemon-reload && systemctl --user enable --now modelrelay-worker\n' +
        'loginctl enable-linger "$USER" 2>/dev/null || true\n' +
        'systemctl --user status modelrelay-worker --no-pager';
    }} else if (selectedOs === 'macos') {{
      persistTitle.textContent = 'macOS — launchd LaunchAgent';
      persistNote.innerHTML = 'For desktop Macs you can also use <a href="/downloads">ModelRelay Desktop</a> with <strong>Launch at Login</strong>.';
      persistCode.textContent =
        'mkdir -p ~/Library/LaunchAgents\n' +
        'cat > ~/Library/LaunchAgents/io.modelrelay.worker.plist <<EOF\n' +
        '<?xml version="1.0" encoding="UTF-8"?>\n' +
        '<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">\n' +
        '<plist version="1.0"><dict>\n' +
        '  <key>Label</key><string>io.modelrelay.worker</string>\n' +
        '  <key>ProgramArguments</key><array>\n' +
        '    <string>/usr/local/bin/modelrelay-worker</string>\n' +
        '    <string>--config</string>\n' +
        '    <string>$HOME/.config/modelrelay/worker.toml</string>\n' +
        '  </array>\n' +
        '  <key>RunAtLoad</key><true/>\n' +
        '  <key>KeepAlive</key><true/>\n' +
        '  <key>StandardOutPath</key><string>/tmp/modelrelay-worker.log</string>\n' +
        '  <key>StandardErrorPath</key><string>/tmp/modelrelay-worker.err</string>\n' +
        '</dict></plist>\n' +
        'EOF\n' +
        'launchctl bootstrap gui/$(id -u) ~/Library/LaunchAgents/io.modelrelay.worker.plist';
    }} else if (selectedOs === 'windows') {{
      persistTitle.textContent = 'Windows PowerShell — Scheduled Task at Startup';
      persistNote.innerHTML = 'Or install <a href="/download/desktop/windows">ModelRelay Desktop</a> and enable <strong>Launch at Login</strong>.';
      persistCode.textContent =
        '$action = New-ScheduledTaskAction -Execute "$env:LOCALAPPDATA\\modelrelay\\modelrelay-worker.exe" `\n' +
        '  -Argument "--config `"$env:USERPROFILE\\.config\\modelrelay\\worker.toml`""\n' +
        '$trigger = New-ScheduledTaskTrigger -AtStartup\n' +
        '$settings = New-ScheduledTaskSettingsSet -AllowStartIfOnBatteries -DontStopIfGoingOnBatteries -RestartCount 3 -RestartInterval (New-TimeSpan -Minutes 1)\n' +
        'Register-ScheduledTask -TaskName "ModelRelayWorker" -Action $action -Trigger $trigger -Settings $settings -RunLevel Highest -Force\n' +
        'Start-ScheduledTask -TaskName "ModelRelayWorker"';
    }} else {{
      persistTitle.textContent = 'Docker — --restart unless-stopped';
      persistNote.innerHTML = 'On macOS/Windows Docker Desktop, omit <code>--network host</code> and set <code>backend_url = "http://host.docker.internal:' + (BACKEND_PORTS[selectedBackend] || '8000') + '"</code>.';
      persistCode.textContent =
        'docker update --restart unless-stopped modelrelay-worker\n' +
        'docker logs -f --tail 50 modelrelay-worker';
    }}

    // Step 4: curl test
    $('#curl-test').textContent =
      'curl -N ' + relay + '/v1/chat/completions \\\n' +
      '  -H "Content-Type: application/json" \\\n' +
      '  -H "Authorization: Bearer ' + secret + '" \\\n' +
      '  -d \'{{"model":"' + model + '","stream":true,"max_tokens":64,"messages":[{{"role":"user","content":"Reply with OK."}}]}}\'';
  }}

  function setOs(os) {{
    selectedOs = os;
    $$('[data-os-pill]').forEach(b => b.classList.toggle('active', b.dataset.osPill === os));
    renderAll();
  }}

  function setBackend(b) {{
    selectedBackend = b;
    $$('[data-backend-pill]').forEach(btn => btn.classList.toggle('active', btn.dataset.backendPill === b));
    const port = BACKEND_PORTS[b] || '8000';
    backendUrlInput.value = 'http://127.0.0.1:' + port;
    renderAll();
  }}

  $$('[data-os-pill]').forEach(b => b.addEventListener('click', () => setOs(b.dataset.osPill)));
  $$('[data-backend-pill]').forEach(b => b.addEventListener('click', () => setBackend(b.dataset.backendPill)));

  // Tab switching
  $$('.tab-group').forEach(group => {{
    const btns = group.querySelectorAll('.tab-btn');
    const panels = group.querySelectorAll('.tab-panel');
    btns.forEach(btn => {{
      btn.addEventListener('click', () => {{
        btns.forEach(b => b.classList.remove('active'));
        panels.forEach(p => p.classList.remove('active'));
        btn.classList.add('active');
        const target = group.querySelector('.tab-panel[data-tab="' + btn.dataset.tab + '"]');
        if (target) target.classList.add('active');
      }});
    }});
  }});

  // Copy for .copy-code-btn inside .code-shell
  document.addEventListener('click', e => {{
    const btn = e.target.closest('.copy-code-btn');
    if (!btn) return;
    const shell = btn.closest('.code-shell');
    const target = shell ? shell.querySelector('.code-text') : null;
    if (!target) return;
    navigator.clipboard.writeText(target.textContent.trim()).then(() => {{
      const prev = btn.textContent;
      btn.textContent = 'Copied';
      btn.classList.add('copied');
      setTimeout(() => {{ btn.textContent = prev; btn.classList.remove('copied'); }}, 1500);
    }});
  }});

  relayInput.addEventListener('input', () => {{ localStorage.setItem('mr_server_url', relayInput.value.trim()); renderAll(); }});
  secretInput.addEventListener('input', () => {{ localStorage.setItem('mr_test_api_key', secretInput.value.trim()); renderAll(); }});
  nameInput.addEventListener('input', () => {{ localStorage.setItem('mr_worker_name', nameInput.value.trim()); renderAll(); }});
  backendUrlInput.addEventListener('input', renderAll);
  testModelInput.addEventListener('input', () => {{ localStorage.setItem('mr_model_name', testModelInput.value.trim()); renderAll(); }});

  // Live worker check
  const checkWorkerBtn = $('#check-workers-btn');
  const statusDot = $('#worker-dot');
  const statusText = $('#worker-status-text');
  async function checkWorkers() {{
    const pollUrl = (cloudCfg && cloudCfg.workersPollUrl) ? cloudCfg.workersPollUrl : (sv() + '/admin/workers');
    statusDot.className = 'status-dot searching';
    statusText.textContent = 'Checking connected workers…';
    try {{
      const opts = cloudCfg ? {{ credentials: 'same-origin' }} : {{}};
      const r = await fetch(pollUrl, opts);
      if (!r.ok) {{
        statusDot.className = 'status-dot';
        statusText.textContent = r.status === 401 || r.status === 403
          ? 'Sign in (or use Fetch live models below) to check live workers.'
          : 'Worker status returned HTTP ' + r.status;
        return;
      }}
      const d = await r.json();
      const workers = d.workers || [];
      if (workers.length === 0) {{
        statusDot.className = 'status-dot';
        statusText.textContent = 'No workers connected yet — start modelrelay-worker and click Refresh.';
        return;
      }}
      statusDot.className = 'status-dot connected';
      const names = workers.map(w => w.worker_name || w.worker_id).join(', ');
      statusText.textContent = workers.length + ' worker' + (workers.length === 1 ? '' : 's') + ' connected: ' + names;
      const firstModel = workers.flatMap(w => w.models || []).find(m => m && m !== '*');
      if (firstModel && !testModelInput.value.trim()) {{
        testModelInput.value = firstModel;
        renderAll();
      }}
    }} catch (err) {{
      statusDot.className = 'status-dot';
      statusText.textContent = 'Could not check workers: ' + err.message;
    }}
  }}
  if (checkWorkerBtn) checkWorkerBtn.addEventListener('click', checkWorkers);

  // Fetch live models via GET /v1/models
  function formatModelMeta(entry) {{
    const parts = [];
    const ctx = entry.meta?.n_ctx || entry.context_window || entry.context_length || entry.max_model_len || entry.max_context_length || entry.n_ctx || entry.contextWindow;
    if (typeof ctx === 'number' && ctx > 0) parts.push(ctx >= 1024 ? Math.round(ctx / 1024) + 'k' : String(ctx));
    const efforts = entry.reasoning_efforts || entry.supported_reasoning_efforts || entry.capabilities?.reasoning_efforts || entry.thinking_levels;
    if (Array.isArray(efforts) && efforts.length > 0) parts.push(efforts.join('/'));
    else if (entry.reasoning === true || entry.capabilities?.reasoning === true) parts.push('reasoning');
    return parts.join(' · ');
  }}

  const discoverBtn = $('#setup-discover-btn');
  const discoverChips = $('#setup-discover-chips');
  const testOutput = $('#test-result');
  if (discoverBtn) {{
    discoverBtn.addEventListener('click', async () => {{
      discoverBtn.disabled = true;
      discoverChips.textContent = '';
      try {{
        const headers = {{}};
        const key = secretInput.value.trim();
        if (key) headers['Authorization'] = 'Bearer ' + key;
        const r = await fetch(sv() + '/v1/models', {{ headers }});
        if (!r.ok) {{
          testOutput.className = 'test-output visible';
          testOutput.textContent = 'GET /v1/models returned HTTP ' + r.status + (r.status === 401 ? ' — enter your API key above.' : '');
          return;
        }}
        const payload = await r.json();
        const entries = (payload.data || []).filter(x => x && x.id);
        if (entries.length === 0) {{
          testOutput.className = 'test-output visible';
          testOutput.textContent = 'Connected to ' + sv() + '/v1/models, but no workers are advertising models yet.';
          return;
        }}
        statusDot.className = 'status-dot connected';
        statusText.textContent = entries.length + ' live model' + (entries.length === 1 ? '' : 's') + ' available on ' + sv();
        entries.forEach(entry => {{
          const chip = document.createElement('button');
          chip.type = 'button';
          chip.className = 'model-chip';
          chip.appendChild(document.createTextNode(entry.id));
          const meta = formatModelMeta(entry);
          if (meta) {{
            const tag = document.createElement('span');
            tag.className = 'model-chip-meta';
            tag.textContent = '· ' + meta;
            chip.appendChild(tag);
          }}
          chip.addEventListener('click', () => {{
            testModelInput.value = entry.id;
            localStorage.setItem('mr_model_name', entry.id);
            renderAll();
          }});
          discoverChips.appendChild(chip);
        }});
        if (!testModelInput.value.trim() && entries[0]) {{
          testModelInput.value = entries[0].id;
          renderAll();
        }}
      }} catch (err) {{
        testOutput.className = 'test-output visible';
        testOutput.textContent = 'Request failed: ' + err.message;
      }} finally {{
        discoverBtn.disabled = false;
      }}
    }});
  }}

  // Live inference test
  const testBtn = $('#test-btn');
  if (testBtn) {{
    testBtn.addEventListener('click', async () => {{
      testBtn.disabled = true;
      testBtn.textContent = 'Sending…';
      testOutput.className = 'test-output visible';
      testOutput.textContent = 'Sending test chat completion to ' + sv() + '/v1/chat/completions…';
      try {{
        const headers = {{ 'Content-Type': 'application/json' }};
        const key = secretInput.value.trim();
        if (key) headers['Authorization'] = 'Bearer ' + key;
        const r = await fetch(sv() + '/v1/chat/completions', {{
          method: 'POST',
          headers,
          body: JSON.stringify({{
            model: tm(),
            messages: [{{ role: 'user', content: 'Hello! Reply in one short sentence.' }}],
            max_tokens: 64,
          }}),
        }});
        const text = await r.text();
        if (r.ok) {{
          try {{
            const d = JSON.parse(text);
            const reply = d.choices?.[0]?.message?.content || text;
            testOutput.textContent = '✓ Success (' + (d.model || tm()) + ')\n\n' + reply;
          }} catch (_) {{
            testOutput.textContent = text;
          }}
        }} else {{
          testOutput.textContent = 'HTTP ' + r.status + ':\n' + text;
        }}
      }} catch (err) {{
        testOutput.textContent = 'Connection failed: ' + err.message;
      }} finally {{
        testBtn.disabled = false;
        testBtn.textContent = 'Send Test Request';
      }}
    }});
  }}

  setOs(selectedOs);
  setBackend('llamacpp');
}})();
"#,
        js_relay_url = relay_url
            .replace('\\', "\\\\")
            .replace('"', "\\\"")
            .replace('<', "\\u003c")
            .replace('>', "\\u003e")
    );

    let logged_in = cloud_config.is_some();
    let extra_css = format!("<style>{setup_css}</style>");

    let body = format!(
        r##"<div class="setup-hero">
      <div class="sec-eyebrow">GPU Worker Setup</div>
      <h1>Serve Your Local LLM on <code>{esc_relay_host}</code></h1>
      <p>Connect any machine running <strong>llama.cpp</strong>, <strong>vLLM</strong>, <strong>LM Studio</strong>, <strong>Ollama</strong>, or <strong>SGLang</strong> &mdash; outbound WebSocket only, zero open ports, live <code>/v1/models</code> metadata pass-through.</p>
      <div class="setup-jump">
        <a href="#agent-setup" class="btn">Agentic Setup (Recommended) &darr;</a>
        <a href="#manual" class="btn btn-secondary">Manual CLI &amp; Service</a>
        <a href="/downloads" class="btn btn-secondary">Desktop App</a>
      </div>
    </div>

    <div id="setup-cloud-banner" class="cloud-banner">
      <span>Logged in &mdash; your relay endpoint and active API key are pre-filled in every terminal command below.</span>
      <a href="/dashboard" style="color:var(--success-fg);font-weight:600;">Manage keys &rarr;</a>
    </div>

    <!-- ═══ 1. Agentic Bootstrap (matches homepage Serve an LLM) ═══ -->
    <section class="agent-banner" id="agent-setup">
      <div class="agent-top">
        <div>
          <div class="eyebrow">Agent-First Setup &middot; {esc_site_host}/AGENTS.md</div>
          <h2>Let Claude Code, Codex, or Pi set up this GPU box</h2>
          <p>Paste this prompt or run a one-liner on your GPU machine. Your agent detects your local LLM server, installs <code>modelrelay-worker</code>, authenticates via browser OAuth (never paste API keys into chat), and offers to set up permanent startup persistence.</p>
        </div>
        <div class="agent-links">
          <a href="/AGENTS.md" class="btn btn-sm">Read AGENTS.md &rarr;</a>
          <button type="button" class="btn btn-sm btn-secondary" data-copy-target="setup-agents-url">Copy AGENTS.md URL</button>
          <span id="setup-agents-url" style="display:none;">{esc_site_url}/AGENTS.md</span>
        </div>
      </div>

      <div class="prompt-box">
        <div class="prompt-raw">
          <button type="button" class="copy-btn" data-copy-target="setup-prompt-serve">Copy prompt</button>
<pre><code id="setup-prompt-serve">Read {esc_site_url}/AGENTS.md and set up this machine to serve its local LLM to {esc_relay_url}. Detect my running local LLM server (llama-server, Ollama, vLLM, LM Studio, SGLang, etc.), install modelrelay-worker with models=["*"], authenticate via ModelRelay's local loopback OAuth flow without asking me to paste an API key into chat (or direct me to {esc_site_url}/setup if headless), offer to configure it to start permanently on boot for whatever platform this machine is running (systemd on Linux, launchd on macOS, Scheduled Task on Windows, or Docker), and verify GET /v1/models and a streaming chat completion end-to-end.</code></pre>
        </div>
        <div class="prompt-note">
          <span>Paste into an existing <strong>Claude Code</strong>, <strong>Codex</strong>, or <strong>Pi</strong> session on your <strong>GPU machine</strong>.</span>
          <span>Browser OAuth (no keys in chat) &middot; Startup service for Linux, macOS &amp; Windows</span>
        </div>
        <div class="agent-oneliners">
          <div class="agent-oneliners-label">Or run a CLI one-liner on your GPU machine</div>
          <div class="oneliner-list">
            <div class="oneliner-row">
              <span class="oneliner-client">Claude Code</span>
              <code class="oneliner-code" id="setup-cmd-claude">claude "Read {esc_site_url}/AGENTS.md and set up this machine to serve its local LLM to {esc_relay_url} via local OAuth (no keys pasted in chat), and offer permanent startup persistence for this platform."</code>
              <button type="button" class="copy-btn" data-copy-target="setup-cmd-claude">Copy</button>
            </div>
            <div class="oneliner-row">
              <span class="oneliner-client">Codex CLI</span>
              <code class="oneliner-code" id="setup-cmd-codex">codex "Read {esc_site_url}/AGENTS.md and set up this machine to serve its local LLM to {esc_relay_url} via local OAuth (no keys pasted in chat), and offer permanent startup persistence for this platform."</code>
              <button type="button" class="copy-btn" data-copy-target="setup-cmd-codex">Copy</button>
            </div>
            <div class="oneliner-row">
              <span class="oneliner-client">Pi Agent</span>
              <code class="oneliner-code" id="setup-cmd-pi">pi "Read {esc_site_url}/AGENTS.md and set up this machine to serve its local LLM to {esc_relay_url} via local OAuth (no keys pasted in chat), and offer permanent startup persistence for this platform."</code>
              <button type="button" class="copy-btn" data-copy-target="setup-cmd-pi">Copy</button>
            </div>
          </div>
        </div>
      </div>
    </section>

    <!-- ═══ 2. Desktop App Shortcut ═══ -->
    <div class="desktop-shortcut" id="desktop">
      <div>
        <h2>Prefer a native desktop tray app?</h2>
        <p>Install <strong>ModelRelay Desktop</strong> on macOS, Linux, or Windows &mdash; visual connection status, live models, Launch at Login, and signed auto-updates with zero terminal commands.</p>
      </div>
      <div class="desktop-actions">
        <a id="setup-dl-btn" href="/download/desktop/macos" class="btn">Download Desktop App &rarr;</a>
        <a href="/downloads" class="btn btn-secondary">All Platforms</a>
      </div>
    </div>

    <!-- ═══ 3. Manual CLI & Startup Service (Pre-Filled Copy-Paste) ═══ -->
    <section class="setup-section" id="manual">
      <div class="sec-head">
        <div>
          <div class="sec-eyebrow">Manual &amp; Headless Terminal Setup</div>
          <h2>Install, Configure &amp; Run on Boot</h2>
        </div>
        <span class="badge">Every command updates live below</span>
      </div>

      <div class="config-card">
        <div class="pill-group">
          <span class="pill-label">1. GPU Machine OS / Runtime</span>
          <div class="pill-bar">
            <button type="button" class="pill-btn active" data-os-pill="linux-amd64">Linux x86_64</button>
            <button type="button" class="pill-btn" data-os-pill="linux-arm64">Linux arm64</button>
            <button type="button" class="pill-btn" data-os-pill="macos">macOS (Apple Silicon)</button>
            <button type="button" class="pill-btn" data-os-pill="windows">Windows</button>
            <button type="button" class="pill-btn" data-os-pill="docker">Docker</button>
          </div>
        </div>

        <div class="pill-group" style="margin-bottom:0;">
          <span class="pill-label">2. Local LLM Server</span>
          <div class="pill-bar">
            <button type="button" class="pill-btn active" data-backend-pill="llamacpp">llama.cpp (:8000)</button>
            <button type="button" class="pill-btn" data-backend-pill="vllm">vLLM (:8000)</button>
            <button type="button" class="pill-btn" data-backend-pill="lmstudio">LM Studio (:1234)</button>
            <button type="button" class="pill-btn" data-backend-pill="ollama">Ollama (:11434)</button>
            <button type="button" class="pill-btn" data-backend-pill="sglang">SGLang (:30000)</button>
          </div>
        </div>

        <div class="config-grid">
          <div class="cfg-field">
            <label for="cfg-server-url">Relay Endpoint</label>
            <input id="cfg-server-url" type="text" placeholder="{esc_relay_url}">
          </div>
          <div class="cfg-field">
            <label for="cfg-worker-secret">API Key / Worker Secret</label>
            <div class="cfg-input-wrap">
              <input id="cfg-worker-secret" type="password" placeholder="mr_live_...">
              <button type="button" class="cfg-toggle" id="toggle-worker-secret">Show</button>
            </div>
          </div>
          <div class="cfg-field">
            <label for="cfg-worker-name">Worker Name</label>
            <input id="cfg-worker-name" type="text" placeholder="my-gpu-box">
          </div>
          <div class="cfg-field">
            <label for="cfg-backend-url">Local Backend URL</label>
            <input id="cfg-backend-url" type="text" placeholder="http://127.0.0.1:8000">
          </div>
        </div>
      </div>

      <div class="step-stack">
        <!-- Step 1 -->
        <div class="step-card">
          <div class="step-head">
            <span class="step-num">1</span>
            <h3>Install <code>modelrelay-worker</code></h3>
          </div>
          <p>Download the standalone worker binary (or pull the Docker image). It connects outbound over WebSocket &mdash; no open firewall ports required.</p>
          <div class="code-shell">
            <div class="code-shell-header">
              <span id="install-shell-title">Linux x86_64</span>
              <button type="button" class="copy-btn copy-code-btn">Copy</button>
            </div>
            <pre><code id="install-cmd" class="code-text"></code></pre>
          </div>
          <details class="opt-box">
            <summary>Need to start your local LLM server first?</summary>
            <div class="opt-body">
              <p>Start your selected backend locally (verify with <code>curl http://127.0.0.1:&lt;port&gt;/v1/models</code>):</p>
              <div class="code-shell">
                <div class="code-shell-header"><span>local LLM server command</span><button type="button" class="copy-btn copy-code-btn">Copy</button></div>
                <pre><code id="backend-start-cmd" class="code-text"></code></pre>
              </div>
            </div>
          </details>
        </div>

        <!-- Step 2 -->
        <div class="step-card">
          <div class="step-head">
            <span class="step-num">2</span>
            <h3>Write <code>worker.toml</code> (<code>chmod 600</code>) &amp; Start</h3>
          </div>
          <p><code>models = ["*"]</code> automatically discovers your local backend&rsquo;s live models and passes through context windows, reasoning efforts, and vision support to clients.</p>
          <div class="tab-group">
            <div class="tab-bar">
              <button type="button" class="tab-btn active" data-tab="cfg-oneline">1-Command Terminal Paste</button>
              <button type="button" class="tab-btn" data-tab="cfg-toml">worker.toml</button>
              <button type="button" class="tab-btn" data-tab="cfg-env">Environment Vars</button>
            </div>
            <div class="tab-panel active" data-tab="cfg-oneline">
              <div class="code-shell">
                <div class="code-shell-header">
                  <span id="oneline-shell-title">run in terminal</span>
                  <button type="button" class="copy-btn copy-code-btn">Copy</button>
                </div>
                <pre><code id="config-oneline" class="code-text"></code></pre>
              </div>
            </div>
            <div class="tab-panel" data-tab="cfg-toml">
              <p class="tab-note">Save to <code>~/.config/modelrelay/worker.toml</code> with <code>chmod 600</code> and run <code>modelrelay-worker --config ~/.config/modelrelay/worker.toml</code>:</p>
              <div class="code-shell">
                <div class="code-shell-header"><span>~/.config/modelrelay/worker.toml</span><button type="button" class="copy-btn copy-code-btn">Copy</button></div>
                <pre><code id="config-toml" class="code-text"></code></pre>
              </div>
            </div>
            <div class="tab-panel" data-tab="cfg-env">
              <p class="tab-note">Or pass configuration via environment variables and run <code>modelrelay-worker</code>:</p>
              <div class="code-shell">
                <div class="code-shell-header"><span>shell environment</span><button type="button" class="copy-btn copy-code-btn">Copy</button></div>
                <pre><code id="config-env" class="code-text"></code></pre>
              </div>
            </div>
          </div>
        </div>

        <!-- Step 3 -->
        <div class="step-card">
          <div class="step-head">
            <span class="step-num">3</span>
            <h3>Keep It Running Permanently on Boot</h3>
          </div>
          <p id="persist-note">Runs automatically on startup using <code>~/.config/modelrelay/worker.toml</code>.</p>
          <div class="code-shell">
            <div class="code-shell-header">
              <span id="persist-shell-title">Startup Service</span>
              <button type="button" class="copy-btn copy-code-btn">Copy</button>
            </div>
            <pre><code id="persist-cmd" class="code-text"></code></pre>
          </div>
        </div>

        <!-- Step 4 -->
        <div class="step-card">
          <div class="step-head">
            <span class="step-num">4</span>
            <h3>Verify Connection &amp; Test Inference</h3>
          </div>
          <p>Confirm your worker is online, inspect its live models, and send a test chat completion directly from your browser or via <code>curl</code>.</p>

          <div class="verify-row">
            <button type="button" class="btn btn-sm btn-secondary" id="check-workers-btn">Check connected workers</button>
            <button type="button" class="btn btn-sm btn-secondary" id="setup-discover-btn">Fetch live models</button>
            <span class="status-pill">
              <span class="status-dot" id="worker-dot"></span>
              <span id="worker-status-text">Ready to verify</span>
            </span>
          </div>

          <div class="discover-chips" id="setup-discover-chips"></div>

          <div class="test-row">
            <input id="test-model" type="text" placeholder="Model ID (e.g. qwen3.5-27b)">
            <button type="button" class="btn" id="test-btn">Send Test Request</button>
          </div>

          <div id="test-result" class="test-output"></div>

          <div class="code-shell">
            <div class="code-shell-header"><span>curl verification</span><button type="button" class="copy-btn copy-code-btn">Copy</button></div>
            <pre><code id="curl-test" class="code-text"></code></pre>
          </div>
        </div>
      </div>

      <div class="next-banner">
        <h3>Ready to connect your laptop or dev machine?</h3>
        <p>Set up Pi Coding Agent (with browser OAuth &amp; zero <code>models.json</code> config), Codex CLI, or Claude Code in one minute.</p>
        <div class="next-actions">
          <a href="/integrate#pi" class="btn">Connect Pi, Codex &amp; Claude &rarr;</a>
          <a href="/dashboard" class="btn btn-secondary">Open Dashboard</a>
        </div>
      </div>
    </section>"##
    );

    let cloud_cfg_script = cloud_config_script(cloud_config);
    let extra_body_end = format!("{cloud_cfg_script}<script>{setup_js}</script>");

    page_shell_custom(
        "Worker Setup",
        "/setup",
        &body,
        logged_in,
        &extra_css,
        &extra_body_end,
    )
}
