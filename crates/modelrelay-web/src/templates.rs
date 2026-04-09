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
        const models = (w.models||[]).map(m => '<span class="model-tag">' + (m === '*' ? 'All models' : escHtml(m)) + '</span>').join('');
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
  <link rel="icon" href="data:image/svg+xml,<svg xmlns='http://www.w3.org/2000/svg' viewBox='0 0 100 100'><rect width='100' height='100' rx='20' fill='%237c3aed'/><text x='50' y='72' font-size='60' font-weight='bold' text-anchor='middle' fill='white'>M</text></svg>">
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
        <a href="/integrate">Integrate</a>
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
    /// The user's worker secret (same as API key for hosted mode).
    pub worker_secret: Option<String>,
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
      transition: color 0.2s, border-color 0.2s; cursor:pointer;
    }
    .wizard-progress .step-indicator:hover {
      color:#c9d1d9;
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

    .backend-content { display:none; }
    .backend-content.active { display:block; }

    .hint-box {
      background:#1c1f26; border:1px solid #30363d; border-radius:8px;
      padding:14px 16px; margin:12px 0; font-size:0.85rem; color:#8b949e;
    }
    .hint-box strong { color:#e6edf3; }

    .skip-link {
      display:none; margin-top:8px; color:#7c3aed; cursor:pointer;
      font-size:0.85rem; background:none; border:none; font-family:inherit;
    }
    .skip-link:hover { text-decoration:underline; }

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
  const STEPS = 8;
  let currentStep = 1;
  let detectedPlatform = 'linux';
  let selectedBackend = 'lmstudio';
  let workerPollInterval = null;
  let troubleshootTimer = null;
  let initialWorkerIds = new Set();
  let detectedModels = [];

  const $ = s => document.querySelector(s);
  const $$ = s => document.querySelectorAll(s);

  const ua = navigator.userAgent.toLowerCase();
  if (ua.includes('mac')) detectedPlatform = 'macos';
  else if (ua.includes('win')) detectedPlatform = 'windows';

  const cloudCfg = window.__mrCloudConfig || null;

  function getAdminToken() {
    if (cloudCfg) return '';
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
    if (n === 6) startWorkerPoll();
    else stopWorkerPoll();
  }

  function nextStep() { goToStep(currentStep + 1); }
  function prevStep() { goToStep(currentStep - 1); }
  window.__wizNext = nextStep;
  window.__wizPrev = prevStep;
  window.__wizGoTo = goToStep;

  // Platform tab switching (steps 1, 4)
  window.__setPlatform = function(p) {
    detectedPlatform = p;
    $$('.platform-tabs:not(.backend-tabs):not(.persist-tabs) .tab').forEach(t =>
      t.classList.toggle('active', t.dataset.platform === p));
    $$('.platform-content').forEach(el =>
      el.classList.toggle('active', el.dataset.platform === p));
    updateDownloadLinks();
    updateConfigSnippet();
    window.__setPersistPlatform(p);
  };

  // Backend tab switching (steps 2, 3)
  window.__setBackend = function(b) {
    selectedBackend = b;
    $$('.backend-tabs .tab').forEach(t =>
      t.classList.toggle('active', t.dataset.backend === b));
    $$('.backend-content').forEach(el =>
      el.classList.toggle('active', el.dataset.backend === b));
    updateConfigSnippet();
  };

  // Persist platform tabs (step 8)
  window.__setPersistPlatform = function(p) {
    $$('.persist-content').forEach(el =>
      el.style.display = el.dataset.platform === p ? 'block' : 'none');
    $$('.persist-tabs .tab').forEach(t =>
      t.classList.toggle('active', t.dataset.platform === p));
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

  function getBackendPort() {
    return selectedBackend === 'lmstudio' ? '1234' : '8000';
  }

  function updateConfigSnippet() {
    const serverUrl = $('#cfg-server-url') ? $('#cfg-server-url').value : getServerUrl();
    const secret = $('#cfg-worker-secret') ? $('#cfg-worker-secret').value : 'your-worker-secret';
    const workerName = $('#cfg-worker-name') ? ($('#cfg-worker-name').value || 'my-gpu-box') : 'my-gpu-box';
    const port = getBackendPort();
    const el = $('#config-toml');
    if (el) {
      el.textContent =
        'proxy_url = "' + serverUrl + '"\n' +
        'worker_secret = "' + secret + '"\n' +
        'worker_name = "' + workerName + '"\n' +
        'backend_url = "http://localhost:' + port + '"\n' +
        'models = ["*"]';
    }
    // Also update env var snippet
    const envEl = $('#config-env');
    if (envEl) {
      envEl.textContent =
        'export PROXY_URL="' + serverUrl + '"\n' +
        'export WORKER_SECRET="' + secret + '"\n' +
        'export WORKER_NAME="' + workerName + '"\n' +
        'export BACKEND_URL="http://localhost:' + port + '"\n' +
        'export MODELS="*"';
    }
    // Update curl test command
    const curlEl = $('#curl-test');
    if (curlEl) {
      const apiKeyInput = $('#test-api-key');
      const apiKey = (cloudCfg && cloudCfg.apiKey) ? cloudCfg.apiKey : (apiKeyInput ? apiKeyInput.value.trim() : '') || (localStorage.getItem('mr_test_api_key') || '');
      const testModel = ($('#test-model') && $('#test-model').value.trim()) || 'your-model';
      let curlCmd = 'curl -X POST ' + serverUrl + '/v1/chat/completions \\\n' +
        '  -H "Content-Type: application/json" \\\n';
      if (apiKey) curlCmd += '  -H "Authorization: Bearer ' + apiKey + '" \\\n';
      curlCmd += '  -d \'{"model":"' + testModel + '","messages":[{"role":"user","content":"Hello!"}],"max_tokens":100}\'';
      curlEl.textContent = curlCmd;
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
    const pulse = $('#worker-pulse');
    const statusText = $('#worker-status-text');
    const troubleshoot = $('#troubleshoot-hints');
    const skipBtn = $('#skip-detect');
    if (pulse) pulse.className = 'pulse searching';
    if (statusText) statusText.textContent = 'Waiting for worker to connect...';
    if (troubleshoot) troubleshoot.style.display = 'none';
    if (skipBtn) skipBtn.style.display = 'none';

    // Show troubleshooting after 15s, skip button after 30s
    let elapsed = 0;
    troubleshootTimer = setInterval(() => {
      elapsed += 1;
      if (elapsed >= 15 && troubleshoot) troubleshoot.style.display = 'block';
      if (elapsed >= 30 && skipBtn) skipBtn.style.display = 'inline-block';
    }, 1000);

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
          if (pulse) pulse.className = 'pulse connected';
          if (statusText) {
            const name = newWorker.worker_name || newWorker.worker_id;
            const models = (newWorker.models || []).join(', ');
            detectedModels = newWorker.models || [];
            const modelsDisplay = models === '*' ? 'all models' : models;
            statusText.innerHTML = '<span class="check-mark">&#10003;</span> Worker <strong>' + escHtml(name) + '</strong> connected!' +
              (modelsDisplay ? ' <span style="color:#8b949e;">(' + escHtml(modelsDisplay) + ')</span>' : '');
          }
          if (troubleshoot) troubleshoot.style.display = 'none';
          if (skipBtn) skipBtn.style.display = 'none';
          const nextBtn = $('#step6-next');
          if (nextBtn) { nextBtn.disabled = false; nextBtn.style.opacity = '1'; }
          // Pre-fill test model from detected models
          const testModel = $('#test-model');
          if (testModel && detectedModels.length > 0 && !testModel.value) {
            testModel.value = detectedModels[0];
          }
        }
      } catch(e) {}
    }, 3000);
  }

  function stopWorkerPoll() {
    if (workerPollInterval) { clearInterval(workerPollInterval); workerPollInterval = null; }
    if (troubleshootTimer) { clearInterval(troubleshootTimer); troubleshootTimer = null; }
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
      const apiKeyInput = $('#test-api-key');
      let apiKey = (cloudCfg && cloudCfg.apiKey) ? cloudCfg.apiKey : '';
      if (!apiKey && apiKeyInput) apiKey = apiKeyInput.value.trim();
      if (!apiKey) apiKey = localStorage.getItem('mr_test_api_key') || '';
      if (apiKey && apiKeyInput) localStorage.setItem('mr_test_api_key', apiKey);
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
            ? d.choices[0].message.content : text;
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
  window.__setBackend('lmstudio');
  window.__setPersistPlatform(detectedPlatform);

  // Pre-fill config inputs
  if (cloudCfg) {
    const urlInput = $('#cfg-server-url');
    if (urlInput && cloudCfg.serverUrl) urlInput.value = cloudCfg.serverUrl;
    const secretInput = $('#cfg-worker-secret');
    if (secretInput && cloudCfg.workerSecret) secretInput.value = cloudCfg.workerSecret;
    const apiKeyInput = $('#test-api-key');
    if (apiKeyInput && cloudCfg.apiKey) apiKeyInput.value = cloudCfg.apiKey;
    updateConfigSnippet();
  } else {
    const urlInput = $('#cfg-server-url');
    if (urlInput && !urlInput.value) urlInput.value = window.location.origin;
    const savedApiKey = localStorage.getItem('mr_test_api_key');
    const apiKeyInput = $('#test-api-key');
    if (apiKeyInput && savedApiKey && !apiKeyInput.value) apiKeyInput.value = savedApiKey;
  }

  document.addEventListener('input', (e) => {
    if (e.target.id === 'cfg-server-url' || e.target.id === 'cfg-worker-secret' || e.target.id === 'cfg-worker-name' || e.target.id === 'test-api-key' || e.target.id === 'test-model') {
      updateConfigSnippet();
    }
  });
})();
    "#;

    let step_labels = [
        "Platform",
        "Backend",
        "Model",
        "Download",
        "Configure",
        "Connect",
        "Test",
        "Persist",
    ];

    let mut progress_html = String::from("<div class=\"wizard-progress\">");
    for (i, label) in step_labels.iter().enumerate() {
        let cls = if i == 0 { " active" } else { "" };
        let step_num = i + 1;
        let _ = write!(
            progress_html,
            "<div class=\"step-indicator{cls}\" onclick=\"window.__wizGoTo({step_num})\">{label}</div>"
        );
    }
    progress_html.push_str("</div>");

    let steps_html = r##"
    <!-- Step 1: Platform -->
    <div class="wizard-step active" data-step="1">
      <div class="wizard-card">
        <h2><span class="step-num">1</span> Choose your platform</h2>
        <p>Select the OS where you'll run inference. We'll tailor the next steps accordingly.</p>
        <div class="platform-tabs">
          <div class="tab" data-platform="macos" onclick="window.__setPlatform('macos')">macOS</div>
          <div class="tab" data-platform="windows" onclick="window.__setPlatform('windows')">Windows</div>
          <div class="tab" data-platform="linux" onclick="window.__setPlatform('linux')">Linux</div>
        </div>
        <div class="platform-content" data-platform="macos">
          <div class="hint-box">
            <strong>Apple Silicon (M1/M2/M3/M4)</strong> — best experience. Models run on the unified GPU with no driver setup.<br>
            <strong>Intel Macs</strong> — CPU-only inference works but is much slower.<br><br>
            <strong>Check:</strong> 16 GB+ unified memory recommended. Open <em>About This Mac</em> to confirm your chip and RAM.
          </div>
        </div>
        <div class="platform-content" data-platform="windows">
          <div class="hint-box">
            <strong>NVIDIA GPU</strong> — 8 GB+ VRAM recommended. Install the latest <a href="https://www.nvidia.com/drivers" target="_blank" style="color:#7c3aed;">NVIDIA driver</a> (CUDA is bundled).<br>
            <strong>AMD GPU</strong> — supported by some backends (Ollama, vLLM with ROCm). Check your backend's compatibility.<br>
            <strong>CPU-only</strong> — works but significantly slower.<br><br>
            <strong>Check:</strong> Open Task Manager &rarr; Performance &rarr; GPU to see your GPU and VRAM.
          </div>
        </div>
        <div class="platform-content" data-platform="linux">
          <div class="hint-box">
            <strong>NVIDIA GPU</strong> — the standard choice. Install NVIDIA drivers + CUDA toolkit, then verify with <code>nvidia-smi</code>.<br>
            <strong>AMD GPU</strong> — use ROCm. Supported by vLLM and Ollama on recent cards.<br>
            <strong>CPU-only</strong> — fine for small models or testing.<br><br>
            <strong>Check:</strong> Run <code>nvidia-smi</code> (NVIDIA) or <code>rocm-smi</code> (AMD) to confirm your GPU is visible.
          </div>
        </div>
      </div>
      <div class="wizard-nav">
        <div></div>
        <button class="btn" onclick="window.__wizNext()">Next &rarr;</button>
      </div>
    </div>

    <!-- Step 2: Backend -->
    <div class="wizard-step" data-step="2">
      <div class="wizard-card">
        <h2><span class="step-num">2</span> Set up your inference backend</h2>
        <p>ModelRelay connects to any OpenAI-compatible server running on your machine. Pick whichever you prefer:</p>
        <div class="platform-tabs backend-tabs" style="margin-top:16px;">
          <div class="tab active" data-backend="lmstudio" onclick="window.__setBackend('lmstudio')">LM Studio</div>
          <div class="tab" data-backend="ollama" onclick="window.__setBackend('ollama')">Ollama</div>
          <div class="tab" data-backend="llamacpp" onclick="window.__setBackend('llamacpp')">llama.cpp</div>
          <div class="tab" data-backend="vllm" onclick="window.__setBackend('vllm')">vLLM</div>
        </div>

        <div class="backend-content active" data-backend="lmstudio">
          <p><strong>Best for:</strong> beginners, desktop use, nice GUI for browsing models.</p>
          <p style="margin:16px 0;">
            <a href="https://lmstudio.ai" target="_blank" class="btn">Download LM Studio &rarr;</a>
          </p>
          <p style="color:#8b949e;font-size:0.85rem;">Install, launch, and head to the <strong style="color:#e6edf3;">Developer</strong> tab to start the local server. Runs on <code>http://localhost:1234</code> by default.</p>
        </div>

        <div class="backend-content" data-backend="ollama">
          <p><strong>Best for:</strong> CLI users, quick model management, easy multi-model setups.</p>
          <div class="code-block">
            <button class="copy-btn" onclick="window.__copyCode('install-ollama')">Copy</button>
            <code id="install-ollama">curl -fsSL https://ollama.ai/install.sh | sh</code>
          </div>
          <p style="color:#8b949e;font-size:0.85rem;margin-top:8px;">On macOS, download from <a href="https://ollama.ai" target="_blank" style="color:#7c3aed;">ollama.ai</a>. On Windows, use the <a href="https://ollama.ai/download/windows" target="_blank" style="color:#7c3aed;">Windows installer</a>. Serves on <code>http://localhost:11434</code>.</p>
        </div>

        <div class="backend-content" data-backend="llamacpp">
          <p><strong>Best for:</strong> lightweight deployments, headless servers, GGUF models.</p>
          <div class="code-block">
            <button class="copy-btn" onclick="window.__copyCode('install-llamacpp')">Copy</button>
            <code id="install-llamacpp"># Build from source (or download a release binary)
