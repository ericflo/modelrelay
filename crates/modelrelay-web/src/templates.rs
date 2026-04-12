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

    let dashboard_override_css = r"
    .container { max-width: 960px; }
    .content { padding: 32px 0; }
    .content h1 { font-size: 1.75rem; margin-bottom: 20px; }
    code { font-family: 'SFMono-Regular', Consolas, monospace; }
    .dash-header { display: flex; justify-content: space-between; align-items: center; margin-bottom: 20px; gap: 12px; flex-wrap: wrap; }
    .dash-header h1 { margin-bottom: 0; }
    @media (max-width: 768px) {
      .content { padding: 24px 0; }
      .content h1 { font-size: 1.5rem; }
      .config-bar { flex-direction: column; align-items: stretch; }
      .config-bar input { width: 100%; }
      .config-bar label { margin-top: 4px; }
      .health-bar { gap: 8px; }
      .health-item { min-width: 0; flex-basis: calc(50% - 4px); }
      table.data { display: block; overflow-x: auto; -webkit-overflow-scrolling: touch; }
      .stat-row .stat-label { min-width: 100px; font-size: 0.8rem; }
      .key-actions { flex-direction: column; }
      .key-actions input { width: 100%; }
    }
    @media (max-width: 480px) {
      .container { padding: 0 16px; }
      .content { padding: 20px 0; }
      .content h1 { font-size: 1.3rem; }
      .dash-header { flex-direction: column; align-items: flex-start; }
      .health-item { flex-basis: 100%; }
      .btn { display: block; width: 100%; text-align: center; }
      .stat-row { flex-direction: column; align-items: flex-start; gap: 4px; }
      .stat-row .stat-label { min-width: 0; }
      .stat-row .stat-bar { width: 100%; }
    }
    ";

    let extra_css = ["<style>", dashboard_override_css, dashboard_css, "</style>"].concat();

    let dash_body = format!(
        "<div class=\"dash-header\">\n\
         <h1>Dashboard</h1>\n\
         <a href=\"/setup\" class=\"btn\">+ Add a machine</a>\n\
         </div>\n\
         {body_content}"
    );

    let extra_body_end = ["<script>", dashboard_js, "</script>"].concat();

    page_shell_custom("Dashboard", &dash_body, true, &extra_css, &extra_body_end)
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
      -webkit-overflow-scrolling:touch;
    }
    .wizard-progress .step-indicator {
      flex:1; text-align:center; padding:12px 8px; font-size:0.75rem;
      color:#484f58; border-bottom:2px solid #21262d; min-width:80px;
      transition: color 0.3s, border-color 0.3s; cursor:pointer; user-select:none;
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
    .wizard-progress .step-indicator.done::after {
      content:' \2713'; font-size:0.7rem; font-weight:700;
    }

    .wizard-step { display:none; animation:fadeIn 0.3s ease; }
    .wizard-step.active { display:block; }

    .wizard-card {
      background:#161b22; border:1px solid #21262d; border-radius:12px;
      padding:32px; margin-bottom:24px;
    }
    .wizard-card h2 { font-size:1.2rem; margin-bottom:12px; color:#e6edf3; }
    .wizard-card p { color:#8b949e; margin-bottom:12px; line-height:1.7; }

    .platform-tabs {
      display:flex; gap:8px; margin-bottom:20px; flex-wrap:wrap;
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
      padding:16px; margin:12px 0; font-size:0.85rem; color:#8b949e; line-height:1.7;
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
      font-size:0.75rem; background:#21262d; color:#8b949e;
      border:1px solid #30363d; border-radius:6px; cursor:pointer;
      transition: all 0.2s;
    }
    .code-block .copy-btn:hover { background:#30363d; color:#e6edf3; border-color:#484f58; }

    .wizard-nav {
      display:flex; justify-content:space-between; align-items:center;
      margin-top:24px; padding-top:24px; border-top:1px solid #21262d;
    }
    .wizard-nav .btn { min-width:120px; text-align:center; }
    .wizard-nav .btn-back {
      background:transparent; border:1px solid #30363d; color:#8b949e;
      transition: all 0.2s;
    }
    .wizard-nav .btn-back:hover { border-color:#7c3aed; color:#e6edf3; }

    .status-indicator {
      display:flex; align-items:center; gap:10px; padding:16px;
      background:#0d1117; border:1px solid #21262d; border-radius:8px;
      margin:16px 0; font-size:0.95rem;
    }
    .status-indicator .pulse {
      width:12px; height:12px; border-radius:50%; background:#484f58; flex-shrink:0;
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
      padding:10px 14px; background:#0d1117; border:1px solid #30363d;
      border-radius:8px; color:#e6edf3; font-size:0.9rem; flex:1; min-width:200px;
      transition: border-color 0.2s, box-shadow 0.2s;
    }
    .config-input input:focus { outline:none; border-color:#7c3aed; box-shadow:0 0 0 3px rgba(124,58,237,0.15); }

    details summary { list-style:none; }
    details summary::-webkit-details-marker { display:none; }

    @keyframes fadeIn { from { opacity:0; transform:translateY(8px); } to { opacity:1; transform:translateY(0); } }

    /* Wizard tablet */
    @media (max-width: 768px) {
      .wizard-card { padding:24px; }
      .wizard-card h2 { font-size:1.1rem; }
      .platform-tabs .tab { padding:8px 14px; font-size:0.85rem; }
      .config-input { flex-direction:column; align-items:stretch; }
      .config-input label { min-width:auto; }
      .config-input input { min-width:auto; }
    }

    /* Wizard mobile */
    @media (max-width: 480px) {
      .wizard-progress { margin-bottom:24px; }
      .wizard-progress .step-indicator { min-width:60px; padding:10px 4px; font-size:0.65rem; }
      .wizard-card { padding:20px; }
      .wizard-nav { flex-direction:column; gap:12px; }
      .wizard-nav .btn { width:100%; min-width:auto; }
      .wizard-nav .btn-back { order:1; }
      .platform-tabs .tab { flex:1; min-width:70px; padding:8px 10px; font-size:0.8rem; text-align:center; }
      .code-block { padding:12px; font-size:0.8rem; }
      .code-block .copy-btn { position:static; display:block; margin:0 0 8px auto; }
      .hint-box { padding:12px; font-size:0.8rem; }
      .status-indicator { padding:12px; font-size:0.85rem; }
    }
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

    let logged_in = cloud_config.is_some();

    let setup_override_css = r"
    .content h1 { font-size: 2rem; margin-bottom: 8px; }
    .subtitle { color: #8b949e; margin-bottom: 24px; }
    code { font-family: 'SFMono-Regular', Consolas, monospace; }
    ";

    let extra_css = ["<style>", setup_override_css, wizard_css, "</style>"].concat();

    let setup_body = format!(
        "<h1>Connect a Worker Machine</h1>\n\
         <p class=\"subtitle\">Follow these steps to connect a GPU machine to your ModelRelay deployment.</p>\n\
         {progress_html}\n\
         {steps_html}"
    );

    let cloud_cfg_script = cloud_config_script(cloud_config);
    let extra_body_end = format!("{cloud_cfg_script}<script>{wizard_js}</script>");

    page_shell_custom("Setup", &setup_body, logged_in, &extra_css, &extra_body_end)
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
      margin-bottom:40px; padding:24px; background:#161b22;
      border:1px solid #21262d; border-radius:12px;
    }
    .integrate-inputs .field { display:flex; flex-direction:column; flex:1; min-width:180px; }
    .integrate-inputs label { font-size:0.7rem; color:#8b949e; text-transform:uppercase; letter-spacing:1px; margin-bottom:8px; font-weight:600; }
    .integrate-inputs input {
      padding:10px 14px; background:#0d1117; border:1px solid #30363d;
      border-radius:8px; color:#e6edf3; font-size:0.88rem; font-family:'SFMono-Regular',Consolas,monospace;
      transition:border-color 0.2s, box-shadow 0.2s;
    }
    .integrate-inputs input:focus { outline:none; border-color:#7c3aed; box-shadow:0 0 0 3px rgba(124,58,237,0.15); }
    .integrate-inputs input::placeholder { color:#484f58; }

    .section-heading {
      font-size:1.15rem; font-weight:700; margin:48px 0 20px; display:flex;
      align-items:center; gap:10px; color:#e6edf3;
      padding-bottom:14px; border-bottom:1px solid #21262d;
    }
    .section-heading .icon { font-size:1.3rem; opacity:0.8; }
    .section-heading .endpoint-label {
      color:#7c3aed; font-weight:500; font-size:0.78rem;
      margin-left:auto; font-family:'SFMono-Regular',Consolas,monospace;
      background:rgba(124,58,237,0.1); padding:3px 10px; border-radius:20px;
      border:1px solid rgba(124,58,237,0.2);
    }
    .section-heading:first-of-type { margin-top:0; }

    .int-tabs { display:flex; gap:0; margin-bottom:0; flex-wrap:wrap; border-bottom:1px solid #21262d; }
    .int-tabs .tab {
      padding:10px 18px; background:transparent; border:none;
      border-bottom:2px solid transparent; color:#8b949e; cursor:pointer;
      font-size:0.82rem; font-weight:600; transition:all 0.2s ease;
    }
    .int-tabs .tab:hover { color:#e6edf3; background:rgba(124,58,237,0.05); }
    .int-tabs .tab.active {
      color:#7c3aed; border-bottom-color:#7c3aed;
      background:rgba(124,58,237,0.05);
    }

    .int-panel {
      background:#161b22; border:1px solid #21262d; border-top:none;
      border-radius:0 0 12px 12px; padding:24px; margin-bottom:28px;
    }
    .int-panel p { color:#8b949e; margin-bottom:12px; line-height:1.7; font-size:0.9rem; }
    .int-panel h3 { font-size:1rem; margin-bottom:8px; color:#e6edf3; }
    .int-panel .step-label {
      color:#7c3aed; font-weight:600; font-size:0.78rem; margin-bottom:6px;
      text-transform:uppercase; letter-spacing:0.8px;
    }

    .int-content { display:none; }
    .int-content.active { display:block; animation:fadeIn 0.2s ease; }
    @keyframes fadeIn { from { opacity:0; transform:translateY(4px); } to { opacity:1; transform:translateY(0); } }

    .code-block {
      background:#0d1117; border:1px solid #30363d; border-radius:8px;
      padding:16px 48px 16px 16px; font-family:'SFMono-Regular',Consolas,monospace;
      font-size:0.82rem; color:#e6edf3; overflow-x:auto; position:relative;
      line-height:1.7; margin:8px 0 16px; white-space:pre;
    }
    .code-block .copy-btn {
      position:absolute; top:8px; right:8px; padding:6px 12px;
      font-size:0.72rem; background:#21262d; color:#8b949e;
      border:1px solid #30363d; border-radius:6px; cursor:pointer; z-index:1;
      transition:all 0.15s; display:flex; align-items:center; gap:4px;
      font-family:-apple-system,BlinkMacSystemFont,'Segoe UI',Roboto,sans-serif;
    }
    .code-block .copy-btn:hover { background:#30363d; color:#e6edf3; border-color:#484f58; }
    .code-block .copy-btn.copied { background:#064e3b; color:#34d399; border-color:#065f46; }

    .ref-grid {
      display:grid; grid-template-columns:repeat(auto-fit, minmax(260px, 1fr));
      gap:16px; margin-bottom:24px;
    }
    .ref-card-item {
      background:#161b22; border:1px solid #21262d; border-radius:12px;
      padding:20px; transition:all 0.2s ease;
    }
    .ref-card-item:hover { border-color:#30363d; transform:translateY(-2px); box-shadow:0 4px 12px rgba(0,0,0,0.3); }
    .ref-card-item .ref-label {
      font-size:0.72rem; color:#8b949e; text-transform:uppercase;
      letter-spacing:0.8px; font-weight:600; margin-bottom:10px; display:block;
    }
    .ref-card-item .ref-value {
      display:flex; align-items:center; justify-content:space-between; gap:8px;
      padding:10px 14px; background:#0d1117; border:1px solid #30363d;
      border-radius:8px; font-family:'SFMono-Regular',Consolas,monospace;
      font-size:0.82rem; color:#e6edf3;
    }
    .ref-card-item .ref-value .copy-btn {
      padding:4px 10px; font-size:0.7rem; background:#21262d; color:#8b949e;
      border:1px solid #30363d; border-radius:4px; cursor:pointer; flex-shrink:0;
      transition:all 0.15s;
      font-family:-apple-system,BlinkMacSystemFont,'Segoe UI',Roboto,sans-serif;
    }
    .ref-card-item .ref-value .copy-btn:hover { background:#30363d; color:#e6edf3; }
    .ref-card-item .ref-value .copy-btn.copied { background:#064e3b; color:#34d399; border-color:#065f46; }

    .hint-box {
      background:#1c1f26; border:1px solid #30363d; border-radius:8px;
      padding:16px 18px; margin:8px 0 16px; font-size:0.85rem; color:#8b949e;
      line-height:1.9;
    }
    .hint-box strong { color:#e6edf3; }

    .demo-card {
      background:#161b22; border:1px solid #21262d; border-radius:12px;
      padding:28px; margin-bottom:28px; position:relative; overflow:hidden;
    }
    .demo-card::before {
      content:''; position:absolute; top:0; left:0; right:0; height:2px;
      background:linear-gradient(90deg, #7c3aed, #a78bfa, #7c3aed);
    }
    .demo-controls {
      display:flex; gap:12px; margin-bottom:16px; flex-wrap:wrap; align-items:center;
    }
    .demo-controls label { font-size:0.78rem; color:#8b949e; font-weight:600; text-transform:uppercase; letter-spacing:0.5px; }
    .demo-controls select {
      padding:8px 14px; background:#0d1117; border:1px solid #30363d;
      border-radius:8px; color:#e6edf3; font-size:0.85rem; font-family:inherit;
      transition:border-color 0.2s;
    }
    .demo-controls select:focus { outline:none; border-color:#7c3aed; }
    .demo-input-row {
      display:flex; gap:10px; margin-bottom:16px;
    }
    .demo-input-row input {
      flex:1; padding:12px 16px; background:#0d1117; border:1px solid #30363d;
      border-radius:8px; color:#e6edf3; font-size:0.95rem; font-family:inherit;
      transition:border-color 0.2s, box-shadow 0.2s;
    }
    .demo-input-row input:focus { outline:none; border-color:#7c3aed; box-shadow:0 0 0 3px rgba(124,58,237,0.15); }
    .demo-input-row input::placeholder { color:#484f58; }
    .demo-btn { padding:12px 24px; font-size:0.9rem; white-space:nowrap; border-radius:8px; font-weight:600; }
    .demo-btn-stop { background:#dc2626; }
    .demo-btn-stop:hover { background:#b91c1c; }
    .demo-btn:disabled { opacity:0.5; cursor:not-allowed; }
    .demo-output {
      background:#0d1117; border:1px solid #30363d; border-radius:8px;
      padding:20px; min-height:140px; max-height:400px; overflow-y:auto;
      font-family:'SFMono-Regular',Consolas,monospace; font-size:0.88rem;
      line-height:1.7; color:#e6edf3; white-space:pre-wrap; word-break:break-word;
      transition:border-color 0.3s, box-shadow 0.3s;
    }
    .demo-output.streaming { border-color:#7c3aed; box-shadow:0 0 0 1px rgba(124,58,237,0.2), 0 0 20px rgba(124,58,237,0.05); }
    .demo-placeholder { color:#484f58; font-style:italic; }
    .demo-error { color:#f87171; }
    .demo-error-title { font-weight:600; margin-bottom:6px; display:block; font-size:0.95rem; }
    .demo-error-detail { color:#8b949e; font-size:0.82rem; margin-top:4px; display:block; }
    .demo-loading {
      display:flex; align-items:center; gap:12px; color:#8b949e; padding:8px 0;
    }
    .demo-spinner {
      width:20px; height:20px; border:2px solid #30363d;
      border-top-color:#7c3aed; border-radius:50%;
      animation:spin 0.8s linear infinite;
    }
    @keyframes spin { to { transform:rotate(360deg); } }
    .demo-cursor {
      display:inline-block; width:2px; height:1.1em; background:#7c3aed;
      vertical-align:text-bottom; animation:blink 1s step-end infinite;
      margin-left:1px;
    }
    @keyframes blink { 50% { opacity:0; } }
    .demo-meta {
      display:flex; justify-content:space-between; align-items:center;
      margin-top:12px; padding-top:12px; border-top:1px solid #21262d;
      font-size:0.78rem; color:#8b949e;
    }
    .demo-toggle { display:flex; align-items:center; gap:6px; cursor:pointer; font-size:0.82rem; }
    .demo-toggle input { accent-color:#7c3aed; }
    .demo-status { font-family:'SFMono-Regular',Consolas,monospace; }

    @media (max-width:640px) {
      .integrate-inputs { flex-direction:column; padding:16px; }
      .integrate-inputs .field { min-width:100%; }
      .int-tabs { gap:0; overflow-x:auto; -webkit-overflow-scrolling:touch; flex-wrap:nowrap; }
      .int-tabs .tab { padding:8px 14px; font-size:0.75rem; white-space:nowrap; flex-shrink:0; }
      .int-panel { padding:16px; border-radius:0 0 8px 8px; }
      .code-block { font-size:0.75rem; padding:12px 40px 12px 12px; -webkit-overflow-scrolling:touch; }
      .ref-grid { grid-template-columns:1fr; }
      .demo-input-row { flex-direction:column; }
      .demo-controls { gap:8px; }
      .demo-card { padding:20px; }
      .section-heading { flex-wrap:wrap; font-size:1.05rem; }
      .section-heading .endpoint-label { margin-left:0; width:auto; }
      .demo-output { min-height:100px; padding:14px; }
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

  // Show cloud banner when logged in with pre-filled credentials
  if (cloudCfg && cloudCfg.apiKey) {
    const banner = $('#int-cloud-banner');
    if (banner) banner.style.display = 'block';
  }

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
    const block = btn.closest('.code-block') || btn.closest('.ref-value') || btn.closest('code');
    if (!block) return;
    const clone = block.cloneNode(true);
    clone.querySelectorAll('.copy-btn').forEach(b => b.remove());
    const text = clone.textContent.trim();
    navigator.clipboard.writeText(text).then(() => {
      const prev = btn.textContent;
      btn.textContent = '\u2713 Copied!';
      btn.classList.add('copied');
      setTimeout(() => { btn.textContent = prev; btn.classList.remove('copied'); }, 1500);
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

  // ── Live Demo ──
  const demoPrompt = $('#demo-prompt');
  const demoSend = $('#demo-send');
  const demoStop = $('#demo-stop');
  const demoOutput = $('#demo-output');
  const demoStatus = $('#demo-status');
  const demoStreamToggle = $('#demo-stream-toggle');
  const demoApiFormat = $('#demo-api-format');
  let demoAbort = null;

  demoPrompt.addEventListener('keydown', e => {
    if (e.key === 'Enter' && !demoSend.disabled) runDemo();
  });
  demoSend.addEventListener('click', runDemo);
  demoStop.addEventListener('click', () => {
    if (demoAbort) demoAbort.abort();
  });

  function buildDemoRequest(format, url, key, model, prompt, streaming) {
    if (format === 'messages') {
      return {
        endpoint: url + '/v1/messages',
        headers: {
          'Content-Type': 'application/json',
          'x-api-key': key,
          'anthropic-version': '2023-06-01',
        },
        body: { model: model, max_tokens: 1024, messages: [{ role: 'user', content: prompt }], stream: streaming },
      };
    } else if (format === 'responses') {
      return {
        endpoint: url + '/v1/responses',
        headers: { 'Content-Type': 'application/json', 'Authorization': 'Bearer ' + key },
        body: { model: model, input: prompt, stream: streaming },
      };
    }
    // default: chat completions
    return {
      endpoint: url + '/v1/chat/completions',
      headers: { 'Content-Type': 'application/json', 'Authorization': 'Bearer ' + key },
      body: { model: model, messages: [{ role: 'user', content: prompt }], stream: streaming },
    };
  }

  function extractNonStreamContent(format, data) {
    if (format === 'messages') return data.content?.[0]?.text || '(empty response)';
    if (format === 'responses') return data.output?.[0]?.content?.[0]?.text || data.output_text || '(empty response)';
    return data.choices?.[0]?.message?.content || '(empty response)';
  }

  function extractStreamDelta(format, line) {
    if (format === 'messages') {
      // Anthropic SSE: look for content_block_delta events
      if (!line.startsWith('data: ')) return null;
      const payload = line.slice(6).trim();
      if (payload === '[DONE]') return null;
      try {
        const chunk = JSON.parse(payload);
        if (chunk.type === 'content_block_delta' && chunk.delta?.text) return chunk.delta.text;
      } catch(_) {}
      return null;
    }
    if (format === 'responses') {
      // Responses API SSE: look for response.output_text.delta events
      if (!line.startsWith('data: ')) return null;
      const payload = line.slice(6).trim();
      if (payload === '[DONE]') return null;
      try {
        const chunk = JSON.parse(payload);
        if (chunk.type === 'response.output_text.delta' && chunk.delta) return chunk.delta;
      } catch(_) {}
      return null;
    }
    // Chat completions
    if (!line.startsWith('data: ')) return null;
    const payload = line.slice(6).trim();
    if (payload === '[DONE]') return null;
    try {
      const chunk = JSON.parse(payload);
      return chunk.choices?.[0]?.delta?.content || null;
    } catch(_) {}
    return null;
  }

  async function runDemo() {
    const url = sv();
    const key = ak();
    const model = mn();
    const format = demoApiFormat.value;
    if (!url || url === 'https://your-server.example.com') {
      demoOutput.innerHTML = '<span class="demo-placeholder">Enter your server URL above to try the live demo.</span>';
      return;
    }
    if (!key || key === 'your-api-key') {
      demoOutput.innerHTML = '<span class="demo-placeholder">Enter your API key above to try the live demo.</span>';
      return;
    }
    if (!model || model === 'your-model-name') {
      demoOutput.innerHTML = '<span class="demo-placeholder">Enter a model name above to try the live demo.</span>';
      return;
    }
    const prompt = demoPrompt.value.trim();
    if (!prompt) return;

    const streaming = demoStreamToggle.checked;
    demoAbort = new AbortController();
    demoSend.disabled = true;
    demoSend.style.display = 'none';
    demoStop.style.display = '';
    demoOutput.innerHTML = '<div class="demo-loading"><div class="demo-spinner"></div><span>' + (streaming ? 'Connecting to stream\u2026' : 'Sending request\u2026') + '</span></div>';
    demoOutput.classList.toggle('streaming', streaming);
    demoStatus.textContent = '';
    const t0 = performance.now();

    const req = buildDemoRequest(format, url, key, model, prompt, streaming);

    try {
      const res = await fetch(req.endpoint, {
        method: 'POST',
        headers: req.headers,
        body: JSON.stringify(req.body),
        signal: demoAbort.signal,
      });

      if (!res.ok) {
        const err = await res.text().catch(() => 'Unknown error');
        const hint = res.status === 401 ? 'Check your API key and try again.'
          : res.status === 404 ? 'Check your server URL \u2014 endpoint not found.'
          : res.status === 503 ? 'No workers available. Ensure a worker is connected.'
          : '';
        demoOutput.innerHTML = '<div class="demo-error"><span class="demo-error-title">HTTP ' + res.status + ' Error</span>' + escHtml(err.substring(0, 200)) + (hint ? '<br><span class="demo-error-detail">' + hint + '</span>' : '') + '</div>';
        demoStatus.textContent = 'Error \u00b7 ' + Math.round(performance.now() - t0) + 'ms';
        demoOutput.classList.remove('streaming');
        return;
      }

      if (!streaming) {
        const data = await res.json();
        const content = extractNonStreamContent(format, data);
        demoOutput.textContent = content;
        const ms = Math.round(performance.now() - t0);
        demoStatus.textContent = 'Done \u00b7 ' + ms + 'ms';
        demoOutput.classList.remove('streaming');
        return;
      }

      // SSE streaming
      demoOutput.textContent = '';
      const reader = res.body.getReader();
      const decoder = new TextDecoder();
      let buf = '';
      let tokens = 0;

      while (true) {
        const { done, value } = await reader.read();
        if (done) break;
        buf += decoder.decode(value, { stream: true });
        const lines = buf.split('\n');
        buf = lines.pop() || '';
        for (const line of lines) {
          const delta = extractStreamDelta(format, line);
          if (delta) {
            tokens++;
            const cursor = demoOutput.querySelector('.demo-cursor');
            if (cursor) cursor.remove();
            demoOutput.appendChild(document.createTextNode(delta));
            const c = document.createElement('span');
            c.className = 'demo-cursor';
            demoOutput.appendChild(c);
            demoOutput.scrollTop = demoOutput.scrollHeight;
          }
        }
        const ms = Math.round(performance.now() - t0);
        demoStatus.textContent = tokens + ' chunks \u00b7 ' + ms + 'ms';
      }
      const cursor = demoOutput.querySelector('.demo-cursor');
      if (cursor) cursor.remove();
      demoOutput.classList.remove('streaming');
      const ms = Math.round(performance.now() - t0);
      demoStatus.textContent = 'Done \u00b7 ' + tokens + ' chunks \u00b7 ' + ms + 'ms';
    } catch (e) {
      demoOutput.classList.remove('streaming');
      if (e.name === 'AbortError') {
        demoStatus.textContent = 'Stopped \u00b7 ' + Math.round(performance.now() - t0) + 'ms';
        const cursor = demoOutput.querySelector('.demo-cursor');
        if (cursor) cursor.remove();
      } else if (e.name === 'TypeError' && e.message.includes('Failed to fetch')) {
        demoOutput.innerHTML = '<div class="demo-error"><span class="demo-error-title">Connection Failed</span>Could not reach the server.<br><span class="demo-error-detail">This is usually a CORS issue or the server is unreachable. Check the URL and try again.</span></div>';
        demoStatus.textContent = 'Connection failed';
      } else {
        demoOutput.innerHTML = '<div class="demo-error"><span class="demo-error-title">Error</span>' + escHtml(e.message) + '</div>';
        demoStatus.textContent = 'Error';
      }
    } finally {
      demoSend.disabled = false;
      demoSend.style.display = '';
      demoStop.style.display = 'none';
      demoAbort = null;
    }
  }
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

    // ── Anthropic Messages API snippet templates ──

    let snippet_anthropic_curl = r"curl SERVER_URL/v1/messages \
  -H &quot;Content-Type: application/json&quot; \
  -H &quot;x-api-key: API_KEY&quot; \
  -H &quot;anthropic-version: 2023-06-01&quot; \
  -d &#x27;{
    &quot;model&quot;: &quot;MODEL_NAME&quot;,
    &quot;max_tokens&quot;: 1024,
    &quot;messages&quot;: [{&quot;role&quot;: &quot;user&quot;, &quot;content&quot;: &quot;Hello!&quot;}]
  }&#x27;";

    let snippet_anthropic_python = r"from anthropic import Anthropic

client = Anthropic(
    base_url=&quot;SERVER_URL/v1&quot;,
    api_key=&quot;API_KEY&quot;,
)

message = client.messages.create(
    model=&quot;MODEL_NAME&quot;,
    max_tokens=1024,
    messages=[{&quot;role&quot;: &quot;user&quot;, &quot;content&quot;: &quot;Hello!&quot;}],
)
print(message.content[0].text)";

    let snippet_anthropic_curl_stream = r"curl -N SERVER_URL/v1/messages \
  -H &quot;Content-Type: application/json&quot; \
  -H &quot;x-api-key: API_KEY&quot; \
  -H &quot;anthropic-version: 2023-06-01&quot; \
  -d &#x27;{
    &quot;model&quot;: &quot;MODEL_NAME&quot;,
    &quot;max_tokens&quot;: 1024,
    &quot;stream&quot;: true,
    &quot;messages&quot;: [{&quot;role&quot;: &quot;user&quot;, &quot;content&quot;: &quot;Hello!&quot;}]
  }&#x27;";

    let snippet_anthropic_python_stream = r"from anthropic import Anthropic

client = Anthropic(
    base_url=&quot;SERVER_URL/v1&quot;,
    api_key=&quot;API_KEY&quot;,
)

with client.messages.stream(
    model=&quot;MODEL_NAME&quot;,
    max_tokens=1024,
    messages=[{&quot;role&quot;: &quot;user&quot;, &quot;content&quot;: &quot;Hello!&quot;}],
) as stream:
    for text in stream.text_stream:
        print(text, end=&quot;&quot;, flush=True)
print()";

    // ── OpenAI Responses API snippet templates ──

    let snippet_responses_curl = r"curl SERVER_URL/v1/responses \
  -H &quot;Content-Type: application/json&quot; \
  -H &quot;Authorization: Bearer API_KEY&quot; \
  -d &#x27;{
    &quot;model&quot;: &quot;MODEL_NAME&quot;,
    &quot;input&quot;: &quot;Hello!&quot;
  }&#x27;";

    let snippet_responses_python = r"from openai import OpenAI

client = OpenAI(
    base_url=&quot;SERVER_URL/v1&quot;,
    api_key=&quot;API_KEY&quot;,
)

response = client.responses.create(
    model=&quot;MODEL_NAME&quot;,
    input=&quot;Hello!&quot;,
)
print(response.output_text)";

    let snippet_responses_curl_stream = r"curl -N SERVER_URL/v1/responses \
  -H &quot;Content-Type: application/json&quot; \
  -H &quot;Authorization: Bearer API_KEY&quot; \
  -d &#x27;{
    &quot;model&quot;: &quot;MODEL_NAME&quot;,
    &quot;input&quot;: &quot;Hello!&quot;,
    &quot;stream&quot;: true
  }&#x27;";

    let snippet_responses_python_stream = r"from openai import OpenAI

client = OpenAI(
    base_url=&quot;SERVER_URL/v1&quot;,
    api_key=&quot;API_KEY&quot;,
)

stream = client.responses.create(
    model=&quot;MODEL_NAME&quot;,
    input=&quot;Hello!&quot;,
    stream=True,
)
for event in stream:
    if event.type == &quot;response.output_text.delta&quot;:
        print(event.delta, end=&quot;&quot;, flush=True)
print()";

    // ── Streaming snippet templates ──

    let snippet_curl_stream = r"curl -N SERVER_URL/v1/chat/completions \
  -H &quot;Content-Type: application/json&quot; \
  -H &quot;Authorization: Bearer API_KEY&quot; \
  -d &#x27;{
    &quot;model&quot;: &quot;MODEL_NAME&quot;,
    &quot;stream&quot;: true,
    &quot;messages&quot;: [{&quot;role&quot;: &quot;user&quot;, &quot;content&quot;: &quot;Hello!&quot;}]
  }&#x27;";

    let snippet_python_stream = r"from openai import OpenAI

client = OpenAI(
    base_url=&quot;SERVER_URL/v1&quot;,
    api_key=&quot;API_KEY&quot;,
)

stream = client.chat.completions.create(
    model=&quot;MODEL_NAME&quot;,
    messages=[{&quot;role&quot;: &quot;user&quot;, &quot;content&quot;: &quot;Hello!&quot;}],
    stream=True,
)
for chunk in stream:
    delta = chunk.choices[0].delta.content
    if delta:
        print(delta, end=&quot;&quot;, flush=True)
print()";

    let snippet_node_stream = r"import OpenAI from &quot;openai&quot;;

const client = new OpenAI({
  baseURL: &quot;SERVER_URL/v1&quot;,
  apiKey: &quot;API_KEY&quot;,
});

const stream = await client.chat.completions.create({
  model: &quot;MODEL_NAME&quot;,
  messages: [{ role: &quot;user&quot;, content: &quot;Hello!&quot; }],
  stream: true,
});
for await (const chunk of stream) {
  const delta = chunk.choices?.[0]?.delta?.content;
  if (delta) process.stdout.write(delta);
}
console.log();";

    let snippet_go_stream = r"package main

import (
    &quot;context&quot;
    &quot;fmt&quot;
    &quot;io&quot;
    openai &quot;github.com/sashabaranov/go-openai&quot;
)

func main() {
    cfg := openai.DefaultConfig(&quot;API_KEY&quot;)
    cfg.BaseURL = &quot;SERVER_URL/v1&quot;
    client := openai.NewClientWithConfig(cfg)

    stream, _ := client.CreateChatCompletionStream(
        context.Background(),
        openai.ChatCompletionRequest{
            Model: &quot;MODEL_NAME&quot;,
            Messages: []openai.ChatCompletionMessage{
                {Role: &quot;user&quot;, Content: &quot;Hello!&quot;},
            },
        },
    )
    defer stream.Close()
    for {
        resp, err := stream.Recv()
        if err == io.EOF { break }
        if err != nil { break }
        fmt.Print(resp.Choices[0].Delta.Content)
    }
    fmt.Println()
}";

    let logged_in = cloud_config.is_some();
    let integrate_override_css = r"
    .content { padding: 32px 0; }
    .content h1 { font-size: 1.75rem; margin-bottom: 4px; }
    .subtitle { color: #8b949e; margin-bottom: 24px; font-size: 0.95rem; }
    code { font-family: 'SFMono-Regular', Consolas, monospace; }
    ";

    let extra_css = ["<style>", integrate_override_css, integrate_css, "</style>"].concat();

    let integrate_body = format!(
        r#"<h1>Integrate</h1>
      <p class="subtitle">Copy-paste snippets for your favorite tools, agents, and languages. Fill in your details below and all code blocks update automatically.</p>
      <p style="display:flex;gap:8px;flex-wrap:wrap;margin-bottom:24px;margin-top:-12px;">
        <span style="font-size:0.75rem;padding:3px 10px;border-radius:20px;background:rgba(124,58,237,0.1);border:1px solid rgba(124,58,237,0.2);color:#a78bfa;">OpenAI Compatible</span>
        <span style="font-size:0.75rem;padding:3px 10px;border-radius:20px;background:rgba(124,58,237,0.1);border:1px solid rgba(124,58,237,0.2);color:#a78bfa;">Anthropic Messages</span>
        <span style="font-size:0.75rem;padding:3px 10px;border-radius:20px;background:rgba(124,58,237,0.1);border:1px solid rgba(124,58,237,0.2);color:#a78bfa;">Responses API</span>
      </p>

      <div id="int-cloud-banner" style="display:none;padding:10px 16px;background:#0b1d0b;border:1px solid #064e3b;border-radius:8px;margin-bottom:16px;font-size:0.9rem;color:#34d399;">
        &#x2705; Logged in &mdash; your server URL and API key are pre-filled below.
      </div>

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

      <!-- ═══ OpenAI Chat Completions ═══ -->
      <div class="section-heading"><span class="icon">&#128187;</span> OpenAI Chat Completions <span class="endpoint-label">/v1/chat/completions</span></div>
      <div class="tab-section">
        <div class="int-tabs">
          <div class="tab active" data-tab="curl">curl</div>
          <div class="tab" data-tab="python">Python</div>
          <div class="tab" data-tab="node">Node.js</div>
          <div class="tab" data-tab="go">Go</div>
          <div class="tab" data-tab="stream-curl">curl (stream)</div>
          <div class="tab" data-tab="stream-python">Python (stream)</div>
          <div class="tab" data-tab="stream-node">Node.js (stream)</div>
          <div class="tab" data-tab="stream-go">Go (stream)</div>
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

          <div class="int-content" data-tab="stream-curl">
            <h3>curl <span style="color:#8b949e;font-weight:400;font-size:0.85rem;">with -N for unbuffered output</span></h3>
            <div class="code-block" data-snippet="{snippet_curl_stream}"><button class="copy-btn">Copy</button><span class="code-text"></span></div>
          </div>

          <div class="int-content" data-tab="stream-python">
            <h3>Python <span style="color:#8b949e;font-weight:400;font-size:0.85rem;">pip install openai</span></h3>
            <div class="code-block" data-snippet="{snippet_python_stream}"><button class="copy-btn">Copy</button><span class="code-text"></span></div>
          </div>

          <div class="int-content" data-tab="stream-node">
            <h3>Node.js / TypeScript <span style="color:#8b949e;font-weight:400;font-size:0.85rem;">npm install openai</span></h3>
            <div class="code-block" data-snippet="{snippet_node_stream}"><button class="copy-btn">Copy</button><span class="code-text"></span></div>
          </div>

          <div class="int-content" data-tab="stream-go">
            <h3>Go <span style="color:#8b949e;font-weight:400;font-size:0.85rem;">github.com/sashabaranov/go-openai</span></h3>
            <div class="code-block" data-snippet="{snippet_go_stream}"><button class="copy-btn">Copy</button><span class="code-text"></span></div>
          </div>

        </div>
      </div>

      <!-- ═══ Anthropic Messages API ═══ -->
      <div class="section-heading"><span class="icon">&#129504;</span> Anthropic Messages API <span class="endpoint-label">/v1/messages</span></div>
      <div class="tab-section">
        <div class="int-tabs">
          <div class="tab active" data-tab="anth-curl">curl</div>
          <div class="tab" data-tab="anth-python">Python</div>
          <div class="tab" data-tab="anth-curl-stream">curl (stream)</div>
          <div class="tab" data-tab="anth-python-stream">Python (stream)</div>
        </div>
        <div class="int-panel">

          <div class="int-content active" data-tab="anth-curl">
            <h3>curl</h3>
            <p>Uses the Anthropic <code style="font-size:0.85rem;">x-api-key</code> header and <code style="font-size:0.85rem;">anthropic-version</code> header.</p>
            <div class="code-block" data-snippet="{snippet_anthropic_curl}"><button class="copy-btn">Copy</button><span class="code-text"></span></div>
          </div>

          <div class="int-content" data-tab="anth-python">
            <h3>Python <span style="color:#8b949e;font-weight:400;font-size:0.85rem;">pip install anthropic</span></h3>
            <div class="code-block" data-snippet="{snippet_anthropic_python}"><button class="copy-btn">Copy</button><span class="code-text"></span></div>
          </div>

          <div class="int-content" data-tab="anth-curl-stream">
            <h3>curl <span style="color:#8b949e;font-weight:400;font-size:0.85rem;">streaming with -N</span></h3>
            <div class="code-block" data-snippet="{snippet_anthropic_curl_stream}"><button class="copy-btn">Copy</button><span class="code-text"></span></div>
          </div>

          <div class="int-content" data-tab="anth-python-stream">
            <h3>Python <span style="color:#8b949e;font-weight:400;font-size:0.85rem;">pip install anthropic</span></h3>
            <div class="code-block" data-snippet="{snippet_anthropic_python_stream}"><button class="copy-btn">Copy</button><span class="code-text"></span></div>
          </div>

        </div>
      </div>

      <!-- ═══ OpenAI Responses API ═══ -->
      <div class="section-heading"><span class="icon">&#128301;</span> OpenAI Responses API <span class="endpoint-label">/v1/responses</span></div>
      <div class="tab-section">
        <div class="int-tabs">
          <div class="tab active" data-tab="resp-api-curl">curl</div>
          <div class="tab" data-tab="resp-api-python">Python</div>
          <div class="tab" data-tab="resp-api-curl-stream">curl (stream)</div>
          <div class="tab" data-tab="resp-api-python-stream">Python (stream)</div>
        </div>
        <div class="int-panel">

          <div class="int-content active" data-tab="resp-api-curl">
            <h3>curl</h3>
            <p>The newer OpenAI Responses API uses a simpler <code style="font-size:0.85rem;">input</code> field instead of a messages array.</p>
            <div class="code-block" data-snippet="{snippet_responses_curl}"><button class="copy-btn">Copy</button><span class="code-text"></span></div>
          </div>

          <div class="int-content" data-tab="resp-api-python">
            <h3>Python <span style="color:#8b949e;font-weight:400;font-size:0.85rem;">pip install openai</span></h3>
            <div class="code-block" data-snippet="{snippet_responses_python}"><button class="copy-btn">Copy</button><span class="code-text"></span></div>
          </div>

          <div class="int-content" data-tab="resp-api-curl-stream">
            <h3>curl <span style="color:#8b949e;font-weight:400;font-size:0.85rem;">streaming with -N</span></h3>
            <div class="code-block" data-snippet="{snippet_responses_curl_stream}"><button class="copy-btn">Copy</button><span class="code-text"></span></div>
          </div>

          <div class="int-content" data-tab="resp-api-python-stream">
            <h3>Python <span style="color:#8b949e;font-weight:400;font-size:0.85rem;">pip install openai</span></h3>
            <div class="code-block" data-snippet="{snippet_responses_python_stream}"><button class="copy-btn">Copy</button><span class="code-text"></span></div>
          </div>

        </div>
      </div>

      <!-- ═══ Response Format ═══ -->
      <div class="section-heading"><span class="icon">&#128196;</span> Response Formats</div>
      <div class="tab-section">
        <div class="int-tabs">
          <div class="tab active" data-tab="resp-chat">Chat Completions</div>
          <div class="tab" data-tab="resp-chat-stream">Chat Completions (SSE)</div>
          <div class="tab" data-tab="resp-messages">Messages API</div>
          <div class="tab" data-tab="resp-messages-stream">Messages API (SSE)</div>
          <div class="tab" data-tab="resp-responses">Responses API</div>
          <div class="tab" data-tab="resp-responses-stream">Responses API (SSE)</div>
        </div>
        <div class="int-panel">

          <div class="int-content active" data-tab="resp-chat">
            <h3>Chat Completions <span style="color:#8b949e;font-weight:400;font-size:0.85rem;">/v1/chat/completions</span></h3>
            <p style="color:#8b949e;font-size:0.9rem;margin-bottom:12px;">Standard OpenAI chat completions response format.</p>
            <div class="code-block"><button class="copy-btn">Copy</button><span class="code-text">{{
  "id": "chatcmpl-abc123",
  "object": "chat.completion",
  "created": 1700000000,
  "model": "your-model-name",
  "choices": [
    {{
      "index": 0,
      "message": {{
        "role": "assistant",
        "content": "Hello! Here's a fun fact: ..."
      }},
      "finish_reason": "stop"
    }}
  ],
  "usage": {{
    "prompt_tokens": 12,
    "completion_tokens": 42,
    "total_tokens": 54
  }}
}}</span></div>
          </div>

          <div class="int-content" data-tab="resp-chat-stream">
            <h3>Chat Completions Streaming <span style="color:#8b949e;font-weight:400;font-size:0.85rem;">Server-Sent Events</span></h3>
            <p style="color:#8b949e;font-size:0.9rem;margin-bottom:12px;">When <code style="font-size:0.85rem;">stream: true</code>, returns SSE events with delta content.</p>
            <div class="code-block"><button class="copy-btn">Copy</button><span class="code-text">data: {{"id":"chatcmpl-abc123","object":"chat.completion.chunk","created":1700000000,"model":"your-model-name","choices":[{{"index":0,"delta":{{"role":"assistant","content":""}},"finish_reason":null}}]}}

data: {{"id":"chatcmpl-abc123","object":"chat.completion.chunk","created":1700000000,"model":"your-model-name","choices":[{{"index":0,"delta":{{"content":"Hello"}},"finish_reason":null}}]}}

data: {{"id":"chatcmpl-abc123","object":"chat.completion.chunk","created":1700000000,"model":"your-model-name","choices":[{{"index":0,"delta":{{"content":"!"}},"finish_reason":null}}]}}

data: {{"id":"chatcmpl-abc123","object":"chat.completion.chunk","created":1700000000,"model":"your-model-name","choices":[{{"index":0,"delta":{{}},"finish_reason":"stop"}}]}}

data: [DONE]</span></div>
          </div>

          <div class="int-content" data-tab="resp-messages">
            <h3>Anthropic Messages <span style="color:#8b949e;font-weight:400;font-size:0.85rem;">/v1/messages</span></h3>
            <p style="color:#8b949e;font-size:0.9rem;margin-bottom:12px;">Standard Anthropic Messages API response format.</p>
            <div class="code-block"><button class="copy-btn">Copy</button><span class="code-text">{{
  "id": "msg_abc123",
  "type": "message",
  "role": "assistant",
  "content": [
    {{
      "type": "text",
      "text": "Hello! Here's a fun fact: ..."
    }}
  ],
  "model": "your-model-name",
  "stop_reason": "end_turn",
  "usage": {{
    "input_tokens": 10,
    "output_tokens": 42
  }}
}}</span></div>
          </div>

          <div class="int-content" data-tab="resp-messages-stream">
            <h3>Anthropic Messages Streaming <span style="color:#8b949e;font-weight:400;font-size:0.85rem;">Server-Sent Events</span></h3>
            <p style="color:#8b949e;font-size:0.9rem;margin-bottom:12px;">When <code style="font-size:0.85rem;">stream: true</code>, returns named SSE events with content deltas.</p>
            <div class="code-block"><button class="copy-btn">Copy</button><span class="code-text">event: message_start
data: {{"type":"message_start","message":{{"id":"msg_abc123","type":"message","role":"assistant","content":[],"model":"your-model-name"}}}}

event: content_block_start
data: {{"type":"content_block_start","index":0,"content_block":{{"type":"text","text":""}}}}

event: content_block_delta
data: {{"type":"content_block_delta","index":0,"delta":{{"type":"text_delta","text":"Hello!"}}}}

event: content_block_stop
data: {{"type":"content_block_stop","index":0}}

event: message_delta
data: {{"type":"message_delta","delta":{{"stop_reason":"end_turn"}},"usage":{{"output_tokens":42}}}}

event: message_stop
data: {{"type":"message_stop"}}</span></div>
          </div>

          <div class="int-content" data-tab="resp-responses">
            <h3>OpenAI Responses <span style="color:#8b949e;font-weight:400;font-size:0.85rem;">/v1/responses</span></h3>
            <p style="color:#8b949e;font-size:0.9rem;margin-bottom:12px;">Newer OpenAI Responses API format.</p>
            <div class="code-block"><button class="copy-btn">Copy</button><span class="code-text">{{
  "id": "resp_abc123",
  "object": "response",
  "created_at": 1700000000,
  "model": "your-model-name",
  "output": [
    {{
      "type": "message",
      "role": "assistant",
      "content": [
        {{
          "type": "output_text",
          "text": "Hello! Here's a fun fact: ..."
        }}
      ]
    }}
  ],
  "usage": {{
    "input_tokens": 10,
    "output_tokens": 42,
    "total_tokens": 52
  }}
}}</span></div>
          </div>

          <div class="int-content" data-tab="resp-responses-stream">
            <h3>Responses API Streaming <span style="color:#8b949e;font-weight:400;font-size:0.85rem;">Server-Sent Events</span></h3>
            <p style="color:#8b949e;font-size:0.9rem;margin-bottom:12px;">When <code style="font-size:0.85rem;">stream: true</code>, returns named SSE events.</p>
            <div class="code-block"><button class="copy-btn">Copy</button><span class="code-text">event: response.created
data: {{"type":"response.created","response":{{"id":"resp_abc123","object":"response","status":"in_progress"}}}}

event: response.output_text.delta
data: {{"type":"response.output_text.delta","delta":"Hello"}}

event: response.output_text.delta
data: {{"type":"response.output_text.delta","delta":"!"}}

event: response.output_text.done
data: {{"type":"response.output_text.done","text":"Hello!"}}

event: response.completed
data: {{"type":"response.completed","response":{{"id":"resp_abc123","object":"response","status":"completed"}}}}</span></div>
          </div>

        </div>
      </div>

      <!-- ═══ Live Demo ═══ -->
      <div class="section-heading"><span class="icon">&#128640;</span> Try It Live</div>
      <div class="demo-card">
        <div class="demo-controls">
          <label>API Format:</label>
          <select id="demo-api-format">
            <option value="chat">Chat Completions</option>
            <option value="messages">Anthropic Messages</option>
            <option value="responses">Responses API</option>
          </select>
        </div>
        <div class="demo-input-row">
          <input id="demo-prompt" type="text" placeholder="Type a message..." value="Say hello and tell me a fun fact.">
          <button id="demo-send" class="btn demo-btn">Send</button>
          <button id="demo-stop" class="btn demo-btn demo-btn-stop" style="display:none;">Stop</button>
        </div>
        <div id="demo-output" class="demo-output"><span class="demo-placeholder">Response will stream here&hellip;</span></div>
        <div class="demo-meta">
          <span id="demo-status" class="demo-status"></span>
          <label class="demo-toggle"><input type="checkbox" id="demo-stream-toggle" checked> Stream</label>
        </div>
      </div>

      <!-- ═══ Quick Reference ═══ -->
      <div class="section-heading"><span class="icon">&#128218;</span> Quick Reference</div>
      <div class="ref-grid">
        <div class="ref-card-item" data-ref="SERVER_URL/v1">
          <span class="ref-label">API Base URL</span>
          <div class="ref-value"><span class="ref-val">SERVER_URL/v1</span><button class="copy-btn">Copy</button></div>
        </div>
        <div class="ref-card-item" data-ref="Bearer API_KEY">
          <span class="ref-label">Authorization Header</span>
          <div class="ref-value"><span class="ref-val">Bearer API_KEY</span><button class="copy-btn">Copy</button></div>
        </div>
        <div class="ref-card-item">
          <span class="ref-label">Supported Endpoints</span>
          <div class="ref-value"><span class="ref-val" style="font-size:0.78rem;">/v1/chat/completions &middot; /v1/messages &middot; /v1/responses &middot; /v1/models</span><button class="copy-btn">Copy</button></div>
        </div>
        <div class="ref-card-item">
          <span class="ref-label">Documentation</span>
          <div class="ref-value"><a href="https://ericflo.github.io/modelrelay/" target="_blank" rel="noopener" style="color:#a78bfa;text-decoration:none;font-size:0.82rem;">Full API reference &amp; guides &#x2192;</a></div>
        </div>
      </div>

"#
    );

    let cloud_cfg_script = cloud_config_script(cloud_config);
    let extra_body_end = format!("{cloud_cfg_script}<script>{integrate_js}</script>");

    page_shell_custom(
        "Integrate",
        &integrate_body,
        logged_in,
        &extra_css,
        &extra_body_end,
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

/// Full-control page shell: caller provides the complete body HTML (including headings),
/// optional extra CSS (injected after the base `<style>` block), and optional extra
/// content appended before `</body>` (typically `<script>` tags).
///
/// `logged_in` controls the nav links: when `true`, shows Dashboard + Setup + Integrate +
/// Docs + Log out; when `false`, shows Pricing + Docs + Log in + Sign up + GitHub.
#[must_use]
#[allow(clippy::too_many_lines)]
pub fn page_shell_custom(
    title: &str,
    body_html: &str,
    logged_in: bool,
    extra_css: &str,
    extra_body_end: &str,
) -> String {
    let title_lower = title.to_lowercase();
    let active_dashboard = if title_lower.contains("dashboard") {
        " active"
    } else {
        ""
    };
    let active_setup = if title_lower.contains("setup") {
        " active"
    } else {
        ""
    };
    let active_integrate = if title_lower.contains("integrat") {
        " active"
    } else {
        ""
    };
    let active_pricing = if title_lower.contains("pricing") {
        " active"
    } else {
        ""
    };

    let nav_links = if logged_in {
        format!(
            r#"<a href="/dashboard" class="nav-link{active_dashboard}">Dashboard</a>
            <a href="/setup" class="nav-link{active_setup}">Setup</a>
            <a href="/integrate" class="nav-link{active_integrate}">Integrate</a>
            <a href="https://ericflo.github.io/modelrelay/" target="_blank" rel="noopener" class="nav-link">Docs</a>
            <form method="POST" action="/logout"><button type="submit">Log out</button></form>"#
        )
    } else {
        format!(
            r#"<a href="/pricing" class="nav-link{active_pricing}">Pricing</a>
            <a href="https://ericflo.github.io/modelrelay/" target="_blank" rel="noopener" class="nav-link">Docs</a>
            <a href="/login" class="nav-link">Log in</a>
            <a class="btn-signup" href="/signup">Sign up</a>
            <a href="https://github.com/ericflo/modelrelay" target="_blank" rel="noopener" class="nav-link">GitHub</a>"#
        )
    };

    format!(
        r##"<!DOCTYPE html>
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

    /* Skip navigation */
    .skip-nav {{
      position: absolute; top: -100%; left: 16px; z-index: 1000;
      background: #7c3aed; color: #fff; padding: 8px 16px; border-radius: 0 0 8px 8px;
      font-size: 0.9rem; font-weight: 600; text-decoration: none;
      transition: top 0.2s;
    }}
    .skip-nav:focus {{ top: 0; text-decoration: none; }}

    /* Focus indicators */
    :focus-visible {{
      outline: 2px solid #7c3aed;
      outline-offset: 2px;
    }}
    input:focus-visible {{ outline: none; }}

    .container {{ max-width: 900px; margin: 0 auto; padding: 0 24px; }}

    /* Nav */
    nav {{ padding: 20px 0; border-bottom: 1px solid #21262d; position: relative; }}
    nav .container {{ display: flex; justify-content: space-between; align-items: center; }}
    .logo {{ font-size: 1.25rem; font-weight: 700; color: #e6edf3; }}
    .logo:hover {{ text-decoration: none; }}
    .logo span {{ color: #7c3aed; }}
    .nav-links {{ display: flex; align-items: center; gap: 0; }}
    .nav-links .nav-link {{ color: #8b949e; font-size: 0.9rem; margin-left: 16px; padding: 4px 0; border-bottom: 2px solid transparent; transition: color 0.2s, border-color 0.2s; }}
    .nav-links .nav-link:hover {{ color: #e6edf3; text-decoration: none; }}
    .nav-links .nav-link.active {{ color: #e6edf3; border-bottom-color: #7c3aed; }}
    .nav-links form {{ display: inline; }}
    .nav-links button {{ background: none; border: none; color: #8b949e; font-size: 0.9rem; cursor: pointer; margin-left: 16px; font-family: inherit; transition: color 0.2s; }}
    .nav-links button:hover {{ color: #e6edf3; }}
    .nav-links .btn-signup {{ background: #7c3aed; color: #fff; padding: 6px 16px; border-radius: 6px; font-weight: 600; font-size: 0.9rem; margin-left: 16px; border-bottom: none; }}
    .nav-links .btn-signup:hover {{ background: #6d28d9; color: #fff; text-decoration: none; }}

    /* Hamburger */
    .nav-hamburger {{
      display: none; background: none; border: none; cursor: pointer; padding: 4px;
      flex-direction: column; justify-content: center; align-items: center; gap: 5px;
    }}
    .nav-hamburger span {{
      display: block; width: 22px; height: 2px; background: #8b949e; border-radius: 1px;
      transition: transform 0.25s, opacity 0.25s;
    }}
    .nav-hamburger:hover span {{ background: #e6edf3; }}
    .nav-hamburger.open span:nth-child(1) {{ transform: translateY(7px) rotate(45deg); }}
    .nav-hamburger.open span:nth-child(2) {{ opacity: 0; }}
    .nav-hamburger.open span:nth-child(3) {{ transform: translateY(-7px) rotate(-45deg); }}

    .content {{ padding: 60px 0; }}
    .content h1 {{ font-size: 2rem; margin-bottom: 24px; }}

    .card {{
      background: #161b22; border: 1px solid #21262d; border-radius: 12px;
      padding: 32px; margin-bottom: 24px;
    }}
    .card h2 {{ font-size: 1.2rem; margin-bottom: 12px; color: #e6edf3; }}
    .card p {{ color: #8b949e; }}
    .back-link {{ margin-top: 16px; }}

    /* Auth split layout */
    .auth-split {{
      display: grid; grid-template-columns: 1fr 1fr; gap: 0;
      min-height: calc(100vh - 200px); align-items: center;
    }}
    .auth-value {{
      padding: 48px 48px 48px 0;
    }}
    .auth-value-headline {{
      font-size: 2.2rem; font-weight: 700; line-height: 1.2;
      color: #e6edf3; margin: 0 0 16px 0;
    }}
    .auth-value-sub {{
      font-size: 1.05rem; color: #8b949e; line-height: 1.6; margin: 0 0 32px 0;
    }}
    .auth-benefits {{
      list-style: none; padding: 0; margin: 0 0 32px 0;
    }}
    .auth-benefits li {{
      display: flex; align-items: center; gap: 10px;
      color: #c9d1d9; font-size: 0.95rem; padding: 8px 0;
    }}
    .auth-benefits li svg {{ flex-shrink: 0; }}
    .auth-trust {{
      font-size: 0.85rem; color: #484f58; margin: 0;
    }}
    .auth-form-panel {{
      display: flex; align-items: center; justify-content: center;
      padding: 48px 0 48px 48px;
      border-left: 1px solid #21262d;
    }}
    .auth-form-inner {{
      width: 100%; max-width: 400px;
    }}
    .auth-form-inner h2 {{
      font-size: 1.4rem; font-weight: 700; color: #e6edf3; margin: 0 0 6px 0;
    }}
    .auth-form-hint {{
      color: #8b949e; font-size: 0.9rem; margin: 0 0 24px 0;
    }}
    .auth-no-cc {{
      text-align: center; color: #3fb950; font-size: 0.85rem; margin: 12px 0 0 0; font-weight: 500;
    }}

    .auth-form .form-group {{ margin-bottom: 16px; }}
    .auth-form label {{ display: block; font-size: 0.9rem; color: #8b949e; margin-bottom: 6px; font-weight: 500; }}
    .auth-form input {{
      width: 100%; padding: 11px 14px; background: #0d1117; border: 1px solid #30363d;
      border-radius: 8px; color: #e6edf3; font-size: 0.95rem;
      transition: border-color 0.2s, box-shadow 0.2s;
    }}
    .auth-form input:focus {{ outline: none; border-color: #7c3aed; box-shadow: 0 0 0 3px rgba(124,58,237,0.15); }}
    .auth-submit {{
      width: 100%; margin-top: 8px; padding: 12px 20px; font-size: 1rem; position: relative;
      transition: background 0.2s, opacity 0.2s;
    }}
    .auth-submit:disabled {{ opacity: 0.7; cursor: not-allowed; }}
    .auth-submit.loading .spinner {{
      display: inline-block; width: 14px; height: 14px;
      border: 2px solid rgba(255,255,255,0.3); border-top-color: #fff;
      border-radius: 50%; animation: spin 0.6s linear infinite;
      margin-right: 8px; vertical-align: middle;
    }}
    @keyframes spin {{ to {{ transform: rotate(360deg); }} }}

    .password-wrapper {{
      position: relative;
    }}
    .password-wrapper input {{ padding-right: 44px; }}
    .password-toggle {{
      position: absolute; right: 10px; top: 50%; transform: translateY(-50%);
      background: none; border: none; cursor: pointer; padding: 4px;
      display: flex; align-items: center; justify-content: center;
    }}
    .password-toggle:hover svg {{ stroke: #e6edf3; }}

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

    /* Footer */
    footer {{ padding: 48px 0; border-top: 1px solid #21262d; }}
    .footer-content {{ display: flex; justify-content: space-between; align-items: center; flex-wrap: wrap; gap: 16px; }}
    .footer-left {{ color: #484f58; font-size: 0.85rem; }}
    .footer-tagline {{ color: #30363d; font-size: 0.8rem; margin-top: 4px; }}
    .footer-links {{ display: flex; gap: 20px; }}
    .footer-links a {{ color: #8b949e; font-size: 0.85rem; }}
    .footer-links a:hover {{ color: #e6edf3; text-decoration: none; }}

    /* Tablet */
    @media (max-width: 768px) {{
      .content {{ padding: 40px 0; }}
      .content h1 {{ font-size: 1.6rem; }}
      .card {{ padding: 24px; }}
      .info-table td:first-child {{ width: 120px; }}
      .auth-split {{ grid-template-columns: 1fr; }}
      .auth-value {{ padding: 32px 0 24px 0; }}
      .auth-value-headline {{ font-size: 1.8rem; }}
      .auth-form-panel {{ border-left: none; border-top: 1px solid #21262d; padding: 32px 0 0 0; }}
      .nav-hamburger {{ display: flex; }}
      .nav-links {{
        display: none; position: absolute; top: 100%; left: 0; right: 0;
        background: #161b22; border-bottom: 1px solid #21262d;
        flex-direction: column; padding: 16px 24px; gap: 0; z-index: 100;
      }}
      .nav-links.open {{ display: flex; }}
      .nav-links .nav-link {{ margin-left: 0; padding: 10px 0; font-size: 0.95rem; border-bottom: none; }}
      .nav-links .nav-link.active {{ color: #7c3aed; }}
      .nav-links form {{ display: block; }}
      .nav-links button {{ margin-left: 0; padding: 10px 0; font-size: 0.95rem; }}
      .nav-links .btn-signup {{ margin-left: 0; margin-top: 8px; display: inline-block; text-align: center; }}
      .footer-content {{ flex-direction: column; text-align: center; }}
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
      .auth-value {{ padding: 24px 0 20px 0; }}
      .auth-value-headline {{ font-size: 1.5rem; }}
      .auth-value-sub {{ font-size: 0.95rem; margin-bottom: 20px; }}
      .auth-benefits {{ margin-bottom: 20px; }}
      .auth-form-panel {{ padding: 24px 0 0 0; }}
      .auth-form-inner h2 {{ font-size: 1.2rem; }}
    }}
  </style>
  {extra_css}
</head>
<body>
  <a href="#main-content" class="skip-nav">Skip to content</a>
  <nav role="navigation" aria-label="Main navigation">
    <div class="container">
      <a href="/" class="logo">Model<span>Relay</span></a>
      <button class="nav-hamburger" aria-label="Toggle navigation" onclick="this.classList.toggle('open');this.parentElement.querySelector('.nav-links').classList.toggle('open')">
        <span></span><span></span><span></span>
      </button>
      <div class="nav-links">
        {nav_links}
      </div>
    </div>
  </nav>

  <main id="main-content" class="content">
    <div class="container">
      {body_html}
    </div>
  </main>

  <footer role="contentinfo">
    <div class="container">
      <div class="footer-content">
        <div class="footer-left">
          &copy; 2026 ModelRelay
          <div class="footer-tagline">Your GPU workers, our relay. Inference without the infrastructure.</div>
        </div>
        <div class="footer-links">
          <a href="/pricing">Pricing</a>
          <a href="/integrate">Integration</a>
          <a href="https://ericflo.github.io/modelrelay/" target="_blank" rel="noopener">Docs</a>
          <a href="https://github.com/ericflo/modelrelay" target="_blank" rel="noopener">GitHub</a>
        </div>
      </div>
    </div>
  </footer>
  {extra_body_end}
</body>
</html>"##
    )
}

/// Convenience wrapper: auto-adds `<h1>` before body content.
/// Most routes use this. For custom headers, use [`page_shell_custom`] directly.
#[must_use]
pub fn page_shell(title: &str, body_content: &str, logged_in: bool) -> String {
    let body_html = format!("<h1>{title}</h1>\n      {body_content}");
    page_shell_custom(title, &body_html, logged_in, "", "")
}
