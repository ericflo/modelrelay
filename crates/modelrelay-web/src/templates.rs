/// Build the full live admin dashboard page with embedded JS polling.
#[must_use]
#[allow(clippy::too_many_lines)]
pub fn dashboard_page() -> String {
    let dashboard_css = r"
    .health-bar { display:flex; gap:16px; flex-wrap:wrap; margin-bottom:24px; }
    .health-item {
      background:#161b22; border:1px solid #21262d; border-radius:10px;
      padding:16px 20px; flex:1; min-width:140px;
    }
    .health-item .label { font-size:0.78rem; color:#8b949e; text-transform:uppercase; letter-spacing:0.5px; }
    .health-item .value { font-size:1.5rem; font-weight:700; margin-top:4px; }
    .health-item .value.ok { color:#34d399; }
    .health-item .value.warn { color:#fbbf24; }
    .health-item .value.err { color:#f87171; }

    .panel { background:#161b22; border:1px solid #21262d; border-radius:12px; padding:24px; margin-bottom:24px; }
    .panel h2 { font-size:1.1rem; margin-bottom:16px; display:flex; align-items:center; gap:8px; }
    .panel h2 .dot { width:8px; height:8px; border-radius:50%; background:#34d399; display:inline-block; }
    .panel h2 .dot.off { background:#484f58; }

    .empty-state { color:#484f58; text-align:center; padding:24px 0; }
    .empty-state a { color:#7c3aed; }

    table.data { width:100%; border-collapse:collapse; font-size:0.9rem; }
    table.data th { text-align:left; color:#8b949e; font-weight:600; padding:8px 12px 8px 0; border-bottom:1px solid #21262d; }
    table.data td { padding:8px 12px 8px 0; border-bottom:1px solid #161b22; color:#e6edf3; }
    table.data tr:last-child td { border-bottom:none; }

    .model-tag {
      display:inline-block; padding:2px 8px; margin:1px 4px 1px 0; border-radius:4px;
      background:#1c1f26; border:1px solid #30363d; font-size:0.8rem; font-family:monospace;
    }

    .stat-row { display:flex; align-items:center; gap:12px; margin-bottom:10px; }
    .stat-row .stat-label { min-width:160px; color:#8b949e; font-size:0.9rem; font-family:monospace; }
    .stat-row .stat-bar { flex:1; height:20px; background:#0d1117; border-radius:4px; overflow:hidden; position:relative; }
    .stat-row .stat-fill { height:100%; background:#7c3aed; border-radius:4px; transition:width 0.3s; }
    .stat-row .stat-num { min-width:36px; text-align:right; font-weight:600; font-size:0.9rem; }

    .key-actions { display:flex; gap:8px; align-items:center; flex-wrap:wrap; margin-bottom:16px; }
    .key-actions input { padding:8px 12px; background:#0d1117; border:1px solid #30363d; border-radius:8px; color:#e6edf3; font-size:0.9rem; width:220px; }
    .key-actions input:focus { outline:none; border-color:#7c3aed; }

    .btn-sm {
      display:inline-block; padding:6px 14px; background:#7c3aed; color:#fff;
      border:none; border-radius:6px; font-size:0.82rem; font-weight:600; cursor:pointer;
    }
    .btn-sm:hover { background:#6d28d9; }
    .btn-sm.danger { background:#7f1d1d; }
    .btn-sm.danger:hover { background:#991b1b; }

    .secret-box {
      margin-top:12px; padding:14px 16px; background:#0d1117; border:2px solid #7c3aed;
      border-radius:8px; font-family:monospace; color:#7c3aed; word-break:break-all;
      position:relative;
    }
    .secret-box .warn { display:block; margin-top:8px; font-family:sans-serif; font-size:0.8rem; color:#fbbf24; }
    .copy-btn { position:absolute; top:10px; right:10px; padding:4px 10px; font-size:0.75rem; background:#30363d; color:#e6edf3; border:none; border-radius:4px; cursor:pointer; }
    .copy-btn:hover { background:#484f58; }

    .config-bar {
      display:flex; gap:8px; align-items:center; margin-bottom:24px; flex-wrap:wrap;
    }
    .config-bar label { color:#8b949e; font-size:0.85rem; }
    .config-bar input { padding:6px 10px; background:#0d1117; border:1px solid #30363d; border-radius:6px; color:#e6edf3; font-size:0.85rem; width:200px; }
    .config-bar input:focus { outline:none; border-color:#7c3aed; }
    .config-bar .status { font-size:0.8rem; }
    .config-bar .status.ok { color:#34d399; }
    .config-bar .status.fail { color:#f87171; }

    .fade-in { animation: fadeIn 0.3s ease; }
    @keyframes fadeIn { from { opacity:0; } to { opacity:1; } }
    ";

    let dashboard_js = r#"
(function() {
  const POLL_MS = 4000;
  let adminToken = localStorage.getItem('mr_admin_token') || '';
  let serverUrl = localStorage.getItem('mr_server_url') || '';

  const $ = (s) => document.querySelector(s);
  const $$ = (s) => document.querySelectorAll(s);

  function baseUrl() {
    return serverUrl || window.location.origin;
  }

  function authHeaders() {
    const h = { 'Content-Type': 'application/json' };
    if (adminToken) h['Authorization'] = 'Bearer ' + adminToken;
    return h;
  }

  function fmtDuration(secs) {
    if (secs < 60) return Math.floor(secs) + 's';
    if (secs < 3600) return Math.floor(secs/60) + 'm ' + Math.floor(secs%60) + 's';
    const h = Math.floor(secs/3600);
    const m = Math.floor((secs%3600)/60);
    return h + 'h ' + m + 'm';
  }

  function fmtTimestamp(ts) {
    if (!ts) return '—';
    return new Date(ts * 1000).toLocaleString();
  }

  function escHtml(s) {
    const d = document.createElement('div');
    d.textContent = s;
    return d.innerHTML;
  }

  // --- Config bar ---
  function initConfig() {
    const tokenInput = $('#cfg-token');
    const urlInput = $('#cfg-url');
    const status = $('#cfg-status');
    tokenInput.value = adminToken;
    urlInput.value = serverUrl;

    tokenInput.addEventListener('change', () => {
      adminToken = tokenInput.value.trim();
      localStorage.setItem('mr_admin_token', adminToken);
      pollAll();
    });
    urlInput.addEventListener('change', () => {
      serverUrl = urlInput.value.trim().replace(/\/+$/, '');
      localStorage.setItem('mr_server_url', serverUrl);
      pollAll();
    });
  }

  // --- Health ---
  async function pollHealth() {
    try {
      const r = await fetch(baseUrl() + '/health');
      if (!r.ok) throw new Error(r.status);
      const d = await r.json();
      $('#h-status').textContent = d.status || '—';
      $('#h-status').className = 'value ' + (d.status === 'ok' ? 'ok' : 'warn');
      $('#h-version').textContent = d.version || '—';
      $('#h-version').className = 'value ok';
      $('#h-workers').textContent = d.workers_connected ?? '—';
      $('#h-workers').className = 'value ' + ((d.workers_connected||0) > 0 ? 'ok' : 'warn');
      $('#h-queue').textContent = d.queue_depth ?? '—';
      $('#h-queue').className = 'value ' + ((d.queue_depth||0) > 0 ? 'warn' : 'ok');
      $('#h-uptime').textContent = fmtDuration(d.uptime_secs || 0);
      $('#h-uptime').className = 'value ok';
      $('#cfg-status').textContent = 'Connected';
      $('#cfg-status').className = 'status ok';
    } catch(e) {
      $('#h-status').textContent = 'error';
      $('#h-status').className = 'value err';
      $('#cfg-status').textContent = 'Connection failed';
      $('#cfg-status').className = 'status fail';
    }
  }

  // --- Workers ---
  async function pollWorkers() {
    const el = $('#workers-body');
    if (!adminToken) {
      el.innerHTML = '<div class="empty-state">Enter admin token above to view workers.</div>';
      return;
    }
    try {
      const r = await fetch(baseUrl() + '/admin/workers', { headers: authHeaders() });
      if (r.status === 403) {
        el.innerHTML = '<div class="empty-state" style="color:#f87171;">Invalid admin token.</div>';
        return;
      }
      if (!r.ok) throw new Error(r.status);
      const d = await r.json();
      const workers = d.workers || [];
      if (workers.length === 0) {
        el.innerHTML = '<div class="empty-state">No workers connected.<br><a href="/setup" class="btn-sm" style="margin-top:8px;display:inline-block;">Set up your first worker &rarr;</a></div>';
        return;
      }
      let html = '<table class="data"><thead><tr><th>Worker</th><th>Models</th><th>Load</th><th>Status</th></tr></thead><tbody>';
      for (const w of workers) {
        const models = (w.models||[]).map(m => '<span class="model-tag">' + escHtml(m) + '</span>').join('');
        const load = w.in_flight_count + ' / ' + w.max_concurrent;
        const status = w.is_draining
          ? '<span class="badge badge-warn">Draining</span>'
          : '<span class="badge badge-active">Active</span>';
        html += '<tr><td>' + escHtml(w.worker_name || w.worker_id) + '</td><td>' + models + '</td><td>' + load + '</td><td>' + status + '</td></tr>';
      }
      html += '</tbody></table>';
      el.innerHTML = html;
    } catch(e) {
      el.innerHTML = '<div class="empty-state" style="color:#f87171;">Failed to load workers.</div>';
    }
  }

  // --- Stats ---
  async function pollStats() {
    const el = $('#stats-body');
    if (!adminToken) {
      el.innerHTML = '<div class="empty-state">Enter admin token above to view stats.</div>';
      return;
    }
    try {
      const r = await fetch(baseUrl() + '/admin/stats', { headers: authHeaders() });
      if (r.status === 403) {
        el.innerHTML = '<div class="empty-state" style="color:#f87171;">Invalid admin token.</div>';
        return;
      }
      if (!r.ok) throw new Error(r.status);
      const d = await r.json();
      const qd = d.queue_depth || {};
      const models = Object.keys(qd);
      let html = '<div style="margin-bottom:12px;color:#8b949e;font-size:0.9rem;">Active workers: <strong style="color:#e6edf3;">' + (d.active_workers||0) + '</strong></div>';
      if (models.length === 0) {
        html += '<div class="empty-state">No models queued.</div>';
      } else {
        const maxQ = Math.max(1, ...models.map(m => qd[m]));
        for (const m of models) {
          const pct = Math.round((qd[m] / maxQ) * 100);
          html += '<div class="stat-row"><span class="stat-label">' + escHtml(m) + '</span>'
            + '<div class="stat-bar"><div class="stat-fill" style="width:' + pct + '%"></div></div>'
            + '<span class="stat-num">' + qd[m] + '</span></div>';
        }
      }
      el.innerHTML = html;
    } catch(e) {
      el.innerHTML = '<div class="empty-state" style="color:#f87171;">Failed to load stats.</div>';
    }
  }

  // --- API Keys ---
  async function pollKeys() {
    const el = $('#keys-body');
    if (!adminToken) {
      el.innerHTML = '<div class="empty-state">Enter admin token above to manage API keys.</div>';
      return;
    }
    try {
      const r = await fetch(baseUrl() + '/admin/keys', { headers: authHeaders() });
      if (r.status === 403) {
        el.innerHTML = '<div class="empty-state" style="color:#f87171;">Invalid admin token.</div>';
        return;
      }
      if (!r.ok) throw new Error(r.status);
      const d = await r.json();
      const keys = d.keys || [];
      if (keys.length === 0) {
        el.innerHTML = '<div class="empty-state">No API keys created yet.</div>';
        return;
      }
      let html = '<table class="data"><thead><tr><th>Name</th><th>Prefix</th><th>Created</th><th>Last Used</th><th>Status</th><th></th></tr></thead><tbody>';
      for (const k of keys) {
        const status = k.revoked
          ? '<span class="badge badge-cancel">Revoked</span>'
          : '<span class="badge badge-active">Active</span>';
        const revokeBtn = k.revoked ? '' : '<button class="btn-sm danger" onclick="window.__revokeKey(\'' + escHtml(k.id) + '\',\'' + escHtml(k.name) + '\')">Revoke</button>';
        html += '<tr><td>' + escHtml(k.name) + '</td><td><code>' + escHtml(k.prefix) + '...</code></td>'
          + '<td>' + fmtTimestamp(k.created_at) + '</td>'
          + '<td>' + fmtTimestamp(k.last_used_at) + '</td>'
          + '<td>' + status + '</td><td>' + revokeBtn + '</td></tr>';
      }
      html += '</tbody></table>';
      el.innerHTML = html;
    } catch(e) {
      el.innerHTML = '<div class="empty-state" style="color:#f87171;">Failed to load keys.</div>';
    }
  }

  window.__createKey = async function() {
    const nameInput = $('#new-key-name');
    const name = nameInput.value.trim();
    if (!name) { nameInput.focus(); return; }
    try {
      const r = await fetch(baseUrl() + '/admin/keys', {
        method: 'POST',
        headers: authHeaders(),
        body: JSON.stringify({ name }),
      });
      if (!r.ok) throw new Error(r.status);
      const d = await r.json();
      nameInput.value = '';
      $('#new-key-secret').innerHTML = '<div class="secret-box">'
        + '<button class="copy-btn" onclick="navigator.clipboard.writeText(\'' + escHtml(d.secret) + '\')">Copy</button>'
        + escHtml(d.secret)
        + '<span class="warn">&#9888; This secret will not be shown again. Copy it now.</span></div>';
      pollKeys();
    } catch(e) {
      alert('Failed to create key: ' + e.message);
    }
  };

  window.__revokeKey = async function(id, name) {
    if (!confirm('Revoke API key "' + name + '"? This cannot be undone.')) return;
    try {
      const r = await fetch(baseUrl() + '/admin/keys/' + encodeURIComponent(id), {
        method: 'DELETE',
        headers: authHeaders(),
      });
      if (!r.ok && r.status !== 204) throw new Error(r.status);
      pollKeys();
    } catch(e) {
      alert('Failed to revoke key: ' + e.message);
    }
  };

  async function pollAll() {
    await Promise.all([pollHealth(), pollWorkers(), pollStats(), pollKeys()]);
  }

  initConfig();
  pollAll();
  setInterval(pollAll, POLL_MS);
})();
    "#;

    let body_content = r#"
    <div class="config-bar">
      <label for="cfg-token">Admin Token:</label>
      <input id="cfg-token" type="password" placeholder="MODELRELAY_ADMIN_TOKEN">
      <label for="cfg-url">Server URL:</label>
      <input id="cfg-url" type="text" placeholder="(same origin)">
      <span id="cfg-status" class="status">—</span>
    </div>

    <div class="health-bar">
      <div class="health-item"><div class="label">Status</div><div id="h-status" class="value">—</div></div>
      <div class="health-item"><div class="label">Version</div><div id="h-version" class="value">—</div></div>
      <div class="health-item"><div class="label">Workers</div><div id="h-workers" class="value">—</div></div>
      <div class="health-item"><div class="label">Queue Depth</div><div id="h-queue" class="value">—</div></div>
      <div class="health-item"><div class="label">Uptime</div><div id="h-uptime" class="value">—</div></div>
    </div>

    <div class="panel">
      <h2><span class="dot" id="workers-dot"></span> Workers</h2>
      <div id="workers-body"><div class="empty-state">Loading...</div></div>
    </div>

    <div class="panel">
      <h2><span class="dot" id="stats-dot"></span> Request Stats</h2>
      <div id="stats-body"><div class="empty-state">Loading...</div></div>
    </div>

    <div class="panel">
      <h2>API Keys</h2>
      <div class="key-actions">
        <input id="new-key-name" type="text" placeholder="Key name (e.g. my-app)">
        <button class="btn-sm" onclick="window.__createKey()">Create Key</button>
      </div>
      <div id="new-key-secret"></div>
      <div id="keys-body"><div class="empty-state">Loading...</div></div>
    </div>
    "#;

    format!(
        r#"<!DOCTYPE html>
<html lang="en">
<head>
  <meta charset="utf-8">
  <meta name="viewport" content="width=device-width, initial-scale=1">
  <title>Dashboard — ModelRelay</title>
  <style>
    *, *::before, *::after {{ box-sizing: border-box; margin: 0; padding: 0; }}
    body {{
      font-family: -apple-system, BlinkMacSystemFont, "Segoe UI", Roboto, Helvetica, Arial, sans-serif;
      background: #0d1117; color: #e6edf3; line-height: 1.6;
    }}
    a {{ color: #7c3aed; text-decoration: none; }}
    a:hover {{ text-decoration: underline; }}
    .container {{ max-width: 960px; margin: 0 auto; padding: 0 24px; }}

    nav {{ padding: 20px 0; border-bottom: 1px solid #21262d; }}
    nav .container {{ display: flex; justify-content: space-between; align-items: center; }}
    .logo {{ font-size: 1.25rem; font-weight: 700; color: #e6edf3; }}
    .logo span {{ color: #7c3aed; }}
    .nav-links a {{ color: #8b949e; font-size: 0.9rem; margin-left: 16px; }}
    .nav-links a:hover {{ color: #e6edf3; }}

    .content {{ padding: 32px 0; }}
    .content h1 {{ font-size: 1.75rem; margin-bottom: 20px; }}

    footer {{ padding: 40px 0; border-top: 1px solid #21262d; text-align: center; color: #484f58; font-size: 0.85rem; }}
    footer a {{ color: #8b949e; }}

    code {{ font-family: "SFMono-Regular", Consolas, monospace; }}
    {dashboard_css}
  </style>
</head>
<body>
  <nav>
    <div class="container">
      <a href="/" class="logo">Model<span>Relay</span></a>
      <div class="nav-links">
        <a href="/dashboard">Dashboard</a>
        <a href="/setup">Setup</a>
      </div>
    </div>
  </nav>

  <section class="content">
    <div class="container">
      <div style="display:flex;justify-content:space-between;align-items:center;margin-bottom:20px;">
        <h1 style="margin-bottom:0;">Dashboard</h1>
        <a href="/setup" class="btn">+ Add a machine</a>
      </div>
      {body_content}
    </div>
  </section>

  <footer>
    <div class="container">
      &copy; 2026 ModelRelay &middot; <a href="https://github.com/ericflo/modelrelay">GitHub</a>
    </div>
  </footer>

  <script>{dashboard_js}</script>
</body>
</html>"#
    )
}

/// Optional configuration for embedding the setup wizard in a cloud context.
///
/// When provided, the wizard JS pre-fills the server URL, API key, and uses
/// a session-authenticated proxy endpoint for worker polling instead of
/// requiring a raw admin token.
pub struct CloudWizardConfig {
    /// The relay server URL to pre-fill (e.g. `https://api.modelrelay.io`).
    pub server_url: String,
    /// The user's API key to pre-fill for the test inference step.
    pub api_key: Option<String>,
    /// The proxy endpoint for polling worker status (e.g. `/dashboard/workers`).
    pub workers_poll_url: String,
}

/// Build the worker onboarding setup wizard page.
///
/// A 7-step guided flow for connecting a new worker machine to `ModelRelay`.
/// Always accessible — shown prominently when zero workers exist, and reachable
/// via the "Add a machine" button on the dashboard at any time.
#[must_use]
#[allow(clippy::too_many_lines)]
pub fn setup_wizard_page() -> String {
    setup_wizard_page_with_config(None)
}

/// Build the setup wizard page, optionally pre-configured for cloud users.
#[must_use]
#[allow(clippy::too_many_lines)]
pub fn setup_wizard_page_with_config(cloud_config: Option<&CloudWizardConfig>) -> String {
    use std::fmt::Write;

    let wizard_css = r"
    .wizard-progress {
      display:flex; gap:0; margin-bottom:32px; overflow-x:auto;
    }
    .wizard-progress .step-indicator {
      flex:1; text-align:center; padding:12px 4px; font-size:0.75rem;
      color:#484f58; border-bottom:3px solid #21262d; min-width:90px;
      transition: color 0.2s, border-color 0.2s;
    }
    .wizard-progress .step-indicator.active {
      color:#7c3aed; border-bottom-color:#7c3aed; font-weight:600;
    }
    .wizard-progress .step-indicator.done {
      color:#34d399; border-bottom-color:#34d399;
    }

    .wizard-step { display:none; animation:fadeIn 0.3s ease; }
    .wizard-step.active { display:block; }

    .wizard-card {
      background:#161b22; border:1px solid #21262d; border-radius:12px;
      padding:32px; margin-bottom:24px;
    }
    .wizard-card h2 { font-size:1.25rem; margin-bottom:16px; }
    .wizard-card p { color:#8b949e; margin-bottom:12px; line-height:1.7; }

    .platform-tabs {
      display:flex; gap:8px; margin-bottom:20px;
    }
    .platform-tabs .tab {
      padding:10px 20px; background:#0d1117; border:1px solid #30363d;
      border-radius:8px; color:#8b949e; cursor:pointer; font-size:0.9rem;
      font-weight:600; transition: all 0.2s;
    }
    .platform-tabs .tab:hover { border-color:#7c3aed; color:#e6edf3; }
    .platform-tabs .tab.active { background:#7c3aed; border-color:#7c3aed; color:#fff; }

    .platform-content { display:none; }
    .platform-content.active { display:block; }

    .code-block {
      background:#0d1117; border:1px solid #30363d; border-radius:8px;
      padding:16px; font-family:'SFMono-Regular',Consolas,monospace;
      font-size:0.85rem; color:#e6edf3; overflow-x:auto; position:relative;
      line-height:1.6; margin:12px 0;
    }
    .code-block .copy-btn {
      position:absolute; top:8px; right:8px; padding:4px 10px;
      font-size:0.75rem; background:#30363d; color:#e6edf3;
      border:none; border-radius:4px; cursor:pointer;
    }
    .code-block .copy-btn:hover { background:#484f58; }

    .wizard-nav {
      display:flex; justify-content:space-between; align-items:center;
      margin-top:24px; padding-top:24px; border-top:1px solid #21262d;
    }
    .wizard-nav .btn { min-width:120px; text-align:center; }
    .wizard-nav .btn-back {
      background:transparent; border:1px solid #30363d; color:#8b949e;
    }
    .wizard-nav .btn-back:hover { border-color:#7c3aed; color:#e6edf3; }

    .status-indicator {
      display:flex; align-items:center; gap:10px; padding:16px;
      background:#0d1117; border:1px solid #21262d; border-radius:8px;
      margin:16px 0; font-size:0.95rem;
    }
    .status-indicator .pulse {
      width:12px; height:12px; border-radius:50%; background:#484f58;
    }
    .status-indicator .pulse.searching {
      background:#fbbf24;
      animation: pulse 1.5s ease-in-out infinite;
    }
    .status-indicator .pulse.connected { background:#34d399; }
    @keyframes pulse {
      0%,100% { opacity:1; }
      50% { opacity:0.4; }
    }

    .check-mark { color:#34d399; font-weight:700; margin-right:4px; }
    .step-num {
      display:inline-flex; align-items:center; justify-content:center;
      width:28px; height:28px; border-radius:50%; background:#7c3aed;
      color:#fff; font-size:0.8rem; font-weight:700; margin-right:10px;
      flex-shrink:0;
    }

    .test-result {
      background:#0d1117; border:1px solid #21262d; border-radius:8px;
      padding:16px; margin-top:16px; font-family:'SFMono-Regular',Consolas,monospace;
      font-size:0.85rem; color:#e6edf3; max-height:300px; overflow-y:auto;
      white-space:pre-wrap;
    }

    .config-input {
      display:flex; gap:8px; align-items:center; flex-wrap:wrap; margin:12px 0;
    }
    .config-input label { color:#8b949e; font-size:0.85rem; min-width:120px; }
    .config-input input {
      padding:8px 12px; background:#0d1117; border:1px solid #30363d;
      border-radius:8px; color:#e6edf3; font-size:0.9rem; flex:1; min-width:200px;
    }
    .config-input input:focus { outline:none; border-color:#7c3aed; }

    @keyframes fadeIn { from { opacity:0; transform:translateY(8px); } to { opacity:1; transform:translateY(0); } }
    ";

    let wizard_js = r#"
(function() {
  const STEPS = 7;
  let currentStep = 1;
  let detectedPlatform = 'linux';
  let workerPollInterval = null;
  let initialWorkerIds = new Set();

  const $ = s => document.querySelector(s);
  const $$ = s => document.querySelectorAll(s);

  // Platform detection
  const ua = navigator.userAgent.toLowerCase();
  if (ua.includes('mac')) detectedPlatform = 'macos';
  else if (ua.includes('win')) detectedPlatform = 'windows';

  const cloudCfg = window.__mrCloudConfig || null;

  function getAdminToken() {
    if (cloudCfg) return ''; // cloud uses session auth via proxy
    return localStorage.getItem('mr_admin_token') || '';
  }
  function getServerUrl() {
    if (cloudCfg && cloudCfg.serverUrl) return cloudCfg.serverUrl;
    return localStorage.getItem('mr_server_url') || window.location.origin;
  }
  function getWorkersPollUrl() {
    if (cloudCfg && cloudCfg.workersPollUrl) return cloudCfg.workersPollUrl;
    return getServerUrl() + '/admin/workers';
  }
  function authHeaders() {
    const h = { 'Content-Type': 'application/json' };
    const t = getAdminToken();
    if (t) h['Authorization'] = 'Bearer ' + t;
    return h;
  }

  function escHtml(s) {
    const d = document.createElement('div');
    d.textContent = s;
    return d.innerHTML;
  }

  function goToStep(n) {
    if (n < 1 || n > STEPS) return;
    currentStep = n;
    $$('.wizard-step').forEach((el, i) => {
      el.classList.toggle('active', i + 1 === n);
    });
    $$('.step-indicator').forEach((el, i) => {
      el.classList.remove('active', 'done');
      if (i + 1 === n) el.classList.add('active');
      else if (i + 1 < n) el.classList.add('done');
    });
    // Start/stop worker polling on step 6
    if (n === 6) startWorkerPoll();
    else stopWorkerPoll();
  }

  function nextStep() { goToStep(currentStep + 1); }
  function prevStep() { goToStep(currentStep - 1); }
  window.__wizNext = nextStep;
  window.__wizPrev = prevStep;
  window.__wizGoTo = goToStep;

  // Platform tab switching
  window.__setPlatform = function(p) {
    detectedPlatform = p;
    $$('.tab').forEach(t => t.classList.toggle('active', t.dataset.platform === p));
    $$('.platform-content').forEach(el => el.classList.toggle('active', el.dataset.platform === p));
    updateDownloadLinks();
    updateConfigSnippet();
  };

  function updateDownloadLinks() {
    const base = 'https://github.com/ericflo/modelrelay/releases/latest/download';
    const binMap = {
      'macos': 'modelrelay-worker-darwin-arm64',
      'windows': 'modelrelay-worker-windows-amd64.exe',
      'linux': 'modelrelay-worker-linux-amd64',
    };
    const bin = binMap[detectedPlatform] || binMap['linux'];
    const el = $('#download-cmd');
    if (el) {
      if (detectedPlatform === 'windows') {
        el.textContent = 'curl -L -o modelrelay-worker.exe ' + base + '/' + bin;
      } else {
        el.textContent = 'curl -L -o modelrelay-worker ' + base + '/' + bin + ' && chmod +x modelrelay-worker';
      }
    }
  }

  function updateConfigSnippet() {
    const serverUrl = $('#cfg-server-url') ? $('#cfg-server-url').value : getServerUrl();
    const secret = $('#cfg-worker-secret') ? $('#cfg-worker-secret').value : 'your-worker-secret';
    const el = $('#config-toml');
    if (el) {
      el.textContent =
        '[server]\n' +
        'url = "' + serverUrl + '"\n' +
        'worker_secret = "' + secret + '"\n\n' +
        '[worker]\n' +
        'name = "my-gpu-box"\n\n' +
        '[[backends]]\n' +
        'name = "lmstudio"\n' +
        'url = "http://localhost:1234"\n' +
        'models = ["*"]';
    }
  }
  window.__updateConfig = updateConfigSnippet;

  // Step 6: poll for new worker
  async function snapshotWorkers() {
    try {
      const pollUrl = getWorkersPollUrl();
      const opts = cloudCfg ? { credentials: 'same-origin' } : { headers: authHeaders() };
      const r = await fetch(pollUrl, opts);
      if (!r.ok) return;
      const d = await r.json();
      (d.workers || []).forEach(w => initialWorkerIds.add(w.worker_id));
    } catch(e) {}
  }

  function startWorkerPoll() {
    stopWorkerPoll();
    snapshotWorkers();
    const indicator = $('#worker-status');
    const pulse = $('#worker-pulse');
    const statusText = $('#worker-status-text');
    if (pulse) { pulse.className = 'pulse searching'; }
    if (statusText) { statusText.textContent = 'Waiting for worker to connect...'; }

    workerPollInterval = setInterval(async () => {
      try {
        const pollUrl = getWorkersPollUrl();
        const opts = cloudCfg ? { credentials: 'same-origin' } : { headers: authHeaders() };
        const r = await fetch(pollUrl, opts);
        if (!r.ok) return;
        const d = await r.json();
        const workers = d.workers || [];
        const newWorker = workers.find(w => !initialWorkerIds.has(w.worker_id));
        if (newWorker) {
          stopWorkerPoll();
          if (pulse) { pulse.className = 'pulse connected'; }
          if (statusText) {
            const name = newWorker.worker_name || newWorker.worker_id;
            const models = (newWorker.models || []).join(', ');
            statusText.innerHTML = '<span class="check-mark">&#10003;</span> Worker <strong>' + escHtml(name) + '</strong> connected!' +
              (models ? ' <span style="color:#8b949e;">(' + escHtml(models) + ')</span>' : '');
          }
          // Enable next button
          const nextBtn = $('#step6-next');
          if (nextBtn) { nextBtn.disabled = false; nextBtn.style.opacity = '1'; }
        }
      } catch(e) {}
    }, 3000);
  }

  function stopWorkerPoll() {
    if (workerPollInterval) {
      clearInterval(workerPollInterval);
      workerPollInterval = null;
    }
  }

  // Step 7: test inference
  window.__testInference = async function() {
    const resultEl = $('#test-result');
    const btnEl = $('#test-btn');
    if (!resultEl || !btnEl) return;
    btnEl.disabled = true;
    btnEl.textContent = 'Sending...';
    resultEl.textContent = 'Sending request...';
    resultEl.style.display = 'block';

    const serverUrl = getServerUrl();
    const model = $('#test-model') ? $('#test-model').value.trim() : 'default';
    const body = {
      model: model || 'default',
      messages: [{ role: 'user', content: 'Hello! Reply in one short sentence.' }],
      max_tokens: 100,
    };

    try {
      const apiKey = (cloudCfg && cloudCfg.apiKey) ? cloudCfg.apiKey : (localStorage.getItem('mr_test_api_key') || '');
      const headers = { 'Content-Type': 'application/json' };
      if (apiKey) headers['Authorization'] = 'Bearer ' + apiKey;
      const r = await fetch(serverUrl + '/v1/chat/completions', {
        method: 'POST',
        headers,
        body: JSON.stringify(body),
      });
      const text = await r.text();
      if (r.ok) {
        try {
          const d = JSON.parse(text);
          const reply = d.choices && d.choices[0] && d.choices[0].message
            ? d.choices[0].message.content
            : text;
          resultEl.innerHTML = '<span class="check-mark">&#10003;</span> <strong>Success!</strong>\n\n'
            + 'Model: ' + escHtml(d.model || model) + '\n'
            + 'Response: ' + escHtml(reply);
        } catch(e) {
          resultEl.textContent = 'Response (raw):\n' + text;
        }
      } else {
        resultEl.textContent = 'Error ' + r.status + ':\n' + text;
      }
    } catch(e) {
      resultEl.textContent = 'Connection failed: ' + e.message;
    }
    btnEl.disabled = false;
    btnEl.textContent = 'Send Test Request';
  };

  window.__copyCode = function(id) {
    const el = document.getElementById(id);
    if (el) navigator.clipboard.writeText(el.textContent);
  };

  // Init
  goToStep(1);
  window.__setPlatform(detectedPlatform);

  // Pre-fill config inputs from cloud config
  if (cloudCfg) {
    const urlInput = $('#cfg-server-url');
    if (urlInput && cloudCfg.serverUrl) { urlInput.value = cloudCfg.serverUrl; }
    const apiKeyInput = $('#test-api-key');
    if (apiKeyInput && cloudCfg.apiKey) { apiKeyInput.value = cloudCfg.apiKey; }
  }

  // Reconfigure when config inputs change
  document.addEventListener('input', (e) => {
    if (e.target.id === 'cfg-server-url' || e.target.id === 'cfg-worker-secret') {
      updateConfigSnippet();
    }
  });
})();
    "#;

    let step_labels = [
        "Platform",
        "Install LM Studio",
        "Load Model",
        "Download Worker",
        "Configure",
        "Connect",
        "Test",
    ];

    let mut progress_html = String::from("<div class=\"wizard-progress\">");
    for (i, label) in step_labels.iter().enumerate() {
        let cls = if i == 0 { " active" } else { "" };
        let _ = write!(
            progress_html,
            "<div class=\"step-indicator{cls}\">{label}</div>"
        );
    }
    progress_html.push_str("</div>");

    let steps_html = r#"
    <!-- Step 1: Platform Detection -->
    <div class="wizard-step active" data-step="1">
      <div class="wizard-card">
        <h2><span class="step-num">1</span> Choose your platform</h2>
        <p>ModelRelay workers run on your machine alongside your local model server. Select your operating system to get started.</p>
        <div class="platform-tabs">
          <div class="tab" data-platform="macos" onclick="window.__setPlatform('macos')">macOS</div>
          <div class="tab" data-platform="windows" onclick="window.__setPlatform('windows')">Windows</div>
          <div class="tab" data-platform="linux" onclick="window.__setPlatform('linux')">Linux</div>
        </div>
        <div class="platform-content" data-platform="macos">
          <p>Great — macOS with Apple Silicon is an excellent choice for running local models. You'll need a Mac with an M-series chip for best performance.</p>
        </div>
        <div class="platform-content" data-platform="windows">
          <p>Windows works well for local inference. You'll want a machine with an NVIDIA GPU for best performance.</p>
        </div>
        <div class="platform-content" data-platform="linux">
          <p>Linux is the most common choice for GPU inference servers. Works great with NVIDIA GPUs and CUDA.</p>
        </div>
      </div>
      <div class="wizard-nav">
        <div></div>
        <button class="btn" onclick="window.__wizNext()">Next &rarr;</button>
      </div>
    </div>

    <!-- Step 2: Install LM Studio -->
    <div class="wizard-step" data-step="2">
      <div class="wizard-card">
        <h2><span class="step-num">2</span> Install LM Studio</h2>
        <p>LM Studio is a free desktop app for running local LLMs. It provides an OpenAI-compatible API server that ModelRelay workers connect to.</p>
        <p style="margin:20px 0;">
          <a href="https://lmstudio.ai" target="_blank" class="btn">Download LM Studio &rarr;</a>
        </p>
        <div class="platform-content active" data-platform="macos">
          <p>Download the macOS DMG, drag LM Studio to your Applications folder, and launch it.</p>
        </div>
        <div class="platform-content" data-platform="windows">
          <p>Download the Windows installer (.exe), run it, and launch LM Studio from the Start menu.</p>
        </div>
        <div class="platform-content" data-platform="linux">
          <p>Download the Linux AppImage, make it executable with <code>chmod +x</code>, and run it. Alternatively, if you prefer a headless setup, you can use <code>llama-server</code> or any OpenAI-compatible local server instead of LM Studio.</p>
        </div>
        <p style="color:#8b949e;font-size:0.85rem;margin-top:16px;">Already have LM Studio or another local model server? Skip ahead.</p>
      </div>
      <div class="wizard-nav">
        <button class="btn btn-back" onclick="window.__wizPrev()">&larr; Back</button>
        <button class="btn" onclick="window.__wizNext()">Next &rarr;</button>
      </div>
    </div>

    <!-- Step 3: Configure a Model -->
    <div class="wizard-step" data-step="3">
      <div class="wizard-card">
        <h2><span class="step-num">3</span> Download and load a model</h2>
        <p>In LM Studio:</p>
        <ol style="color:#8b949e;margin:12px 0 12px 20px;line-height:2;">
          <li>Open the <strong style="color:#e6edf3;">Discover</strong> tab and search for a model (e.g. <code>llama-3.2-3b</code>)</li>
          <li>Click <strong style="color:#e6edf3;">Download</strong> and wait for it to complete</li>
          <li>Go to the <strong style="color:#e6edf3;">Developer</strong> tab</li>
          <li>Select your downloaded model and click <strong style="color:#e6edf3;">Start Server</strong></li>
          <li>Confirm the server is running on <code>http://localhost:1234</code></li>
        </ol>
        <p style="color:#8b949e;font-size:0.85rem;">Using llama-server or another backend? Just make sure it's running and note the URL and port.</p>
      </div>
      <div class="wizard-nav">
        <button class="btn btn-back" onclick="window.__wizPrev()">&larr; Back</button>
        <button class="btn" onclick="window.__wizNext()">Next &rarr;</button>
      </div>
    </div>

    <!-- Step 4: Download Worker -->
    <div class="wizard-step" data-step="4">
      <div class="wizard-card">
        <h2><span class="step-num">4</span> Download the ModelRelay worker</h2>
        <p>Download the <code>modelrelay-worker</code> binary for your platform:</p>
        <div class="code-block">
          <button class="copy-btn" onclick="window.__copyCode('download-cmd')">Copy</button>
          <code id="download-cmd">curl -L -o modelrelay-worker https://github.com/ericflo/modelrelay/releases/latest/download/modelrelay-worker-linux-amd64 &amp;&amp; chmod +x modelrelay-worker</code>
        </div>
        <p style="color:#8b949e;font-size:0.85rem;margin-top:12px;">
          Or download directly from
          <a href="https://github.com/ericflo/modelrelay/releases/latest" target="_blank">GitHub Releases</a>.
        </p>
      </div>
      <div class="wizard-nav">
        <button class="btn btn-back" onclick="window.__wizPrev()">&larr; Back</button>
        <button class="btn" onclick="window.__wizNext()">Next &rarr;</button>
      </div>
    </div>

    <!-- Step 5: Configure Worker -->
    <div class="wizard-step" data-step="5">
      <div class="wizard-card">
        <h2><span class="step-num">5</span> Configure the worker</h2>
        <p>Create a <code>config.toml</code> file next to the worker binary. Adjust the values below for your setup:</p>
        <div class="config-input">
          <label for="cfg-server-url">Server URL:</label>
          <input id="cfg-server-url" type="text" placeholder="https://your-server.example.com">
        </div>
        <div class="config-input">
          <label for="cfg-worker-secret">Worker Secret:</label>
          <input id="cfg-worker-secret" type="text" placeholder="your-worker-secret">
        </div>
        <div class="code-block">
          <button class="copy-btn" onclick="window.__copyCode('config-toml')">Copy</button>
          <code id="config-toml">[server]
url = ""
worker_secret = "your-worker-secret"

[worker]
name = "my-gpu-box"

[[backends]]
name = "lmstudio"
url = "http://localhost:1234"
models = ["*"]</code>
        </div>
        <p style="color:#8b949e;font-size:0.85rem;margin-top:12px;">
          The <code>worker_secret</code> must match the <code>MODELRELAY_WORKER_SECRET</code> configured on your server.
          <code>models = ["*"]</code> advertises all models from your backend.
        </p>
      </div>
      <div class="wizard-nav">
        <button class="btn btn-back" onclick="window.__wizPrev()">&larr; Back</button>
        <button class="btn" onclick="window.__wizNext()">Next &rarr;</button>
      </div>
    </div>

    <!-- Step 6: Start Worker & Detect Connection -->
    <div class="wizard-step" data-step="6">
      <div class="wizard-card">
        <h2><span class="step-num">6</span> Start the worker</h2>
        <p>Run the worker from the directory containing your <code>config.toml</code>:</p>
        <div class="code-block">
          <button class="copy-btn" onclick="window.__copyCode('run-cmd')">Copy</button>
          <code id="run-cmd">./modelrelay-worker --config config.toml</code>
        </div>
        <p style="margin-top:16px;">Once started, the worker will connect to your ModelRelay server. We'll detect it automatically:</p>
        <div class="status-indicator" id="worker-status">
          <div class="pulse" id="worker-pulse"></div>
          <span id="worker-status-text">Waiting for worker to connect...</span>
        </div>
        <p style="color:#8b949e;font-size:0.85rem;margin-top:12px;">
          Make sure your admin token is set on the
          <a href="/dashboard">dashboard</a> so we can detect the connection. Polling every 3 seconds.
        </p>
      </div>
      <div class="wizard-nav">
        <button class="btn btn-back" onclick="window.__wizPrev()">&larr; Back</button>
        <button class="btn" id="step6-next" onclick="window.__wizNext()">Next &rarr;</button>
      </div>
    </div>

    <!-- Step 7: Test Inference -->
    <div class="wizard-step" data-step="7">
      <div class="wizard-card">
        <h2><span class="step-num">7</span> Test inference</h2>
        <p>Send a test request through ModelRelay to verify everything works end-to-end.</p>
        <div class="config-input">
          <label for="test-model">Model name:</label>
          <input id="test-model" type="text" placeholder="e.g. llama-3.2-3b-instruct" value="">
        </div>
        <p style="margin:16px 0;">
          <button class="btn" id="test-btn" onclick="window.__testInference()">Send Test Request</button>
        </p>
        <div id="test-result" class="test-result" style="display:none;"></div>
        <p style="color:#8b949e;font-size:0.85rem;margin-top:16px;">
          You can also test from the command line:
        </p>
        <div class="code-block">
          <button class="copy-btn" onclick="window.__copyCode('curl-test')">Copy</button>
          <code id="curl-test">curl -X POST http://localhost:8080/v1/chat/completions \
  -H "Content-Type: application/json" \
  -d '{"model":"your-model","messages":[{"role":"user","content":"Hello!"}],"max_tokens":100}'</code>
        </div>
      </div>
      <div class="wizard-card" style="text-align:center;">
        <h2 style="color:#34d399;">&#127881; Setup complete!</h2>
        <p>Your worker is connected and serving inference requests through ModelRelay.</p>
        <p style="margin-top:16px;">
          <a href="/dashboard" class="btn">Go to Dashboard</a>
          <a href="/setup" class="btn btn-back" style="margin-left:8px;" onclick="event.preventDefault();window.__wizGoTo(1);">Add another machine</a>
        </p>
      </div>
    </div>
    "#;

    format!(
        r#"<!DOCTYPE html>
<html lang="en">
<head>
  <meta charset="utf-8">
  <meta name="viewport" content="width=device-width, initial-scale=1">
  <title>Setup — ModelRelay</title>
  <style>
    *, *::before, *::after {{ box-sizing: border-box; margin: 0; padding: 0; }}
    body {{
      font-family: -apple-system, BlinkMacSystemFont, "Segoe UI", Roboto, Helvetica, Arial, sans-serif;
      background: #0d1117; color: #e6edf3; line-height: 1.6;
    }}
    a {{ color: #7c3aed; text-decoration: none; }}
    a:hover {{ text-decoration: underline; }}
    .container {{ max-width: 720px; margin: 0 auto; padding: 0 24px; }}

    nav {{ padding: 20px 0; border-bottom: 1px solid #21262d; }}
    nav .container {{ display: flex; justify-content: space-between; align-items: center; max-width: 960px; }}
    .logo {{ font-size: 1.25rem; font-weight: 700; color: #e6edf3; }}
    .logo span {{ color: #7c3aed; }}
    .nav-links a {{ color: #8b949e; font-size: 0.9rem; margin-left: 16px; }}
    .nav-links a:hover {{ color: #e6edf3; }}

    .content {{ padding: 32px 0; }}
    .content h1 {{ font-size: 1.75rem; margin-bottom: 8px; }}
    .subtitle {{ color: #8b949e; margin-bottom: 24px; }}

    footer {{ padding: 40px 0; border-top: 1px solid #21262d; text-align: center; color: #484f58; font-size: 0.85rem; }}
    footer a {{ color: #8b949e; }}

    code {{ font-family: "SFMono-Regular", Consolas, monospace; }}

    .btn {{
      display: inline-block; padding: 10px 20px; background: #7c3aed; color: #fff;
      border: none; border-radius: 8px; font-size: 0.9rem; font-weight: 600;
      cursor: pointer; text-decoration: none;
    }}
    .btn:hover {{ background: #6d28d9; text-decoration: none; }}
    {wizard_css}
  </style>
</head>
<body>
  <nav>
    <div class="container" style="max-width:960px;">
      <a href="/" class="logo">Model<span>Relay</span></a>
      <div class="nav-links">
        <a href="/dashboard">Dashboard</a>
        <a href="/setup">Setup</a>
      </div>
    </div>
  </nav>

  <section class="content">
    <div class="container">
      <h1>Connect a Worker Machine</h1>
      <p class="subtitle">Follow these steps to connect a GPU machine to your ModelRelay deployment.</p>
      {progress_html}
      {steps_html}
    </div>
  </section>

  <footer>
    <div class="container">
      &copy; 2026 ModelRelay &middot; <a href="https://github.com/ericflo/modelrelay">GitHub</a>
    </div>
  </footer>

  {cloud_config_script}
  <script>{wizard_js}</script>
</body>
</html>"#,
        cloud_config_script = cloud_config_script(cloud_config),
    )
}

fn cloud_config_script(config: Option<&CloudWizardConfig>) -> String {
    let Some(cfg) = config else {
        return String::new();
    };
    let api_key_js = match &cfg.api_key {
        Some(k) => {
            // Escape any quotes/backslashes in the key for safe JS embedding
            let escaped = k.replace('\\', "\\\\").replace('"', "\\\"");
            format!("\"{escaped}\"")
        }
        None => "null".to_string(),
    };
    let server_url = cfg.server_url.replace('\\', "\\\\").replace('"', "\\\"");
    let poll_url = cfg
        .workers_poll_url
        .replace('\\', "\\\\")
        .replace('"', "\\\"");
    format!(
        "<script>window.__mrCloudConfig = {{ serverUrl: \"{server_url}\", apiKey: {api_key_js}, workersPollUrl: \"{poll_url}\" }};</script>"
    )
}

/// Shared HTML page shell used by the admin dashboard and commercial cloud routes.
///
/// `logged_in` controls the nav links: when `true`, shows Dashboard + Pricing + Log out;
/// when `false`, shows Dashboard + Pricing only (login/signup are reached via their own pages).
#[must_use]
#[allow(clippy::too_many_lines)]
pub fn page_shell(title: &str, body_content: &str, logged_in: bool) -> String {
    let nav_links = if logged_in {
        r#"<a href="/dashboard">Dashboard</a>
        <a href="/pricing">Pricing</a>
        <form method="POST" action="/logout"><button type="submit">Log out</button></form>"#
    } else {
        r#"<a href="/dashboard">Dashboard</a>
        <a href="/pricing">Pricing</a>"#
    };

    format!(
        r#"<!DOCTYPE html>
<html lang="en">
<head>
  <meta charset="utf-8">
  <meta name="viewport" content="width=device-width, initial-scale=1">
  <title>{title} — ModelRelay</title>
  <style>
    *, *::before, *::after {{ box-sizing: border-box; margin: 0; padding: 0; }}
    body {{
      font-family: -apple-system, BlinkMacSystemFont, "Segoe UI", Roboto, Helvetica, Arial, sans-serif;
      background: #0d1117; color: #e6edf3; line-height: 1.6;
    }}
    a {{ color: #7c3aed; text-decoration: none; }}
    a:hover {{ text-decoration: underline; }}
    .container {{ max-width: 900px; margin: 0 auto; padding: 0 24px; }}

    nav {{ padding: 20px 0; border-bottom: 1px solid #21262d; }}
    nav .container {{ display: flex; justify-content: space-between; align-items: center; }}
    .logo {{ font-size: 1.25rem; font-weight: 700; color: #e6edf3; }}
    .logo span {{ color: #7c3aed; }}
    .nav-links a {{ color: #8b949e; font-size: 0.9rem; margin-left: 16px; }}
    .nav-links a:hover {{ color: #e6edf3; }}
    .nav-links form {{ display: inline; }}
    .nav-links button {{ background: none; border: none; color: #8b949e; font-size: 0.9rem; cursor: pointer; margin-left: 16px; font-family: inherit; }}
    .nav-links button:hover {{ color: #e6edf3; }}

    .content {{ padding: 60px 0; }}
    .content h1 {{ font-size: 2rem; margin-bottom: 24px; }}

    .card {{
      background: #161b22; border: 1px solid #21262d; border-radius: 12px;
      padding: 32px; margin-bottom: 24px;
    }}
    .card h2 {{ font-size: 1.2rem; margin-bottom: 12px; color: #e6edf3; }}
    .card p {{ color: #8b949e; }}

    .auth-form .form-group {{ margin-bottom: 16px; }}
    .auth-form label {{ display: block; font-size: 0.9rem; color: #8b949e; margin-bottom: 4px; }}
    .auth-form input {{
      width: 100%; padding: 10px 12px; background: #0d1117; border: 1px solid #30363d;
      border-radius: 8px; color: #e6edf3; font-size: 0.95rem;
    }}
    .auth-form input:focus {{ outline: none; border-color: #7c3aed; }}

    .badge {{
      display: inline-block; padding: 4px 12px; border-radius: 20px;
      font-size: 0.8rem; font-weight: 600; background: #1f2937; color: #8b949e;
    }}
    .badge-active {{ background: #064e3b; color: #34d399; }}
    .badge-warn {{ background: #78350f; color: #fbbf24; }}
    .badge-cancel {{ background: #7f1d1d; color: #f87171; }}

    .info-table {{ margin-top: 16px; width: 100%; border-collapse: collapse; }}
    .info-table td {{ padding: 8px 0; border-bottom: 1px solid #21262d; color: #8b949e; }}
    .info-table td:first-child {{ font-weight: 600; color: #e6edf3; width: 140px; }}

    .key-display {{
      margin-top: 12px; padding: 12px 16px; background: #0d1117;
      border: 1px solid #21262d; border-radius: 8px; font-family: monospace;
      color: #7c3aed; word-break: break-all;
    }}

    .btn {{
      display: inline-block; padding: 10px 20px; background: #7c3aed; color: #fff;
      border: none; border-radius: 8px; font-size: 0.9rem; font-weight: 600;
      cursor: pointer; text-decoration: none;
    }}
    .btn:hover {{ background: #6d28d9; text-decoration: none; }}

    .auth-switch {{ margin-top: 16px; text-align: center; color: #8b949e; font-size: 0.9rem; }}

    .error-msg {{
      background: #3b1219; border: 1px solid #7f1d1d; border-radius: 8px;
      padding: 10px 14px; margin-bottom: 16px; color: #f87171; font-size: 0.9rem;
    }}

    footer {{ padding: 40px 0; border-top: 1px solid #21262d; text-align: center; color: #484f58; font-size: 0.85rem; }}
    footer a {{ color: #8b949e; }}
  </style>
</head>
<body>
  <nav>
    <div class="container">
      <a href="/" class="logo">Model<span>Relay</span></a>
      <div class="nav-links">
        {nav_links}
      </div>
    </div>
  </nav>

  <section class="content">
    <div class="container">
      <h1>{title}</h1>
      {body_content}
    </div>
  </section>

  <footer>
    <div class="container">
      &copy; 2026 ModelRelay &middot; <a href="https://github.com/ericflo/modelrelay">GitHub</a>
    </div>
  </footer>
</body>
</html>"#
    )
}