git clone https://github.com/ggerganov/llama.cpp && cd llama.cpp
cmake -B build && cmake --build build --config Release -t llama-server</code>
          </div>
          <p style="color:#8b949e;font-size:0.85rem;margin-top:8px;">Pre-built binaries available on the <a href="https://github.com/ggerganov/llama.cpp/releases" target="_blank" style="color:#7c3aed;">llama.cpp releases page</a>. Serves on <code>http://localhost:8080</code> by default (use <code>--port 8000</code> to change).</p>
        </div>

        <div class="backend-content" data-backend="vllm">
          <p><strong>Best for:</strong> production throughput, continuous batching, HuggingFace models.</p>
          <div class="code-block">
            <button class="copy-btn" onclick="window.__copyCode('install-vllm')">Copy</button>
            <code id="install-vllm">pip install vllm</code>
          </div>
          <p style="color:#8b949e;font-size:0.85rem;margin-top:8px;">Requires NVIDIA GPU with CUDA. Serves an OpenAI-compatible API on <code>http://localhost:8000</code>. See <a href="https://docs.vllm.ai" target="_blank" style="color:#7c3aed;">vLLM docs</a>.</p>
        </div>

        <p style="color:#8b949e;font-size:0.85rem;margin-top:16px;">Already have a running backend? <a href="#" onclick="event.preventDefault();window.__wizGoTo(4);" style="color:#7c3aed;">Skip to Download Worker &rarr;</a></p>
      </div>
      <div class="wizard-nav">
        <button class="btn btn-back" onclick="window.__wizPrev()">&larr; Back</button>
        <button class="btn" onclick="window.__wizNext()">Next &rarr;</button>
      </div>
    </div>

    <!-- Step 3: Model -->
    <div class="wizard-step" data-step="3">
      <div class="wizard-card">
        <h2><span class="step-num">3</span> Download and load a model</h2>

        <div class="backend-content active" data-backend="lmstudio">
          <ol style="color:#8b949e;margin:12px 0 12px 20px;line-height:2;">
            <li>Open the <strong style="color:#e6edf3;">Discover</strong> tab and search for a model (e.g. <code>llama-3.2-3b</code>)</li>
            <li>Click <strong style="color:#e6edf3;">Download</strong> and wait for it to complete</li>
            <li>Go to the <strong style="color:#e6edf3;">Developer</strong> tab</li>
            <li>Select your model and click <strong style="color:#e6edf3;">Start Server</strong></li>
            <li>Confirm the server is running on <code>http://localhost:1234</code></li>
          </ol>
        </div>

        <div class="backend-content" data-backend="ollama">
          <p>Pull a model and start serving:</p>
          <div class="code-block">
            <button class="copy-btn" onclick="window.__copyCode('model-ollama')">Copy</button>
            <code id="model-ollama">ollama pull llama3.2:3b
