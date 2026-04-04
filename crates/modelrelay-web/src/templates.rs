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
        el.innerHTML = '<div class="empty-state">No workers connected.<br><a href="https://github.com/ericflo/llm-worker-proxy#quickstart" target="_blank">How to connect a worker &rarr;</a></div>';
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
      </div>
    </div>
  </nav>

  <section class="content">
    <div class="container">
      <h1>Dashboard</h1>
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
