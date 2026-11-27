use super::page_shell_custom;

/// Build the live OSS admin dashboard page (used by `modelrelay-web` standalone
/// and mounted under `/admin/dashboard`).
#[must_use]
#[allow(clippy::too_many_lines)]
pub fn dashboard_page() -> String {
    let dashboard_css = r"
    .dash-top {
      display: flex; justify-content: space-between; align-items: flex-start;
      gap: 16px; flex-wrap: wrap; margin-bottom: 24px;
    }
    .dash-top h1 { font-size: 1.9rem; font-weight: 800; letter-spacing: -0.03em; margin-bottom: 4px; }
    .dash-top p { color: var(--text-secondary); font-size: 0.92rem; }
    .dash-actions { display: flex; gap: 10px; flex-wrap: wrap; }

    .config-bar {
      display: flex; gap: 10px; align-items: center; flex-wrap: wrap;
      padding: 14px 18px; background: var(--bg-surface); border: 1px solid var(--border);
      border-radius: var(--radius-md); margin-bottom: 22px;
    }
    .config-bar label { font-size: 0.78rem; font-weight: 600; color: var(--text-muted); text-transform: uppercase; letter-spacing: 0.06em; }
    .config-bar input {
      padding: 7px 11px; background: var(--bg-input); border: 1px solid var(--border-strong);
      border-radius: 7px; color: var(--text-primary); font-family: var(--font-mono); font-size: 0.84rem;
      min-width: 190px;
    }
    .config-bar input:focus { border-color: var(--accent); box-shadow: 0 0 0 3px var(--accent-soft); }
    .config-bar .status { margin-left: auto; font-size: 0.82rem; font-weight: 600; }
    .config-bar .status.ok { color: var(--success); }
    .config-bar .status.fail { color: var(--danger); }

    .kpi-grid {
      display: grid; grid-template-columns: repeat(5, 1fr); gap: 12px; margin-bottom: 24px;
    }
    .kpi-card {
      background: var(--bg-surface); border: 1px solid var(--border);
      border-radius: var(--radius-md); padding: 16px 18px;
    }
    .kpi-label {
      font-size: 0.7rem; font-weight: 700; text-transform: uppercase;
      letter-spacing: 0.08em; color: var(--text-muted);
    }
    .kpi-val {
      font-size: 1.45rem; font-weight: 700; margin-top: 6px;
      font-variant-numeric: tabular-nums; color: var(--text-strong);
    }
    .kpi-val.ok { color: var(--success); }
    .kpi-val.warn { color: var(--warning); }
    .kpi-val.err { color: var(--danger); }

    .panel {
      background: var(--bg-surface); border: 1px solid var(--border);
      border-radius: var(--radius-lg); padding: 24px; margin-bottom: 20px;
      box-shadow: var(--shadow-card);
    }
    .panel-head {
      display: flex; align-items: center; justify-content: space-between; gap: 12px;
      margin-bottom: 16px;
    }
    .panel h2 {
      font-size: 1.05rem; font-weight: 700; margin: 0; color: var(--text-strong);
      display: flex; align-items: center; gap: 8px;
    }
    .dot { width: 8px; height: 8px; border-radius: 50%; background: var(--success); display: inline-block; }

    .empty-state { color: var(--text-muted); text-align: center; padding: 24px 0; font-size: 0.9rem; }
    table.data { width: 100%; border-collapse: collapse; font-size: 0.88rem; }
    table.data th {
      text-align: left; color: var(--text-muted); font-weight: 600; font-size: 0.72rem;
      text-transform: uppercase; letter-spacing: 0.07em;
      padding: 8px 12px 10px 0; border-bottom: 1px solid var(--border);
    }
    table.data td { padding: 11px 12px 11px 0; border-bottom: 1px solid var(--border); color: var(--text-primary); }
    table.data tr:last-child td { border-bottom: none; }

    .model-tag {
      display: inline-block; padding: 2px 8px; margin: 2px 4px 2px 0; border-radius: 5px;
      background: var(--accent-soft); border: 1px solid var(--accent-border);
      font-size: 0.76rem; font-family: var(--font-mono); color: var(--accent-fg);
    }
    .load-bar { display: inline-flex; align-items: center; gap: 8px; font-variant-numeric: tabular-nums; font-size: 0.84rem; }
    .load-track { width: 64px; height: 6px; border-radius: 999px; background: var(--bg-elevated); overflow: hidden; }
    .load-fill { height: 100%; background: var(--accent-bright); border-radius: 999px; }

    .stat-row { display: flex; align-items: center; gap: 12px; margin-bottom: 10px; }
    .stat-label { min-width: 140px; color: var(--text-secondary); font-size: 0.86rem; font-family: var(--font-mono); }
    .stat-bar { flex: 1; height: 10px; background: var(--bg-elevated); border-radius: 999px; overflow: hidden; }
    .stat-fill { height: 100%; background: linear-gradient(90deg, var(--accent), var(--cyan)); border-radius: 999px; transition: width 0.25s; }
    .stat-num { min-width: 32px; text-align: right; font-weight: 700; font-variant-numeric: tabular-nums; font-size: 0.86rem; }

    .key-create { display: flex; gap: 8px; align-items: center; flex-wrap: wrap; margin-bottom: 16px; }
    .key-create input {
      padding: 8px 12px; background: var(--bg-input); border: 1px solid var(--border-strong);
      border-radius: 8px; color: var(--text-primary); font-size: 0.88rem; width: 240px;
    }
    .secret-box {
      margin-bottom: 16px; padding: 14px 76px 14px 16px; background: var(--bg-code);
      border: 1px solid var(--accent-border); border-radius: var(--radius-md);
      font-family: var(--font-mono); font-size: 0.84rem; color: #a5b4fc;
      word-break: break-all; position: relative;
    }
    .secret-box .warn { display: block; margin-top: 6px; font-family: var(--font-sans); font-size: 0.78rem; color: var(--warning); }
    .secret-box .copy-btn { position: absolute; top: 10px; right: 10px; }

    @media (max-width: 820px) {
      .kpi-grid { grid-template-columns: repeat(2, 1fr); }
      table.data { display: block; overflow-x: auto; }
    }
    @media (max-width: 520px) {
      .kpi-grid { grid-template-columns: 1fr; }
      .config-bar input, .key-create input { width: 100%; }
    }
    ";

    let dashboard_js = r#"
(function() {
  const POLL_MS = 4000;
  let adminToken = localStorage.getItem('mr_admin_token') || '';
  let serverUrl = localStorage.getItem('mr_server_url') || '';
  const $ = s => document.querySelector(s);

  function baseUrl() { return serverUrl || window.location.origin; }
  function authHeaders() {
    const h = { 'Content-Type': 'application/json' };
    if (adminToken) h['Authorization'] = 'Bearer ' + adminToken;
    return h;
  }
  function fmtDuration(secs) {
    if (secs < 60) return Math.floor(secs) + 's';
    if (secs < 3600) return Math.floor(secs / 60) + 'm ' + Math.floor(secs % 60) + 's';
    return Math.floor(secs / 3600) + 'h ' + Math.floor((secs % 3600) / 60) + 'm';
  }
  function fmtTs(ts) { return ts ? new Date(ts * 1000).toLocaleString() : '—'; }
  function escHtml(s) { const d = document.createElement('div'); d.textContent = s; return d.innerHTML; }

  function initConfig() {
    const tokenInput = $('#cfg-token');
    const urlInput = $('#cfg-url');
    tokenInput.value = adminToken;
    urlInput.value = serverUrl;
    tokenInput.addEventListener('change', () => { adminToken = tokenInput.value.trim(); localStorage.setItem('mr_admin_token', adminToken); pollAll(); });
    urlInput.addEventListener('change', () => { serverUrl = urlInput.value.trim().replace(/\/+$/, ''); localStorage.setItem('mr_server_url', serverUrl); pollAll(); });
  }

  async function pollHealth() {
    try {
      const r = await fetch(baseUrl() + '/health');
      if (!r.ok) throw new Error(r.status);
      const d = await r.json();
      $('#h-status').textContent = d.status || '—';
      $('#h-status').className = 'kpi-val ' + (d.status === 'ok' ? 'ok' : 'warn');
      $('#h-version').textContent = d.version || '—';
      $('#h-workers').textContent = d.workers_connected ?? '—';
      $('#h-workers').className = 'kpi-val ' + ((d.workers_connected || 0) > 0 ? 'ok' : 'warn');
      $('#h-queue').textContent = d.queue_depth ?? '—';
      $('#h-queue').className = 'kpi-val ' + ((d.queue_depth || 0) > 0 ? 'warn' : 'ok');
      $('#h-uptime').textContent = fmtDuration(d.uptime_secs || 0);
      $('#cfg-status').textContent = 'Connected';
      $('#cfg-status').className = 'status ok';
    } catch (_) {
      $('#h-status').textContent = 'error';
      $('#h-status').className = 'kpi-val err';
      $('#cfg-status').textContent = 'Disconnected';
      $('#cfg-status').className = 'status fail';
    }
  }

  async function pollWorkers() {
    const el = $('#workers-body');
    if (!adminToken) { el.innerHTML = '<div class="empty-state">Enter your admin token above to inspect workers.</div>'; return; }
    try {
      const r = await fetch(baseUrl() + '/admin/workers', { headers: authHeaders() });
      if (r.status === 403) { el.innerHTML = '<div class="empty-state" style="color:var(--danger);">Invalid admin token.</div>'; return; }
      if (!r.ok) throw new Error(r.status);
      const workers = (await r.json()).workers || [];
      if (!workers.length) {
        el.innerHTML = '<div class="empty-state">No workers connected.<br><a href="/setup" class="btn btn-sm" style="margin-top:10px;">Connect a worker &rarr;</a></div>';
        return;
      }
      let html = '<table class="data"><thead><tr><th>Worker</th><th>Models</th><th>Load</th><th>Status</th></tr></thead><tbody>';
      for (const w of workers) {
        const models = (w.models || []).map(m => '<span class="model-tag">' + (m === '*' ? 'All models' : escHtml(m)) + '</span>').join('');
        const pct = w.max_concurrent ? Math.min(100, Math.round(((w.in_flight_count || 0) / w.max_concurrent) * 100)) : 0;
        const load = '<span class="load-bar"><span class="load-track"><span class="load-fill" style="width:' + pct + '%"></span></span>' + (w.in_flight_count || 0) + '/' + (w.max_concurrent || 0) + '</span>';
        const status = w.is_draining ? '<span class="badge badge-warn">Draining</span>' : '<span class="badge badge-active">Active</span>';
        html += '<tr><td>' + escHtml(w.worker_name || w.worker_id) + '</td><td>' + models + '</td><td>' + load + '</td><td>' + status + '</td></tr>';
      }
      el.innerHTML = html + '</tbody></table>';
    } catch (_) {
      el.innerHTML = '<div class="empty-state" style="color:var(--danger);">Failed to load workers.</div>';
    }
  }

  async function pollStats() {
    const el = $('#stats-body');
    if (!adminToken) { el.innerHTML = '<div class="empty-state">Enter your admin token above to inspect queue stats.</div>'; return; }
    try {
      const r = await fetch(baseUrl() + '/admin/stats', { headers: authHeaders() });
      if (r.status === 403) { el.innerHTML = '<div class="empty-state" style="color:var(--danger);">Invalid admin token.</div>'; return; }
      if (!r.ok) throw new Error(r.status);
      const d = await r.json();
      const qd = d.queue_depth || {};
      const providers = Object.keys(qd);
      if (!providers.length) { el.innerHTML = '<div class="empty-state">Queue is empty &mdash; all requests dispatched immediately.</div>'; return; }
      const maxQ = Math.max(1, ...providers.map(p => qd[p]));
      el.innerHTML = providers.map(p => {
        const pct = Math.round((qd[p] / maxQ) * 100);
        return '<div class="stat-row"><span class="stat-label">' + escHtml(p) + '</span><div class="stat-bar"><div class="stat-fill" style="width:' + pct + '%"></div></div><span class="stat-num">' + qd[p] + '</span></div>';
      }).join('');
    } catch (_) {
      el.innerHTML = '<div class="empty-state" style="color:var(--danger);">Failed to load stats.</div>';
    }
  }

  async function pollKeys() {
    const el = $('#keys-body');
    if (!adminToken) { el.innerHTML = '<div class="empty-state">Enter your admin token above to manage API keys.</div>'; return; }
    try {
      const r = await fetch(baseUrl() + '/admin/keys', { headers: authHeaders() });
      if (r.status === 403) { el.innerHTML = '<div class="empty-state" style="color:var(--danger);">Invalid admin token.</div>'; return; }
      if (!r.ok) throw new Error(r.status);
      const keys = (await r.json()).keys || [];
      if (!keys.length) { el.innerHTML = '<div class="empty-state">No API keys created yet.</div>'; return; }
      let html = '<table class="data"><thead><tr><th>Name</th><th>Prefix</th><th>Created</th><th>Last Used</th><th>Status</th><th></th></tr></thead><tbody>';
      for (const k of keys) {
        const status = k.revoked ? '<span class="badge badge-cancel">Revoked</span>' : '<span class="badge badge-active">Active</span>';
        const revokeBtn = k.revoked ? '' : '<button type="button" class="btn btn-sm btn-danger js-revoke-key" data-key-id="' + escHtml(k.id) + '" data-key-name="' + escHtml(k.name) + '">Revoke</button>';
        html += '<tr><td>' + escHtml(k.name) + '</td><td><code>' + escHtml(k.prefix) + '…</code></td><td>' + fmtTs(k.created_at) + '</td><td>' + fmtTs(k.last_used_at) + '</td><td>' + status + '</td><td>' + revokeBtn + '</td></tr>';
      }
      el.innerHTML = html + '</tbody></table>';
      el.querySelectorAll('.js-revoke-key').forEach(btn => btn.addEventListener('click', () => window.__revokeKey(btn.dataset.keyId, btn.dataset.keyName)));
    } catch (_) {
      el.innerHTML = '<div class="empty-state" style="color:var(--danger);">Failed to load keys.</div>';
    }
  }

  window.__createKey = async function() {
    const nameInput = $('#new-key-name');
    const name = nameInput.value.trim();
    if (!name) { nameInput.focus(); return; }
    try {
      const r = await fetch(baseUrl() + '/admin/keys', { method: 'POST', headers: authHeaders(), body: JSON.stringify({ name }) });
      if (!r.ok) throw new Error(r.status);
      const d = await r.json();
      nameInput.value = '';
      const box = document.createElement('div');
      box.className = 'secret-box';
      const copyBtn = document.createElement('button');
      copyBtn.type = 'button';
      copyBtn.className = 'copy-btn';
      copyBtn.textContent = 'Copy';
      copyBtn.addEventListener('click', () => {
        navigator.clipboard.writeText(d.secret);
        copyBtn.textContent = 'Copied';
        copyBtn.classList.add('copied');
      });
      box.appendChild(copyBtn);
      box.appendChild(document.createTextNode(d.secret));
      const warn = document.createElement('span');
      warn.className = 'warn';
      warn.textContent = 'Copy this key now — it is only displayed once.';
      box.appendChild(warn);
      const container = $('#new-key-secret');
      container.textContent = '';
      container.appendChild(box);
      pollKeys();
    } catch (e) {
      alert('Failed to create key: ' + e.message);
    }
  };

  window.__revokeKey = async function(id, name) {
    if (!confirm('Revoke API key "' + name + '"? This cannot be undone.')) return;
    try {
      const r = await fetch(baseUrl() + '/admin/keys/' + encodeURIComponent(id), { method: 'DELETE', headers: authHeaders() });
      if (!r.ok && r.status !== 204) throw new Error(r.status);
      pollKeys();
    } catch (e) {
      alert('Failed to revoke key: ' + e.message);
    }
  };

  function pollAll() { return Promise.all([pollHealth(), pollWorkers(), pollStats(), pollKeys()]); }
  initConfig();
  pollAll();
  setInterval(pollAll, POLL_MS);
})();
    "#;

    let body = r#"
    <div class="dash-top">
      <div>
        <h1>Relay Admin Dashboard</h1>
        <p>Live worker telemetry, queue depth, and API key management for your ModelRelay server.</p>
      </div>
      <div class="dash-actions">
        <a href="/setup" class="btn btn-sm">+ Connect Worker</a>
        <a href="/integrate" class="btn btn-sm btn-secondary">Pi &amp; API Setup</a>
      </div>
    </div>

    <div class="config-bar">
      <label for="cfg-token">Admin Token</label>
      <input id="cfg-token" type="password" placeholder="MODELRELAY_ADMIN_TOKEN">
      <label for="cfg-url">Server URL</label>
      <input id="cfg-url" type="text" placeholder="(same origin)">
      <span id="cfg-status" class="status">&mdash;</span>
    </div>

    <div class="kpi-grid">
      <div class="kpi-card"><div class="kpi-label">Status</div><div id="h-status" class="kpi-val">&mdash;</div></div>
      <div class="kpi-card"><div class="kpi-label">Version</div><div id="h-version" class="kpi-val">&mdash;</div></div>
      <div class="kpi-card"><div class="kpi-label">Workers</div><div id="h-workers" class="kpi-val">&mdash;</div></div>
      <div class="kpi-card"><div class="kpi-label">Queue Depth</div><div id="h-queue" class="kpi-val">&mdash;</div></div>
      <div class="kpi-card"><div class="kpi-label">Uptime</div><div id="h-uptime" class="kpi-val">&mdash;</div></div>
    </div>

    <div class="panel">
      <div class="panel-head"><h2><span class="dot"></span> Connected Workers</h2></div>
      <div id="workers-body"><div class="empty-state">Loading&hellip;</div></div>
    </div>

    <div class="panel">
      <div class="panel-head"><h2>Queue Depth</h2></div>
      <div id="stats-body"><div class="empty-state">Loading&hellip;</div></div>
    </div>

    <div class="panel">
      <div class="panel-head"><h2>API Keys</h2></div>
      <div class="key-create">
        <input id="new-key-name" type="text" placeholder="Key name (e.g. pi-laptop)">
        <button type="button" class="btn btn-sm" onclick="window.__createKey()">Create Key</button>
      </div>
      <div id="new-key-secret"></div>
      <div id="keys-body"><div class="empty-state">Loading&hellip;</div></div>
    </div>
    "#;

    let extra_css = format!("<style>{dashboard_css}</style>");
    let extra_body_end = format!("<script>{dashboard_js}</script>");

    page_shell_custom(
        "Dashboard",
        "/dashboard",
        body,
        true,
        &extra_css,
        &extra_body_end,
    )
}