ollama serve</code>
          </div>
          <p style="color:#8b949e;font-size:0.85rem;margin-top:8px;">Browse models at <a href="https://ollama.ai/library" target="_blank" style="color:#7c3aed;">ollama.ai/library</a>. The server runs on <code>http://localhost:11434</code>.</p>
        </div>

        <div class="backend-content" data-backend="llamacpp">
          <p>Download a GGUF model and start the server:</p>
          <div class="code-block">
            <button class="copy-btn" onclick="window.__copyCode('model-llamacpp')">Copy</button>
            <code id="model-llamacpp"># Download a GGUF model (example: Llama 3.2 3B)
curl -L -o model.gguf https://huggingface.co/bartowski/Llama-3.2-3B-Instruct-GGUF/resolve/main/Llama-3.2-3B-Instruct-Q4_K_M.gguf

# Start the server
./build/bin/llama-server -m model.gguf --port 8000 --host 0.0.0.0</code>
          </div>
          <p style="color:#8b949e;font-size:0.85rem;margin-top:8px;">Find GGUF models on <a href="https://huggingface.co/models?sort=trending&amp;search=gguf" target="_blank" style="color:#7c3aed;">HuggingFace</a>. The Q4_K_M quantization is a good balance of quality and speed.</p>
        </div>

        <div class="backend-content" data-backend="vllm">
          <p>Start vLLM with a HuggingFace model:</p>
          <div class="code-block">
            <button class="copy-btn" onclick="window.__copyCode('model-vllm')">Copy</button>
            <code id="model-vllm">vllm serve meta-llama/Llama-3.2-3B-Instruct \
  --port 8000 \
  --host 0.0.0.0</code>
          </div>
          <p style="color:#8b949e;font-size:0.85rem;margin-top:8px;">vLLM downloads from HuggingFace automatically. You may need <code>huggingface-cli login</code> for gated models.</p>
        </div>

        <div class="hint-box" style="margin-top:16px;">
          <strong>Verify it's running:</strong> <code>curl http://localhost:<span id="backend-port-hint">1234</span>/v1/models</code> should return a JSON list of available models.
        </div>
      </div>
      <div class="wizard-nav">
        <button class="btn btn-back" onclick="window.__wizPrev()">&larr; Back</button>
        <button class="btn" onclick="window.__wizNext()">Next &rarr;</button>
      </div>
    </div>

    <!-- Step 4: Download Worker -->
    <div class="wizard-step" data-step="4">
      <div class="wizard-card">
        <h2><span class="step-num">4</span> Download the worker binary</h2>
        <p>The ModelRelay worker runs alongside your model server and connects it to the relay.</p>
        <div class="code-block">
          <button class="copy-btn" onclick="window.__copyCode('download-cmd')">Copy</button>
          <code id="download-cmd">curl -L -o modelrelay-worker https://github.com/ericflo/modelrelay/releases/latest/download/modelrelay-worker-linux-amd64 &amp;&amp; chmod +x modelrelay-worker</code>
        </div>
        <p style="color:#8b949e;font-size:0.85rem;margin-top:12px;">
          Or download from <a href="https://github.com/ericflo/modelrelay/releases/latest" target="_blank">GitHub Releases</a>. All platforms (Linux, macOS, Windows) and architectures (x86_64, arm64) are available.
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
        <p>Create a <code>config.toml</code> next to the worker binary:</p>
        <div class="config-input">
          <label for="cfg-server-url">Server URL:</label>
          <input id="cfg-server-url" type="text" placeholder="http://your-server:8080">
        </div>
        <div class="config-input">
          <label for="cfg-worker-secret">Worker Secret:</label>
          <input id="cfg-worker-secret" type="text" placeholder="your-worker-secret">
        </div>
        <div class="config-input">
          <label for="cfg-worker-name">Worker Name:</label>
          <input id="cfg-worker-name" type="text" placeholder="my-gpu-box">
        </div>
        <div class="code-block">
          <button class="copy-btn" onclick="window.__copyCode('config-toml')">Copy</button>
          <code id="config-toml">proxy_url = ""
worker_secret = "your-worker-secret"
worker_name = "my-gpu-box"
backend_url = "http://localhost:1234"
models = ["*"]</code>
        </div>
        <div class="hint-box">
          <strong>worker_secret</strong> — shared secret that must match the <code>WORKER_SECRET</code> on your ModelRelay server. It authenticates the worker connection.<br>
          <strong>worker_name</strong> — a label for this machine (e.g. "strix-halo-lmstudio", "rtx4090-desktop").<br>
          <strong>models = ["*"]</strong> — advertises all models from your backend. Replace with specific names to expose a subset.
        </div>
        <details style="margin-top:12px;">
          <summary style="color:#7c3aed;cursor:pointer;font-size:0.9rem;font-weight:600;">Prefer environment variables?</summary>
          <div class="code-block" style="margin-top:8px;">
            <button class="copy-btn" onclick="window.__copyCode('config-env')">Copy</button>
            <code id="config-env">export PROXY_URL=""
export WORKER_SECRET="your-worker-secret"
export WORKER_NAME="my-gpu-box"
export BACKEND_URL="http://localhost:1234"
export MODELS="*"</code>
          </div>
          <p style="color:#8b949e;font-size:0.85rem;margin-top:4px;">CLI flags also work: <code>--proxy-url</code>, <code>--worker-secret</code>, <code>--backend-url</code>, <code>--models</code>.</p>
        </details>
      </div>
      <div class="wizard-nav">
        <button class="btn btn-back" onclick="window.__wizPrev()">&larr; Back</button>
        <button class="btn" onclick="window.__wizNext()">Next &rarr;</button>
      </div>
    </div>

    <!-- Step 6: Connect -->
    <div class="wizard-step" data-step="6">
      <div class="wizard-card">
        <h2><span class="step-num">6</span> Start the worker</h2>
        <p>Run the worker from the directory with your <code>config.toml</code>:</p>
        <div class="code-block">
          <button class="copy-btn" onclick="window.__copyCode('run-cmd')">Copy</button>
          <code id="run-cmd">./modelrelay-worker --config config.toml</code>
        </div>
        <p style="margin-top:16px;">The worker will connect to your server over WebSocket. We'll detect it automatically:</p>
        <div class="status-indicator" id="worker-status">
          <div class="pulse" id="worker-pulse"></div>
          <span id="worker-status-text">Waiting for worker to connect...</span>
        </div>

        <div id="troubleshoot-hints" style="display:none;" class="hint-box">
          <strong>Taking a while?</strong> Common fixes:<br>
          &bull; Check the worker terminal for error messages<br>
          &bull; Verify <code>proxy_url</code> in config.toml points to this server<br>
          &bull; Confirm <code>worker_secret</code> matches the server's <code>WORKER_SECRET</code><br>
          &bull; If the server is remote, ensure port 8080 is reachable (no firewall blocking)
        </div>

        <button id="skip-detect" class="skip-link" onclick="window.__wizNext();">Skip detection &rarr; (worker may be on a different network)</button>

        <p style="color:#8b949e;font-size:0.85rem;margin-top:12px;">
          Admin token must be set on the <a href="/dashboard">dashboard</a> for live detection. Polling every 3 seconds.
        </p>
      </div>
      <div class="wizard-nav">
        <button class="btn btn-back" onclick="window.__wizPrev()">&larr; Back</button>
        <button class="btn" id="step6-next" onclick="window.__wizNext()">Next &rarr;</button>
      </div>
    </div>

    <!-- Step 7: Test -->
    <div class="wizard-step" data-step="7">
      <div class="wizard-card">
        <h2><span class="step-num">7</span> Test inference</h2>
        <p>Send a request through the relay to verify the full pipeline works.</p>
        <div class="config-input">
          <label for="test-model">Model name:</label>
          <input id="test-model" type="text" placeholder="e.g. llama-3.2-3b-instruct" value="">
        </div>
        <div class="config-input">
          <label for="test-api-key">API key <span style="color:#484f58;">(optional, for auth-required setups)</span>:</label>
          <input id="test-api-key" type="text" placeholder="mr_live_..." value="">
        </div>
        <p style="margin:16px 0;">
          <button class="btn" id="test-btn" onclick="window.__testInference()">Send Test Request</button>
        </p>
        <div id="test-result" class="test-result" style="display:none;"></div>
        <details style="margin-top:16px;">
          <summary style="color:#7c3aed;cursor:pointer;font-size:0.9rem;font-weight:600;">Test from the command line</summary>
          <div class="code-block" style="margin-top:8px;">
            <button class="copy-btn" onclick="window.__copyCode('curl-test')">Copy</button>
            <code id="curl-test">curl -X POST http://localhost:8080/v1/chat/completions \
  -H "Content-Type: application/json" \
  -d '{"model":"your-model","messages":[{"role":"user","content":"Hello!"}],"max_tokens":100}'</code>
          </div>
        </details>
      </div>
      <div class="wizard-nav">
        <button class="btn btn-back" onclick="window.__wizPrev()">&larr; Back</button>
        <button class="btn" onclick="window.__wizNext()">Next &rarr;</button>
      </div>
    </div>

    <!-- Step 8: Persist -->
    <div class="wizard-step" data-step="8">
      <div class="wizard-card">
        <h2><span class="step-num">8</span> Make it persistent</h2>
        <p>Your worker is running — now set it up as a system service so it starts on boot and restarts on crash.</p>

        <div class="platform-tabs persist-tabs" style="margin-top:20px;">
          <div class="tab" data-platform="linux" onclick="window.__setPersistPlatform('linux')">Linux</div>
          <div class="tab" data-platform="macos" onclick="window.__setPersistPlatform('macos')">macOS</div>
          <div class="tab" data-platform="windows" onclick="window.__setPersistPlatform('windows')">Windows</div>
        </div>

        <div class="persist-content" data-platform="linux">
          <p style="font-weight:600;color:#e6edf3;">systemd — supports multiple workers per machine</p>
          <p style="margin-top:12px;font-weight:600;color:#e6edf3;">1. Install binary and create service user</p>
          <div class="code-block">
            <button class="copy-btn" onclick="window.__copyCode('persist-linux-1')">Copy</button>
            <code id="persist-linux-1">sudo install -m 755 modelrelay-worker /usr/local/bin/
sudo useradd --system --no-create-home modelrelay
sudo mkdir -p /var/lib/modelrelay /etc/modelrelay</code>
          </div>
          <p style="margin-top:12px;font-weight:600;color:#e6edf3;">2. Install the service file and configure</p>
          <div class="code-block">
            <button class="copy-btn" onclick="window.__copyCode('persist-linux-2')">Copy</button>
            <code id="persist-linux-2"># Download the template unit
curl -L -o /tmp/modelrelay-worker@.service \
  https://raw.githubusercontent.com/ericflo/modelrelay/main/extras/modelrelay-worker%40.service

sudo cp /tmp/modelrelay-worker@.service /etc/systemd/system/

# Create per-instance env file
sudo tee /etc/modelrelay/worker-gpu0.env > /dev/null &lt;&lt;'EOF'
PROXY_URL=http://your-proxy:8080
WORKER_SECRET=your-secret
BACKEND_URL=http://127.0.0.1:8000
MODELS=llama3.2:3b
EOF</code>
          </div>
          <p style="margin-top:12px;font-weight:600;color:#e6edf3;">3. Enable and start</p>
          <div class="code-block">
            <button class="copy-btn" onclick="window.__copyCode('persist-linux-3')">Copy</button>
            <code id="persist-linux-3">sudo systemctl daemon-reload
sudo systemctl enable --now modelrelay-worker@gpu0</code>
          </div>
          <p style="margin-top:12px;font-weight:600;color:#e6edf3;">4. Verify</p>
          <div class="code-block">
            <button class="copy-btn" onclick="window.__copyCode('persist-linux-4')">Copy</button>
            <code id="persist-linux-4">systemctl status modelrelay-worker@gpu0
journalctl -u modelrelay-worker@gpu0 -f</code>
          </div>
          <p style="color:#8b949e;font-size:0.85rem;margin-top:8px;">Add more workers: <code>modelrelay-worker@gpu1</code>, <code>@gpu2</code>, etc. Each gets its own env file.</p>
        </div>

        <div class="persist-content" data-platform="macos" style="display:none;">
          <p style="font-weight:600;color:#e6edf3;">launchd — starts on boot, restarts on crash</p>
          <p style="margin-top:12px;font-weight:600;color:#e6edf3;">1. Create the plist</p>
          <div class="code-block">
            <button class="copy-btn" onclick="window.__copyCode('persist-mac-1')">Copy</button>
            <code id="persist-mac-1">sudo tee /Library/LaunchDaemons/io.modelrelay.worker.plist > /dev/null &lt;&lt;'EOF'
&lt;?xml version="1.0" encoding="UTF-8"?&gt;
&lt;!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN"
  "http://www.apple.com/DTDs/PropertyList-1.0.dtd"&gt;
&lt;plist version="1.0"&gt;
&lt;dict&gt;
  &lt;key&gt;Label&lt;/key&gt;&lt;string&gt;io.modelrelay.worker&lt;/string&gt;
  &lt;key&gt;ProgramArguments&lt;/key&gt;
  &lt;array&gt;
    &lt;string&gt;/usr/local/bin/modelrelay-worker&lt;/string&gt;
    &lt;string&gt;--config&lt;/string&gt;
    &lt;string&gt;/etc/modelrelay/config.toml&lt;/string&gt;
  &lt;/array&gt;
  &lt;key&gt;RunAtLoad&lt;/key&gt;&lt;true/&gt;
  &lt;key&gt;KeepAlive&lt;/key&gt;&lt;true/&gt;
  &lt;key&gt;StandardErrorPath&lt;/key&gt;
  &lt;string&gt;/var/log/modelrelay-worker.log&lt;/string&gt;
&lt;/dict&gt;
&lt;/plist&gt;
EOF</code>
          </div>
          <p style="margin-top:12px;font-weight:600;color:#e6edf3;">2. Install binary and config</p>
          <div class="code-block">
            <button class="copy-btn" onclick="window.__copyCode('persist-mac-2')">Copy</button>
            <code id="persist-mac-2">sudo cp modelrelay-worker /usr/local/bin/
sudo mkdir -p /etc/modelrelay
sudo cp config.toml /etc/modelrelay/config.toml</code>
          </div>
          <p style="margin-top:12px;font-weight:600;color:#e6edf3;">3. Load and start</p>
          <div class="code-block">
            <button class="copy-btn" onclick="window.__copyCode('persist-mac-3')">Copy</button>
            <code id="persist-mac-3">sudo launchctl load /Library/LaunchDaemons/io.modelrelay.worker.plist</code>
          </div>
          <p style="margin-top:12px;font-weight:600;color:#e6edf3;">4. Verify</p>
          <div class="code-block">
            <button class="copy-btn" onclick="window.__copyCode('persist-mac-4')">Copy</button>
            <code id="persist-mac-4">sudo launchctl list | grep modelrelay
tail -f /var/log/modelrelay-worker.log</code>
          </div>
        </div>

        <div class="persist-content" data-platform="windows" style="display:none;">
          <p style="font-weight:600;color:#e6edf3;">Windows Service — run PowerShell as Administrator</p>
          <p style="margin-top:12px;font-weight:600;color:#e6edf3;">1. Install the binary</p>
          <div class="code-block">
            <button class="copy-btn" onclick="window.__copyCode('persist-win-1')">Copy</button>
            <code id="persist-win-1">mkdir C:\ModelRelay
copy modelrelay-worker.exe C:\ModelRelay\</code>
          </div>
          <p style="margin-top:12px;font-weight:600;color:#e6edf3;">2. Create the service</p>
          <div class="code-block">
            <button class="copy-btn" onclick="window.__copyCode('persist-win-2')">Copy</button>
            <code id="persist-win-2">sc.exe create ModelRelayWorker binPath= "C:\ModelRelay\modelrelay-worker.exe" start= auto</code>
          </div>
          <p style="margin-top:12px;font-weight:600;color:#e6edf3;">3. Set environment variables</p>
          <div class="code-block">
            <button class="copy-btn" onclick="window.__copyCode('persist-win-3')">Copy</button>
            <code id="persist-win-3">[Environment]::SetEnvironmentVariable("PROXY_URL", "http://your-proxy:8080", "Machine")
[Environment]::SetEnvironmentVariable("WORKER_SECRET", "your-secret", "Machine")
[Environment]::SetEnvironmentVariable("BACKEND_URL", "http://localhost:8000", "Machine")
[Environment]::SetEnvironmentVariable("MODELS", "llama3.2:3b", "Machine")</code>
          </div>
          <p style="margin-top:12px;font-weight:600;color:#e6edf3;">4. Start and verify</p>
          <div class="code-block">
            <button class="copy-btn" onclick="window.__copyCode('persist-win-4')">Copy</button>
            <code id="persist-win-4">Start-Service ModelRelayWorker
Get-Service ModelRelayWorker</code>
          </div>
          <p style="color:#8b949e;font-size:0.85rem;margin-top:8px;">
            For annotated scripts with error handling, see <a href="https://github.com/ericflo/modelrelay/blob/main/extras/install-windows-service-worker.ps1" target="_blank">extras/install-windows-service-worker.ps1</a>.
          </p>
        </div>
      </div>

      <div class="wizard-nav">
        <button class="btn btn-back" onclick="window.__wizPrev()">&larr; Back</button>
      </div>

      <div class="wizard-card" style="text-align:center;">
        <h2 style="color:#34d399;">&#127881; Setup complete!</h2>
        <p>Your worker is connected, tested, and will start automatically on boot.</p>
        <p style="margin-top:16px;">
          <a href="/dashboard" class="btn">Go to Dashboard</a>
          <a href="/setup" class="btn btn-back" style="margin-left:8px;" onclick="event.preventDefault();window.__wizGoTo(4);">Add another machine</a>
        </p>
      </div>
    </div>
    "##;

    format!(
        r#"<!DOCTYPE html>
<html lang="en">
<head>
  <meta charset="utf-8">
  <meta name="viewport" content="width=device-width, initial-scale=1">
  <title>Setup — ModelRelay</title>
  <link rel="icon" href="data:image/svg+xml,<svg xmlns='http://www.w3.org/2000/svg' viewBox='0 0 100 100'><rect width='100' height='100' rx='20' fill='%237c3aed'/><text x='50' y='72' font-size='60' font-weight='bold' text-anchor='middle' fill='white'>M</text></svg>">
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
        <a href="/integrate">Integrate</a>
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

/// Build the integration snippets page (no cloud config).
#[must_use]
#[allow(clippy::too_many_lines)]
pub fn integrate_page() -> String {
    integrate_page_with_config(None)
}

/// Build the integration snippets page, optionally pre-configured for cloud users.
#[must_use]
#[allow(clippy::too_many_lines)]
pub fn integrate_page_with_config(cloud_config: Option<&CloudWizardConfig>) -> String {
    let integrate_css = r"
    .integrate-inputs {
      display:flex; gap:12px; align-items:flex-end; flex-wrap:wrap;
      margin-bottom:32px; padding:20px; background:#161b22;
      border:1px solid #21262d; border-radius:12px;
    }
    .integrate-inputs .field { display:flex; flex-direction:column; flex:1; min-width:180px; }
    .integrate-inputs label { font-size:0.78rem; color:#8b949e; text-transform:uppercase; letter-spacing:0.5px; margin-bottom:4px; }
    .integrate-inputs input {
      padding:8px 12px; background:#0d1117; border:1px solid #30363d;
      border-radius:8px; color:#e6edf3; font-size:0.9rem; font-family:'SFMono-Regular',Consolas,monospace;
    }
    .integrate-inputs input:focus { outline:none; border-color:#7c3aed; }

    .section-heading {
      font-size:1.2rem; font-weight:700; margin:32px 0 16px; display:flex;
      align-items:center; gap:10px;
    }
    .section-heading .icon { font-size:1.4rem; }
    .section-heading:first-of-type { margin-top:0; }

    .int-tabs { display:flex; gap:6px; margin-bottom:0; flex-wrap:wrap; }
    .int-tabs .tab {
      padding:8px 16px; background:#0d1117; border:1px solid #30363d;
      border-radius:8px 8px 0 0; color:#8b949e; cursor:pointer;
      font-size:0.85rem; font-weight:600; transition:all 0.15s;
      border-bottom:none; position:relative; top:1px;
    }
    .int-tabs .tab:hover { border-color:#7c3aed; color:#e6edf3; }
    .int-tabs .tab.active { background:#161b22; border-color:#21262d; color:#7c3aed; }

    .int-panel {
      background:#161b22; border:1px solid #21262d; border-radius:0 12px 12px 12px;
      padding:24px; margin-bottom:24px;
    }
    .int-panel p { color:#8b949e; margin-bottom:12px; line-height:1.7; font-size:0.92rem; }
    .int-panel h3 { font-size:1rem; margin-bottom:8px; color:#e6edf3; }
    .int-panel .step-label { color:#7c3aed; font-weight:600; font-size:0.85rem; margin-bottom:4px; }

    .int-content { display:none; }
    .int-content.active { display:block; }

    .code-block {
      background:#0d1117; border:1px solid #30363d; border-radius:8px;
      padding:16px; font-family:'SFMono-Regular',Consolas,monospace;
      font-size:0.82rem; color:#e6edf3; overflow-x:auto; position:relative;
      line-height:1.7; margin:8px 0 16px; white-space:pre;
    }
    .code-block .copy-btn {
      position:absolute; top:8px; right:8px; padding:4px 10px;
      font-size:0.72rem; background:#30363d; color:#e6edf3;
      border:none; border-radius:4px; cursor:pointer; z-index:1;
    }
    .code-block .copy-btn:hover { background:#484f58; }
    .code-block .copy-btn.copied { background:#064e3b; color:#34d399; }

    .ref-card {
      background:#161b22; border:1px solid #21262d; border-radius:12px;
      padding:20px; margin-bottom:16px;
    }
    .ref-card h3 { font-size:1rem; margin-bottom:8px; }
    .ref-row { display:flex; align-items:center; gap:12px; margin-bottom:8px; }
    .ref-row .ref-label { color:#8b949e; font-size:0.85rem; min-width:180px; }
    .ref-row code {
      flex:1; padding:6px 10px; background:#0d1117; border:1px solid #30363d;
      border-radius:6px; font-size:0.85rem; color:#e6edf3; position:relative;
      display:flex; align-items:center; justify-content:space-between;
    }
    .ref-row code .copy-btn {
      padding:2px 8px; font-size:0.7rem; background:#30363d; color:#e6edf3;
      border:none; border-radius:4px; cursor:pointer; margin-left:8px; flex-shrink:0;
    }
    .ref-row code .copy-btn:hover { background:#484f58; }

    .hint-box {
      background:#1c1f26; border:1px solid #30363d; border-radius:8px;
      padding:12px 14px; margin:8px 0 16px; font-size:0.82rem; color:#8b949e;
    }
    .hint-box strong { color:#e6edf3; }

    @media (max-width:600px) {
      .integrate-inputs { flex-direction:column; }
      .integrate-inputs .field { min-width:100%; }
      .int-tabs { gap:4px; }
      .int-tabs .tab { padding:6px 10px; font-size:0.78rem; }
      .ref-row { flex-direction:column; align-items:stretch; gap:4px; }
      .ref-row .ref-label { min-width:auto; }
    }
    ";

    let integrate_js = r#"
(function() {
  const $ = s => document.querySelector(s);
  const $$ = s => document.querySelectorAll(s);

  const cloudCfg = window.__mrCloudConfig || null;

  // ── Inputs ──
  const urlInput = $('#int-server-url');
  const keyInput = $('#int-api-key');
  const modelInput = $('#int-model-name');

  // Pre-fill from cloud config or localStorage
  urlInput.value = (cloudCfg && cloudCfg.serverUrl) ? cloudCfg.serverUrl
    : localStorage.getItem('mr_server_url') || window.location.origin;
  keyInput.value = (cloudCfg && cloudCfg.apiKey) ? cloudCfg.apiKey
    : localStorage.getItem('mr_test_api_key') || '';
  modelInput.value = localStorage.getItem('mr_model_name') || '';

  function sv() { return urlInput.value.trim().replace(/\/+$/, '') || 'https://your-server.example.com'; }
  function ak() { return keyInput.value.trim() || 'your-api-key'; }
  function mn() { return modelInput.value.trim() || 'your-model-name'; }

  // ── Tab switching ──
  function initTabs(section) {
    const tabs = section.querySelectorAll('.int-tabs .tab');
    const panels = section.querySelectorAll('.int-content');
    tabs.forEach(tab => {
      tab.addEventListener('click', () => {
        tabs.forEach(t => t.classList.remove('active'));
        panels.forEach(p => p.classList.remove('active'));
        tab.classList.add('active');
        const target = section.querySelector('.int-content[data-tab="' + tab.dataset.tab + '"]');
        if (target) target.classList.add('active');
        updateSnippets();
      });
    });
  }
  $$('.tab-section').forEach(initTabs);

  // ── Copy button ──
  document.addEventListener('click', e => {
    const btn = e.target.closest('.copy-btn');
    if (!btn) return;
    const block = btn.closest('.code-block') || btn.closest('code');
    if (!block) return;
    // Get text content minus the button text
    const clone = block.cloneNode(true);
    clone.querySelectorAll('.copy-btn').forEach(b => b.remove());
    const text = clone.textContent.trim();
    navigator.clipboard.writeText(text).then(() => {
      btn.textContent = 'Copied!';
      btn.classList.add('copied');
      setTimeout(() => { btn.textContent = 'Copy'; btn.classList.remove('copied'); }, 1500);
    });
  });

  // ── Snippet updater ──
  function updateSnippets() {
    const s = sv(), a = ak(), m = mn();
    // Agent snippets
    $$('[data-snippet]').forEach(el => {
      const tpl = el.getAttribute('data-snippet');
      el.querySelector('.code-text').innerHTML = tpl
        .replace(/SERVER_URL/g, escHtml(s))
        .replace(/API_KEY/g, escHtml(a))
        .replace(/MODEL_NAME/g, escHtml(m));
    });
    // Ref values
    $$('[data-ref]').forEach(el => {
      const tpl = el.getAttribute('data-ref');
      const span = el.querySelector('.ref-val');
      if (span) span.textContent = tpl.replace(/SERVER_URL/g, s).replace(/API_KEY/g, a);
    });
  }

  function escHtml(s) {
    const d = document.createElement('div');
    d.textContent = s;
    return d.innerHTML;
  }

  urlInput.addEventListener('input', () => {
    localStorage.setItem('mr_server_url', urlInput.value.trim());
    updateSnippets();
  });
  keyInput.addEventListener('input', () => {
    localStorage.setItem('mr_test_api_key', keyInput.value.trim());
    updateSnippets();
  });
  modelInput.addEventListener('input', () => {
    localStorage.setItem('mr_model_name', modelInput.value.trim());
    updateSnippets();
  });

  updateSnippets();
})();
    "#;

    // ── Snippet templates (using SERVER_URL / API_KEY / MODEL_NAME placeholders) ──

    let snippet_pi = r"{
  &quot;providers&quot;: {
    &quot;modelrelay&quot;: {
      &quot;baseUrl&quot;: &quot;SERVER_URL/v1&quot;,
      &quot;api&quot;: &quot;openai-completions&quot;,
      &quot;apiKey&quot;: &quot;API_KEY&quot;,
      &quot;compat&quot;: { &quot;supportsDeveloperRole&quot;: false, &quot;supportsReasoningEffort&quot;: false },
      &quot;models&quot;: [{
        &quot;id&quot;: &quot;MODEL_NAME&quot;,
        &quot;name&quot;: &quot;My Model via ModelRelay&quot;,
        &quot;input&quot;: [&quot;text&quot;],
        &quot;contextWindow&quot;: 200000,
        &quot;maxTokens&quot;: 16384
      }]
    }
  }
}";

    let snippet_codex = r"model_provider = &quot;modelrelay&quot;
model = &quot;MODEL_NAME&quot;

[model_providers.modelrelay]
name = &quot;ModelRelay&quot;
base_url = &quot;SERVER_URL/v1&quot;
env_key = &quot;MODELRELAY_API_KEY&quot;";

    let snippet_codex_env = r"export MODELRELAY_API_KEY=API_KEY";

    let snippet_aider = r"export OPENAI_API_BASE=SERVER_URL/v1
export OPENAI_API_KEY=API_KEY
aider --model openai/MODEL_NAME";

    let snippet_continue = r"models:
  - name: My Model via ModelRelay
    provider: openai
    model: MODEL_NAME
    apiBase: SERVER_URL/v1
    apiKey: API_KEY";

    let snippet_curl = r"curl SERVER_URL/v1/chat/completions \
  -H &quot;Content-Type: application/json&quot; \
  -H &quot;Authorization: Bearer API_KEY&quot; \
  -d &#x27;{
    &quot;model&quot;: &quot;MODEL_NAME&quot;,
    &quot;messages&quot;: [{&quot;role&quot;: &quot;user&quot;, &quot;content&quot;: &quot;Hello!&quot;}]
  }&#x27;";

    let snippet_python = r"from openai import OpenAI

client = OpenAI(
    base_url=&quot;SERVER_URL/v1&quot;,
    api_key=&quot;API_KEY&quot;,
)

response = client.chat.completions.create(
    model=&quot;MODEL_NAME&quot;,
    messages=[{&quot;role&quot;: &quot;user&quot;, &quot;content&quot;: &quot;Hello!&quot;}],
)
print(response.choices[0].message.content)";

    let snippet_node = r"import OpenAI from &quot;openai&quot;;

const client = new OpenAI({
  baseURL: &quot;SERVER_URL/v1&quot;,
  apiKey: &quot;API_KEY&quot;,
});

const response = await client.chat.completions.create({
  model: &quot;MODEL_NAME&quot;,
  messages: [{ role: &quot;user&quot;, content: &quot;Hello!&quot; }],
});
console.log(response.choices[0].message.content);";

    let snippet_go = r"package main

import (
    &quot;context&quot;
    &quot;fmt&quot;
    openai &quot;github.com/sashabaranov/go-openai&quot;
)

func main() {
    cfg := openai.DefaultConfig(&quot;API_KEY&quot;)
    cfg.BaseURL = &quot;SERVER_URL/v1&quot;
    client := openai.NewClientWithConfig(cfg)

    resp, _ := client.CreateChatCompletion(context.Background(),
        openai.ChatCompletionRequest{
            Model: &quot;MODEL_NAME&quot;,
            Messages: []openai.ChatCompletionMessage{
                {Role: &quot;user&quot;, Content: &quot;Hello!&quot;},
            },
        },
    )
    fmt.Println(resp.Choices[0].Message.Content)
}";

    format!(
        r#"<!DOCTYPE html>
<html lang="en">
<head>
  <meta charset="utf-8">
  <meta name="viewport" content="width=device-width, initial-scale=1">
  <title>Integrate — ModelRelay</title>
  <link rel="icon" href="data:image/svg+xml,<svg xmlns='http://www.w3.org/2000/svg' viewBox='0 0 100 100'><rect width='100' height='100' rx='20' fill='%237c3aed'/><text x='50' y='72' font-size='60' font-weight='bold' text-anchor='middle' fill='white'>M</text></svg>">
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
    .nav-links a.active {{ color: #7c3aed; }}

    .content {{ padding: 32px 0; }}
    .content h1 {{ font-size: 1.75rem; margin-bottom: 4px; }}
    .subtitle {{ color: #8b949e; margin-bottom: 24px; font-size: 0.95rem; }}

    footer {{ padding: 40px 0; border-top: 1px solid #21262d; text-align: center; color: #484f58; font-size: 0.85rem; }}
    footer a {{ color: #8b949e; }}

    code {{ font-family: "SFMono-Regular", Consolas, monospace; }}
    {integrate_css}
  </style>
</head>
<body>
  <nav>
    <div class="container">
      <a href="/" class="logo">Model<span>Relay</span></a>
      <div class="nav-links">
        <a href="/dashboard">Dashboard</a>
        <a href="/setup">Setup</a>
        <a href="/integrate" class="active">Integrate</a>
      </div>
    </div>
  </nav>

  <section class="content">
    <div class="container">
      <h1>Integrate</h1>
      <p class="subtitle">Copy-paste configuration for your favorite tools, agents, and languages.</p>

      <!-- ── Inputs bar ── -->
      <div class="integrate-inputs">
        <div class="field">
          <label for="int-server-url">Server URL</label>
          <input id="int-server-url" type="text" placeholder="https://your-server.example.com">
        </div>
        <div class="field">
          <label for="int-api-key">API Key</label>
          <input id="int-api-key" type="text" placeholder="your-api-key">
        </div>
        <div class="field">
          <label for="int-model-name">Model Name</label>
          <input id="int-model-name" type="text" placeholder="your-model-name">
        </div>
      </div>

      <!-- ═══ AI Coding Agents ═══ -->
      <div class="section-heading"><span class="icon">&#129302;</span> AI Coding Agents</div>
      <div class="tab-section">
        <div class="int-tabs">
          <div class="tab active" data-tab="pi">Pi</div>
          <div class="tab" data-tab="codex">Codex CLI</div>
          <div class="tab" data-tab="aider">Aider</div>
          <div class="tab" data-tab="continue">Continue.dev</div>
          <div class="tab" data-tab="cursor">Cursor</div>
        </div>
        <div class="int-panel">

          <!-- Pi -->
          <div class="int-content active" data-tab="pi">
            <h3>Pi <span style="color:#8b949e;font-weight:400;font-size:0.85rem;">by Mario Zechner</span></h3>
            <p class="step-label">1. Install</p>
            <div class="code-block"><button class="copy-btn">Copy</button><span class="code-text">npm install -g @mariozechner/pi-coding-agent</span></div>
            <p class="step-label">2. Configure <code style="font-size:0.82rem;color:#8b949e;">~/.pi/agent/models.json</code></p>
            <div class="code-block" data-snippet="{snippet_pi}"><button class="copy-btn">Copy</button><span class="code-text"></span></div>
          </div>

          <!-- Codex CLI -->
          <div class="int-content" data-tab="codex">
            <h3>Codex CLI <span style="color:#8b949e;font-weight:400;font-size:0.85rem;">by OpenAI</span></h3>
            <p class="step-label">1. Set environment variable</p>
            <div class="code-block" data-snippet="{snippet_codex_env}"><button class="copy-btn">Copy</button><span class="code-text"></span></div>
            <p class="step-label">2. Configure <code style="font-size:0.82rem;color:#8b949e;">~/.codex/config.toml</code></p>
            <div class="code-block" data-snippet="{snippet_codex}"><button class="copy-btn">Copy</button><span class="code-text"></span></div>
          </div>

          <!-- Aider -->
          <div class="int-content" data-tab="aider">
            <h3>Aider</h3>
            <p class="step-label">Run with environment variables</p>
            <div class="code-block" data-snippet="{snippet_aider}"><button class="copy-btn">Copy</button><span class="code-text"></span></div>
          </div>

          <!-- Continue.dev -->
          <div class="int-content" data-tab="continue">
            <h3>Continue.dev <span style="color:#8b949e;font-weight:400;font-size:0.85rem;">VS Code extension</span></h3>
            <p class="step-label">Add to <code style="font-size:0.82rem;color:#8b949e;">~/.continue/config.yaml</code></p>
            <div class="code-block" data-snippet="{snippet_continue}"><button class="copy-btn">Copy</button><span class="code-text"></span></div>
          </div>

          <!-- Cursor -->
          <div class="int-content" data-tab="cursor">
            <h3>Cursor</h3>
            <p>Configure via the Cursor settings UI:</p>
            <div class="hint-box">
              <strong>1.</strong> Open <strong>Settings &gt; Models</strong><br>
              <strong>2.</strong> Set <strong>"Override OpenAI Base URL"</strong> to:<br>
              <code style="color:#7c3aed;" class="cursor-url">SERVER_URL/v1</code><br>
              <strong>3.</strong> Set <strong>"OpenAI API Key"</strong> to your API key<br>
              <strong>4.</strong> Add your model name: <code style="color:#7c3aed;" class="cursor-model">MODEL_NAME</code>
            </div>
          </div>

        </div>
      </div>

      <!-- ═══ Languages & SDKs ═══ -->
      <div class="section-heading"><span class="icon">&#128187;</span> Languages &amp; SDKs</div>
      <div class="tab-section">
        <div class="int-tabs">
          <div class="tab active" data-tab="curl">curl</div>
          <div class="tab" data-tab="python">Python</div>
          <div class="tab" data-tab="node">Node.js</div>
          <div class="tab" data-tab="go">Go</div>
        </div>
        <div class="int-panel">

          <!-- curl -->
          <div class="int-content active" data-tab="curl">
            <h3>curl</h3>
            <div class="code-block" data-snippet="{snippet_curl}"><button class="copy-btn">Copy</button><span class="code-text"></span></div>
          </div>

          <!-- Python -->
          <div class="int-content" data-tab="python">
            <h3>Python <span style="color:#8b949e;font-weight:400;font-size:0.85rem;">pip install openai</span></h3>
            <div class="code-block" data-snippet="{snippet_python}"><button class="copy-btn">Copy</button><span class="code-text"></span></div>
          </div>

          <!-- Node.js -->
          <div class="int-content" data-tab="node">
            <h3>Node.js / TypeScript <span style="color:#8b949e;font-weight:400;font-size:0.85rem;">npm install openai</span></h3>
            <div class="code-block" data-snippet="{snippet_node}"><button class="copy-btn">Copy</button><span class="code-text"></span></div>
          </div>

          <!-- Go -->
          <div class="int-content" data-tab="go">
            <h3>Go <span style="color:#8b949e;font-weight:400;font-size:0.85rem;">github.com/sashabaranov/go-openai</span></h3>
            <div class="code-block" data-snippet="{snippet_go}"><button class="copy-btn">Copy</button><span class="code-text"></span></div>
          </div>

        </div>
      </div>

      <!-- ═══ Quick Reference ═══ -->
      <div class="section-heading"><span class="icon">&#128218;</span> Quick Reference</div>
      <div class="ref-card">
        <div class="ref-row" data-ref="SERVER_URL/v1">
          <span class="ref-label">API Base URL</span>
          <code><span class="ref-val">SERVER_URL/v1</span><button class="copy-btn">Copy</button></code>
        </div>
        <div class="ref-row" data-ref="Bearer API_KEY">
          <span class="ref-label">Authorization Header</span>
          <code><span class="ref-val">Bearer API_KEY</span><button class="copy-btn">Copy</button></code>
        </div>
        <div class="ref-row">
          <span class="ref-label">Supported Endpoints</span>
          <code><span class="ref-val">/v1/chat/completions, /v1/models</span><button class="copy-btn">Copy</button></code>
        </div>
      </div>

    </div>
  </section>

  <footer>
    <div class="container">
      &copy; 2026 ModelRelay &middot; <a href="https://github.com/ericflo/modelrelay">GitHub</a>
    </div>
  </footer>

  {cloud_config_script}
  <script>{integrate_js}</script>
</body>
</html>"#,
        cloud_config_script = cloud_config_script(cloud_config),
    )
}

fn cloud_config_script(config: Option<&CloudWizardConfig>) -> String {
    let Some(cfg) = config else {
        return String::new();
    };
    let escape = |s: &str| s.replace('\\', "\\\\").replace('"', "\\\"");
    let opt_js = |o: &Option<String>| match o {
        Some(k) => format!("\"{}\"", escape(k)),
        None => "null".to_string(),
    };
    let server_url = escape(&cfg.server_url);
    let poll_url = escape(&cfg.workers_poll_url);
    let api_key_js = opt_js(&cfg.api_key);
    let worker_secret_js = opt_js(&cfg.worker_secret);
    format!(
        "<script>window.__mrCloudConfig = {{ serverUrl: \"{server_url}\", apiKey: {api_key_js}, workerSecret: {worker_secret_js}, workersPollUrl: \"{poll_url}\" }};</script>"
    )
}

/// Shared HTML page shell used by the admin dashboard and commercial cloud routes.
///
/// `logged_in` controls the nav links: when `true`, shows Dashboard + Pricing + Log out;
/// when `false`, shows Pricing + Log in + Sign up + GitHub (matching the landing page nav).
#[must_use]
#[allow(clippy::too_many_lines)]
pub fn page_shell(title: &str, body_content: &str, logged_in: bool) -> String {
    let nav_links = if logged_in {
        r#"<a href="/dashboard">Dashboard</a>
        <a href="/integrate">Integrate</a>
        <a href="/pricing">Pricing</a>
        <form method="POST" action="/logout"><button type="submit">Log out</button></form>"#
    } else {
        r#"<a href="/pricing">Pricing</a>
        <a href="/login">Log in</a>
        <a class="btn-signup" href="/signup">Sign up</a>
        <a href="https://github.com/ericflo/modelrelay">GitHub</a>"#
    };

    format!(
        r#"<!DOCTYPE html>
<html lang="en">
<head>
  <meta charset="utf-8">
  <meta name="viewport" content="width=device-width, initial-scale=1">
  <title>{title} — ModelRelay</title>
  <link rel="icon" href="data:image/svg+xml,<svg xmlns='http://www.w3.org/2000/svg' viewBox='0 0 100 100'><rect width='100' height='100' rx='20' fill='%237c3aed'/><text x='50' y='72' font-size='60' font-weight='bold' text-anchor='middle' fill='white'>M</text></svg>">
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
    .nav-links .btn-signup {{ background: #7c3aed; color: #fff; padding: 6px 16px; border-radius: 6px; font-weight: 600; font-size: 0.9rem; }}
    .nav-links .btn-signup:hover {{ background: #6d28d9; color: #fff; text-decoration: none; }}

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

    /* Tablet */
    @media (max-width: 768px) {{
      .content {{ padding: 40px 0; }}
      .content h1 {{ font-size: 1.6rem; }}
      .card {{ padding: 24px; }}
      .nav-links a {{ font-size: 0.8rem; margin-left: 12px; }}
      .nav-links button {{ font-size: 0.8rem; margin-left: 12px; }}
      .info-table td:first-child {{ width: 120px; }}
    }}

    /* Mobile */
    @media (max-width: 480px) {{
      .container {{ padding: 0 16px; }}
      .content {{ padding: 32px 0; }}
      .content h1 {{ font-size: 1.4rem; }}
      .card {{ padding: 20px; }}
      .btn {{ display: block; width: 100%; text-align: center; }}
      .auth-form input {{ font-size: 1rem; padding: 12px; }}
      .info-table td {{ display: block; padding: 4px 0; }}
      .info-table td:first-child {{ width: auto; border-bottom: none; }}
      .key-display {{ font-size: 0.85rem; padding: 10px 12px; }}
      .nav-links .btn-signup {{ padding: 6px 12px; }}
    }}
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
