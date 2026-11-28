//! Shared Smart Multi-Worker Routing dashboard panel.
//!
//! Rendered identically in both the self-hosted `modelrelay-web` dashboard and
//! the hosted `modelrelay-cloud` dashboard (for both admins and subscribers).
//! Uses zero external JS/CSS dependencies, respects the shared light/dark theme
//! tokens from `BASE_CSS`, and communicates with the single unified server
//! routing contract (`RoutingStateResponse` / `UpdateRoutingConfigRequest`).

use serde::Serialize;

/// Runtime configuration passed to the shared Smart Routing UI component.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RoutingPanelConfig {
    /// Endpoint for `GET` routing state (used when `use_oss_admin_auth` is `false`).
    pub get_url: String,
    /// Endpoint for saving routing config (used when `use_oss_admin_auth` is `false`).
    pub save_url: String,
    /// HTTP method for saving config (`"PUT"` for `/admin/routing`, `"POST"` for Cloud `/dashboard/routing`).
    pub save_method: String,
    /// Optional CSRF token sent via `X-CSRF-Token` header on save requests.
    pub csrf_token: Option<String>,
    /// When `true`, read `mr_admin_token` and `mr_server_url` from `localStorage`
    /// and target `<serverUrl || origin>/admin/routing`.
    pub use_oss_admin_auth: bool,
    /// When `true`, render the Connected Machines (Speed & Prefix Cache) card inside the panel.
    pub show_workers_table: bool,
    /// Optional DOM element ID of a KPI counter (e.g. `"sub-kpi-workers"`) to sync with live worker count.
    pub sync_worker_kpi_id: Option<String>,
}

/// Rendered CSS, HTML, and JavaScript fragments for embedding the shared routing panel.
#[derive(Debug, Clone)]
pub struct RoutingPanelAssets {
    /// Inline `<style>` block containing `.mr-rt-*` rules.
    pub css: String,
    /// Dashboard section HTML markup (telemetry, exact models, custom groups, modal).
    pub html: String,
    /// Inline `<script>` block bootstrapping and driving the panel.
    pub js: String,
}

/// Render the shared Smart Multi-Worker Routing dashboard UI assets.
#[must_use]
pub fn render_routing_panel(config: &RoutingPanelConfig) -> RoutingPanelAssets {
    let config_json = serde_json::to_string(config)
        .unwrap_or_else(|_| "{}".to_string())
        .replace("</", "<\\/");

    let css = format!("<style>{ROUTING_CSS}</style>");
    let html = if config.show_workers_table {
        ROUTING_HTML.replace("<!--WORKERS_CARD-->", ROUTING_WORKERS_CARD_HTML)
    } else {
        ROUTING_HTML.replace("<!--WORKERS_CARD-->", "")
    };
    let js = format!(
        "<script>window.__mrRoutingPanelConfig = {config_json};</script>\n<script>{ROUTING_JS}</script>"
    );

    RoutingPanelAssets { css, html, js }
}

const ROUTING_WORKERS_CARD_HTML: &str = r#"
  <div class="mr-rt-card" style="margin-bottom:18px;">
    <div class="mr-rt-head">
      <div>
        <h2><span class="mr-rt-dot"></span> Connected Machines &amp; Cache Telemetry</h2>
        <p class="mr-rt-sub">Live load, average time-to-first-token (EWMA), and active warm prefix nodes per worker.</p>
      </div>
      <a href="/setup" class="btn btn-sm btn-secondary">+ Connect Machine</a>
    </div>
    <div id="mr-rt-workers-body"><div class="mr-rt-empty">Loading connected machines&hellip;</div></div>
  </div>
"#;

const ROUTING_HTML: &str = r#"
<section class="mr-rt-section" id="smart-routing-section" aria-label="Smart Multi-Worker Routing">
  <!--WORKERS_CARD-->
  <div class="mr-rt-card">
    <div class="mr-rt-head">
      <div>
        <h2>Smart Multi-Worker Routing</h2>
        <p class="mr-rt-sub">Prefix-aware caching keeps sequential turns warm and spills concurrent bursts to idle machines automatically.</p>
      </div>
      <div class="mr-rt-head-actions">
        <button type="button" class="btn btn-sm btn-secondary" id="mr-rt-edit-default-btn">Edit Default Policy</button>
        <button type="button" class="btn btn-sm" id="mr-rt-new-group-btn">+ Tie Models Together</button>
      </div>
    </div>

    <div id="mr-rt-toast" class="mr-rt-toast" role="status" aria-live="polite" hidden></div>
    <div id="mr-rt-multi-banner" class="mr-rt-banner" hidden></div>

    <div class="mr-rt-telemetry" id="mr-rt-telemetry">
      <div class="mr-rt-kpi">
        <div class="mr-rt-kpi-label">Prefix Hit Rate</div>
        <div class="mr-rt-kpi-val" id="mr-rt-kpi-hitrate">&mdash;</div>
        <div class="mr-rt-kpi-sub" id="mr-rt-kpi-hitrate-sub">0 routed</div>
      </div>
      <div class="mr-rt-kpi">
        <div class="mr-rt-kpi-label">Avg Matched Prefix</div>
        <div class="mr-rt-kpi-val" id="mr-rt-kpi-avgmatch">&mdash;</div>
        <div class="mr-rt-kpi-sub" id="mr-rt-kpi-avgmatch-sub">0 tracked nodes</div>
      </div>
      <div class="mr-rt-kpi">
        <div class="mr-rt-kpi-label">Concurrent Spillovers</div>
        <div class="mr-rt-kpi-val" id="mr-rt-kpi-spills">0</div>
        <div class="mr-rt-kpi-sub">parallel bursts farmed to idle peers</div>
      </div>
      <div class="mr-rt-kpi">
        <div class="mr-rt-kpi-label">Verified Cache Hit</div>
        <div class="mr-rt-kpi-val" id="mr-rt-kpi-verified">&mdash;</div>
        <div class="mr-rt-kpi-sub" id="mr-rt-kpi-verified-sub">awaiting backend token usage</div>
      </div>
    </div>

    <div class="mr-rt-toolbar">
      <div class="mr-rt-default-pill">
        <span class="mr-rt-muted">Default policy:</span>
        <strong id="mr-rt-default-summary">Prefix-Aware &middot; Spill on Busy</strong>
      </div>
      <label class="mr-rt-toggle-label">
        <input type="checkbox" id="mr-rt-debug-headers">
        <span>Expose <code>X-ModelRelay-*</code> routing debug headers</span>
      </label>
    </div>

    <div class="mr-rt-subhead">
      <h3>Exact Models &amp; Unified Pools</h3>
      <span class="mr-rt-muted" style="font-size:0.8rem;">Exact model IDs are grouped automatically; click Configure to customize any model.</span>
    </div>
    <div id="mr-rt-models-body"><div class="mr-rt-empty">Loading live models&hellip;</div></div>

    <div class="mr-rt-subhead" style="margin-top:22px;">
      <h3>Custom Model Groups</h3>
      <span class="mr-rt-muted" style="font-size:0.8rem;">Tie exact models together visually &mdash; share one policy or pool differently-named models under one ID.</span>
    </div>
    <div id="mr-rt-groups-body"><div class="mr-rt-empty">No custom groups configured yet.</div></div>
  </div>

  <div class="mr-rt-backdrop" id="mr-rt-modal-backdrop" hidden>
    <div class="mr-rt-modal" role="dialog" aria-modal="true" aria-labelledby="mr-rt-modal-title">
      <div class="mr-rt-modal-head">
        <div>
          <h3 id="mr-rt-modal-title">Configure Routing</h3>
          <p id="mr-rt-modal-subtitle" class="mr-rt-sub"></p>
        </div>
        <button type="button" class="mr-rt-close" id="mr-rt-modal-close" aria-label="Close">&times;</button>
      </div>
      <div id="mr-rt-modal-error" class="mr-rt-error" hidden></div>
      <div class="mr-rt-modal-body" id="mr-rt-modal-body"></div>
      <div class="mr-rt-modal-foot" id="mr-rt-modal-foot"></div>
    </div>
  </div>
</section>
"#;

const ROUTING_CSS: &str = r"
    .mr-rt-section { margin-bottom: 24px; }
    .mr-rt-card {
      background: var(--bg-surface); border: 1px solid var(--border);
      border-radius: var(--radius-lg); padding: 24px; box-shadow: var(--shadow-card);
    }
    .mr-rt-head {
      display: flex; justify-content: space-between; align-items: flex-start;
      gap: 14px; flex-wrap: wrap; margin-bottom: 16px;
    }
    .mr-rt-head h2 {
      font-size: 1.08rem; font-weight: 700; margin: 0 0 4px; color: var(--text-strong);
      display: flex; align-items: center; gap: 8px;
    }
    .mr-rt-dot { width: 8px; height: 8px; border-radius: 50%; background: var(--success); display: inline-block; }
    .mr-rt-sub { font-size: 0.85rem; color: var(--text-secondary); margin: 0; }
    .mr-rt-muted { color: var(--text-muted); }
    .mr-rt-head-actions { display: flex; gap: 8px; flex-wrap: wrap; }

    .mr-rt-toast {
      margin-bottom: 14px; padding: 10px 14px; border-radius: var(--radius-md);
      font-size: 0.85rem; font-weight: 600;
      background: var(--success-soft); color: var(--success-fg);
      border: 1px solid rgba(5, 150, 105, 0.35);
    }
    .mr-rt-toast.err {
      background: var(--danger-soft); color: var(--danger-fg);
      border-color: rgba(220, 38, 38, 0.35);
    }
    .mr-rt-banner {
      margin-bottom: 16px; padding: 11px 15px; border-radius: var(--radius-md);
      background: var(--accent-soft); border: 1px solid var(--accent-border);
      color: var(--accent-fg); font-size: 0.86rem; line-height: 1.45;
    }

    .mr-rt-telemetry {
      display: grid; grid-template-columns: repeat(4, 1fr); gap: 10px; margin-bottom: 16px;
    }
    .mr-rt-kpi {
      background: var(--bg-subtle); border: 1px solid var(--border);
      border-radius: var(--radius-md); padding: 12px 14px;
    }
    .mr-rt-kpi-label {
      font-size: 0.68rem; font-weight: 700; text-transform: uppercase;
      letter-spacing: 0.08em; color: var(--text-muted);
    }
    .mr-rt-kpi-val {
      font-size: 1.3rem; font-weight: 700; color: var(--text-strong);
      margin-top: 4px; font-variant-numeric: tabular-nums;
    }
    .mr-rt-kpi-sub { font-size: 0.75rem; color: var(--text-muted); margin-top: 2px; }

    .mr-rt-toolbar {
      display: flex; justify-content: space-between; align-items: center; gap: 12px; flex-wrap: wrap;
      padding: 11px 14px; border-radius: var(--radius-md);
      background: var(--bg-subtle); border: 1px solid var(--border); margin-bottom: 18px;
      font-size: 0.84rem;
    }
    .mr-rt-default-pill { display: flex; align-items: center; gap: 6px; flex-wrap: wrap; }
    .mr-rt-toggle-label {
      display: inline-flex; align-items: center; gap: 7px; cursor: pointer;
      color: var(--text-secondary); font-size: 0.82rem; user-select: none;
    }
    .mr-rt-toggle-label input { accent-color: var(--accent); cursor: pointer; }

    .mr-rt-subhead {
      display: flex; justify-content: space-between; align-items: baseline; gap: 10px;
      flex-wrap: wrap; margin-bottom: 10px;
    }
    .mr-rt-subhead h3 {
      font-size: 0.84rem; font-weight: 700; text-transform: uppercase;
      letter-spacing: 0.07em; color: var(--text-muted); margin: 0;
    }

    .mr-rt-table { width: 100%; border-collapse: collapse; font-size: 0.86rem; }
    .mr-rt-table th {
      text-align: left; font-size: 0.7rem; font-weight: 700; text-transform: uppercase;
      letter-spacing: 0.07em; color: var(--text-muted);
      padding: 8px 10px 9px 0; border-bottom: 1px solid var(--border);
    }
    .mr-rt-table td {
      padding: 11px 10px 11px 0; border-bottom: 1px solid var(--border);
      color: var(--text-primary); vertical-align: top;
    }
    .mr-rt-table tr:last-child td { border-bottom: none; }
    .mr-rt-empty { color: var(--text-muted); text-align: center; padding: 18px 0; font-size: 0.88rem; }

    .mr-rt-model-cell { display: flex; flex-direction: column; gap: 5px; align-items: flex-start; }
    .mr-rt-model-id {
      font-family: var(--font-mono); font-size: 0.84rem; font-weight: 600;
      color: var(--text-strong); word-break: break-all;
    }
    .mr-rt-badges { display: flex; gap: 5px; flex-wrap: wrap; }

    .mr-rt-worker-list { display: flex; flex-wrap: wrap; gap: 6px; }
    .mr-rt-chip {
      display: inline-flex; align-items: center; gap: 6px; flex-wrap: wrap;
      padding: 4px 9px; border-radius: 7px;
      background: var(--bg-elevated); border: 1px solid var(--border);
      font-size: 0.78rem; color: var(--text-primary); line-height: 1.3;
    }
    .mr-rt-chip.err { border-color: rgba(220, 38, 38, 0.45); }
    .mr-rt-chip-dot { width: 7px; height: 7px; border-radius: 50%; flex-shrink: 0; background: var(--text-muted); }
    .mr-rt-chip-dot.loaded { background: var(--success); }
    .mr-rt-chip-dot.unloaded { background: var(--warning); }
    .mr-rt-chip-name { font-weight: 600; color: var(--text-strong); }
    .mr-rt-chip-meta { color: var(--text-secondary); font-family: var(--font-mono); font-size: 0.73rem; }

    .mr-rt-strat-name { font-weight: 600; color: var(--text-strong); font-size: 0.84rem; }
    .mr-rt-strat-sub { font-size: 0.76rem; color: var(--text-muted); margin-top: 2px; }

    .mr-rt-group-list { display: flex; flex-direction: column; gap: 10px; }
    .mr-rt-group-card {
      display: flex; justify-content: space-between; align-items: flex-start; gap: 14px; flex-wrap: wrap;
      padding: 13px 15px; border-radius: var(--radius-md);
      background: var(--bg-subtle); border: 1px solid var(--border);
    }
    .mr-rt-group-title { display: flex; align-items: center; gap: 8px; flex-wrap: wrap; margin-bottom: 6px; }
    .mr-rt-group-title strong { color: var(--text-strong); font-size: 0.92rem; }
    .mr-rt-group-targets { display: flex; flex-wrap: wrap; gap: 5px; margin-top: 6px; }
    .mr-rt-target-pill {
      display: inline-block; padding: 2px 8px; border-radius: 5px;
      background: var(--accent-soft); border: 1px solid var(--accent-border);
      font-family: var(--font-mono); font-size: 0.75rem; color: var(--accent-fg);
    }
    .mr-rt-group-actions { display: flex; gap: 6px; flex-shrink: 0; }

    .mr-rt-load-wrap { display: inline-flex; align-items: center; gap: 7px; font-variant-numeric: tabular-nums; font-size: 0.82rem; }
    .mr-rt-load-track { width: 54px; height: 6px; border-radius: 999px; background: var(--bg-elevated); overflow: hidden; }
    .mr-rt-load-fill { height: 100%; background: linear-gradient(90deg, var(--accent), var(--cyan)); border-radius: 999px; }

    /* Modal */
    .mr-rt-backdrop {
      position: fixed; inset: 0; z-index: 500;
      background: rgba(6, 8, 13, 0.72); backdrop-filter: blur(4px);
      display: flex; align-items: center; justify-content: center; padding: 20px;
    }
    .mr-rt-backdrop[hidden] { display: none; }
    .mr-rt-modal {
      width: 100%; max-width: 640px; max-height: 90vh;
      display: flex; flex-direction: column;
      background: var(--bg-surface); border: 1px solid var(--border-strong);
      border-radius: var(--radius-xl); box-shadow: var(--shadow-hero); overflow: hidden;
    }
    .mr-rt-modal-head {
      display: flex; justify-content: space-between; align-items: flex-start; gap: 12px;
      padding: 18px 22px 14px; border-bottom: 1px solid var(--border);
    }
    .mr-rt-modal-head h3 { font-size: 1.08rem; font-weight: 700; color: var(--text-strong); margin: 0 0 3px; }
    .mr-rt-close {
      background: transparent; border: 1px solid var(--border); border-radius: 7px;
      color: var(--text-secondary); font-size: 1.25rem; line-height: 1;
      width: 30px; height: 30px; cursor: pointer; flex-shrink: 0;
    }
    .mr-rt-close:hover { background: var(--bg-hover); color: var(--text-strong); }
    .mr-rt-error {
      margin: 12px 22px 0; padding: 10px 13px; border-radius: var(--radius-md);
      background: var(--danger-soft); border: 1px solid rgba(220, 38, 38, 0.35);
      color: var(--danger-fg); font-size: 0.84rem;
    }
    .mr-rt-modal-body { padding: 18px 22px; overflow-y: auto; display: flex; flex-direction: column; gap: 15px; }
    .mr-rt-modal-foot {
      display: flex; justify-content: space-between; align-items: center; gap: 10px; flex-wrap: wrap;
      padding: 14px 22px; border-top: 1px solid var(--border); background: var(--bg-subtle);
    }
    .mr-rt-foot-right { display: flex; gap: 8px; margin-left: auto; }

    .mr-rt-field { display: flex; flex-direction: column; gap: 6px; }
    .mr-rt-field label, .mr-rt-field-label {
      font-size: 0.8rem; font-weight: 600; color: var(--text-secondary);
    }
    .mr-rt-input, .mr-rt-select {
      width: 100%; padding: 9px 12px; border-radius: 8px;
      background: var(--bg-input); border: 1px solid var(--border-strong);
      color: var(--text-primary); font-size: 0.88rem; font-family: inherit;
    }
    .mr-rt-input:focus, .mr-rt-select:focus {
      border-color: var(--accent); box-shadow: 0 0 0 3px var(--accent-soft); outline: none;
    }
    .mr-rt-help { font-size: 0.77rem; color: var(--text-muted); line-height: 1.4; }
    .mr-rt-row-2 { display: grid; grid-template-columns: 1fr 1fr; gap: 12px; }

    .mr-rt-mode-grid { display: grid; grid-template-columns: 1fr 1fr; gap: 10px; }
    .mr-rt-mode-card {
      padding: 12px; border-radius: var(--radius-md); cursor: pointer;
      border: 1px solid var(--border-strong); background: var(--bg-subtle);
      display: flex; align-items: flex-start; gap: 9px; text-align: left;
    }
    .mr-rt-mode-card.active {
      border-color: var(--accent); background: var(--accent-soft);
    }
    .mr-rt-mode-card input { margin-top: 3px; accent-color: var(--accent); }
    .mr-rt-mode-card strong { display: block; font-size: 0.86rem; color: var(--text-strong); margin-bottom: 2px; }
    .mr-rt-mode-card span { display: block; font-size: 0.76rem; color: var(--text-secondary); line-height: 1.35; }

    .mr-rt-check-box {
      border: 1px solid var(--border); border-radius: var(--radius-md);
      background: var(--bg-subtle); padding: 10px 12px;
      max-height: 220px; overflow-y: auto; display: flex; flex-direction: column; gap: 8px;
    }
    .mr-rt-check-group-label {
      font-size: 0.73rem; font-weight: 700; text-transform: uppercase;
      letter-spacing: 0.06em; color: var(--text-muted); margin-top: 4px;
    }
    .mr-rt-check-item {
      display: flex; align-items: center; gap: 8px; font-size: 0.84rem;
      color: var(--text-primary); cursor: pointer;
    }
    .mr-rt-check-item input { accent-color: var(--accent); cursor: pointer; }
    .mr-rt-check-item code { font-size: 0.8rem; }
    .mr-rt-manual-add { display: flex; gap: 8px; flex-wrap: wrap; align-items: center; margin-top: 4px; }
    .mr-rt-manual-add .mr-rt-input { flex: 1; min-width: 140px; padding: 7px 10px; font-size: 0.82rem; }

    .mr-rt-prio-list { display: flex; flex-direction: column; gap: 6px; }
    .mr-rt-prio-item {
      display: flex; align-items: center; justify-content: space-between; gap: 8px;
      padding: 7px 10px; border-radius: 7px;
      background: var(--bg-elevated); border: 1px solid var(--border); font-size: 0.84rem;
    }
    .mr-rt-prio-rank {
      font-size: 0.74rem; font-weight: 700; color: var(--accent-fg);
      min-width: 22px; font-family: var(--font-mono);
    }
    .mr-rt-prio-btns { display: inline-flex; gap: 4px; }
    .mr-rt-icon-btn {
      padding: 2px 7px; border-radius: 5px; border: 1px solid var(--border-strong);
      background: var(--bg-surface); color: var(--text-secondary);
      font-size: 0.76rem; cursor: pointer;
    }
    .mr-rt-icon-btn:hover { background: var(--bg-hover); color: var(--text-strong); }

    .mr-rt-details {
      border: 1px solid var(--border); border-radius: var(--radius-md);
      padding: 10px 13px; background: var(--bg-subtle);
    }
    .mr-rt-details summary {
      cursor: pointer; font-size: 0.82rem; font-weight: 600; color: var(--text-secondary);
    }
    .mr-rt-details-body { display: flex; flex-direction: column; gap: 9px; margin-top: 11px; }

    @media (max-width: 820px) {
      .mr-rt-telemetry { grid-template-columns: repeat(2, 1fr); }
      .mr-rt-table { display: block; overflow-x: auto; }
      .mr-rt-mode-grid, .mr-rt-row-2 { grid-template-columns: 1fr; }
    }
    @media (max-width: 520px) {
      .mr-rt-telemetry { grid-template-columns: 1fr; }
      .mr-rt-card { padding: 18px; }
    }
";

#[allow(clippy::too_many_lines)]
const ROUTING_JS: &str = r#"
(function() {
  var cfg = window.__mrRoutingPanelConfig;
  if (!cfg) return;

  var POLL_MS = 5000;
  var lastState = null;
  var toastTimer = null;

  var DEFAULT_POLICY = {
    strategy: 'prefix_aware',
    spillover_mode: 'spill_on_busy',
    min_prefix_bytes: 512,
    prefix_ttl_secs: 900,
    prefix_aware_queue: true,
    respect_context_window: true,
    prefer_loaded_models: true,
    allow_client_overrides: true,
    worker_priority: [],
    sticky_after_overflow: true
  };

  function el(tag, className, text) {
    var node = document.createElement(tag);
    if (className) node.className = className;
    if (text !== undefined && text !== null) node.textContent = String(text);
    return node;
  }

  function cloneJson(obj) {
    return JSON.parse(JSON.stringify(obj));
  }

  function mergePolicy(p) {
    var out = cloneJson(DEFAULT_POLICY);
    if (!p || typeof p !== 'object') return out;
    Object.keys(out).forEach(function(k) {
      if (p[k] !== undefined && p[k] !== null) out[k] = cloneJson(p[k]);
    });
    if (!Array.isArray(out.worker_priority)) out.worker_priority = [];
    return out;
  }

  function formatStrategyLabel(policy) {
    var p = mergePolicy(policy);
    switch (p.strategy) {
      case 'prefix_aware': return 'Prefix-Aware';
      case 'priority_failover': return 'Priority Failover';
      case 'least_loaded': return 'Least Loaded';
      case 'round_robin': return 'Round Robin';
      default: return p.strategy || 'Prefix-Aware';
    }
  }

  function formatSpilloverLabel(mode) {
    switch (mode) {
      case 'spill_on_busy': return 'Spill on Busy';
      case 'cost_balanced': return 'Cost Balanced';
      case 'strict_sticky': return 'Strict Sticky';
      default: return 'Spill on Busy';
    }
  }

  function formatStrategySub(policy) {
    var p = mergePolicy(policy);
    if (p.strategy === 'prefix_aware') {
      return formatSpilloverLabel(p.spillover_mode) + ' \u00b7 TTL ' + p.prefix_ttl_secs + 's';
    }
    if (p.strategy === 'priority_failover') {
      if (p.worker_priority.length > 0) {
        return p.worker_priority.slice(0, 3).join(' \u2192 ') + (p.worker_priority.length > 3 ? ' \u2026' : '');
      }
      return 'Default worker order';
    }
    if (p.strategy === 'least_loaded') return 'Lowest load ratio + fastest TTFT';
    if (p.strategy === 'round_robin') return 'Deterministic rotation';
    return '';
  }

  function formatCtx(ctx) {
    if (!ctx || ctx <= 0) return '';
    if (ctx >= 1000) return Math.round(ctx / 1024) + 'K ctx';
    return ctx + ' ctx';
  }

  function showToast(msg, isError) {
    var node = document.getElementById('mr-rt-toast');
    if (!node) return;
    node.textContent = msg;
    node.className = 'mr-rt-toast' + (isError ? ' err' : '');
    node.hidden = false;
    if (toastTimer) clearTimeout(toastTimer);
    toastTimer = setTimeout(function() { node.hidden = true; }, 4500);
  }

  function ossBaseUrl() {
    var stored = '';
    try { stored = (localStorage.getItem('mr_server_url') || '').trim().replace(/\/+$/, ''); } catch (_) {}
    return stored || window.location.origin;
  }

  function ossAdminToken() {
    try { return (localStorage.getItem('mr_admin_token') || '').trim(); } catch (_) { return ''; }
  }

  function requestEndpoint(isSave) {
    if (cfg.useOssAdminAuth) {
      return ossBaseUrl() + '/admin/routing';
    }
    return isSave ? cfg.saveUrl : cfg.getUrl;
  }

  function requestHeaders(isSave) {
    var headers = {};
    if (isSave) headers['Content-Type'] = 'application/json';
    if (cfg.useOssAdminAuth) {
      var tok = ossAdminToken();
      if (tok) headers['Authorization'] = 'Bearer ' + tok;
    } else if (isSave && cfg.csrfToken) {
      headers['X-CSRF-Token'] = cfg.csrfToken;
    }
    return headers;
  }

  function fetchOptions(isSave, bodyObj) {
    var opts = {
      method: isSave ? (cfg.saveMethod || 'PUT') : 'GET',
      headers: requestHeaders(isSave)
    };
    if (!cfg.useOssAdminAuth) {
      opts.credentials = 'same-origin';
    }
    if (isSave) {
      opts.body = JSON.stringify(bodyObj);
    }
    return opts;
  }

  async function saveRoutingConfig(nextConfig) {
    var url = requestEndpoint(true);
    var resp = await fetch(url, fetchOptions(true, { config: nextConfig }));
    var text = await resp.text();
    var parsed = null;
    try { parsed = text ? JSON.parse(text) : null; } catch (_) {}
    if (!resp.ok) {
      var errMsg = (parsed && parsed.error && (parsed.error.message || parsed.error)) ||
        ('Save failed (HTTP ' + resp.status + ')');
      throw new Error(String(errMsg));
    }
    if (parsed && parsed.config) {
      lastState = parsed;
      renderAll(parsed);
    } else {
      await pollRouting();
    }
  }

  function renderTelemetry(t) {
    t = t || {};
    var total = Number(t.total_routed) || 0;
    var hits = (Number(t.prefix_hits) || 0) + (Number(t.session_hits) || 0);
    var spills = Number(t.concurrent_spillovers) || 0;
    var stickies = Number(t.warm_busy_stickies) || 0;
    var matchedReqs = (Number(t.prefix_hits) || 0) + spills + stickies;
    var totalMatchedBytes = Number(t.total_matched_prefix_bytes) || 0;
    var trackedNodes = Number(t.tracked_prefix_nodes) || 0;
    var vPrompt = Number(t.verified_prompt_tokens) || 0;
    var vCached = Number(t.verified_cached_prompt_tokens) || 0;

    var hr = document.getElementById('mr-rt-kpi-hitrate');
    var hrSub = document.getElementById('mr-rt-kpi-hitrate-sub');
    if (hr) hr.textContent = total > 0 ? Math.round((hits / total) * 100) + '%' : '\u2014';
    if (hrSub) hrSub.textContent = hits.toLocaleString() + ' / ' + total.toLocaleString() + ' routed';

    var am = document.getElementById('mr-rt-kpi-avgmatch');
    var amSub = document.getElementById('mr-rt-kpi-avgmatch-sub');
    var avgBytes = matchedReqs > 0 ? Math.round(totalMatchedBytes / matchedReqs) : 0;
    if (am) {
      if (avgBytes >= 1024) am.textContent = (avgBytes / 1024).toFixed(1) + ' KB';
      else if (avgBytes > 0) am.textContent = avgBytes + ' B';
      else am.textContent = '\u2014';
    }
    if (amSub) {
      amSub.textContent = avgBytes > 0
        ? '~' + Math.round(avgBytes / 4).toLocaleString() + ' tok \u00b7 ' + trackedNodes + ' nodes'
        : trackedNodes + ' tracked prefix nodes';
    }

    var sp = document.getElementById('mr-rt-kpi-spills');
    if (sp) sp.textContent = spills.toLocaleString();

    var vf = document.getElementById('mr-rt-kpi-verified');
    var vfSub = document.getElementById('mr-rt-kpi-verified-sub');
    if (vf) vf.textContent = vPrompt > 0 ? Math.round((vCached / vPrompt) * 100) + '%' : '\u2014';
    if (vfSub) {
      vfSub.textContent = vPrompt > 0
        ? vCached.toLocaleString() + ' / ' + vPrompt.toLocaleString() + ' prompt tok'
        : 'awaiting backend token usage';
    }
  }

  function renderWorkersCard(workers) {
    var container = document.getElementById('mr-rt-workers-body');
    if (!container) return;
    container.textContent = '';
    if (!workers || !workers.length) {
      var empty = el('div', 'mr-rt-empty', 'No workers connected right now. Launch ModelRelay Desktop or modelrelay-worker on your GPU machines.');
      container.appendChild(empty);
      return;
    }
    var tbl = el('table', 'mr-rt-table');
    var thead = el('thead');
    var hr = el('tr');
    ['Machine', 'Models', 'Load', 'Avg TTFT', 'Warm Prefixes', 'Status'].forEach(function(col) {
      hr.appendChild(el('th', null, col));
    });
    thead.appendChild(hr);
    tbl.appendChild(thead);

    var tbody = el('tbody');
    workers.forEach(function(w) {
      var tr = el('tr');
      var tdName = el('td');
      tdName.appendChild(el('strong', null, w.worker_name || w.worker_id));
      tr.appendChild(tdName);

      var tdModels = el('td');
      (w.models || []).forEach(function(m) {
        tdModels.appendChild(el('span', 'mr-rt-target-pill', m === '*' ? 'All models' : m));
        tdModels.appendChild(document.createTextNode(' '));
      });
      tr.appendChild(tdModels);

      var tdLoad = el('td');
      var inflight = Number(w.in_flight_count) || 0;
      var maxc = Number(w.max_concurrent) || 0;
      var pct = maxc > 0 ? Math.min(100, Math.round((inflight / maxc) * 100)) : 0;
      var lw = el('span', 'mr-rt-load-wrap');
      var lt = el('span', 'mr-rt-load-track');
      var lf = el('span', 'mr-rt-load-fill');
      lf.style.width = pct + '%';
      lt.appendChild(lf);
      lw.appendChild(lt);
      lw.appendChild(document.createTextNode(inflight + '/' + maxc));
      tdLoad.appendChild(lw);
      tr.appendChild(tdLoad);

      var tdTtft = el('td', null, w.avg_ttft_ms != null ? Math.round(w.avg_ttft_ms) + 'ms' : '\u2014');
      tr.appendChild(tdTtft);

      var tdWarm = el('td', null, String(w.warm_prefix_count || 0));
      tr.appendChild(tdWarm);

      var tdStatus = el('td');
      tdStatus.appendChild(el('span', w.is_draining ? 'badge badge-warn' : 'badge badge-active', w.is_draining ? 'Draining' : 'Active'));
      tr.appendChild(tdStatus);
      tbody.appendChild(tr);
    });
    tbl.appendChild(tbody);
    container.appendChild(tbl);
  }

  function renderLiveModels(liveModels, config) {
    var body = document.getElementById('mr-rt-models-body');
    if (!body) return;
    body.textContent = '';

    var multiCount = (liveModels || []).filter(function(m) { return (m.workers || []).length >= 2; }).length;
    var banner = document.getElementById('mr-rt-multi-banner');
    if (banner) {
      if (multiCount > 0) {
        banner.textContent = 'Multi-worker routing active on ' + multiCount + ' model' + (multiCount === 1 ? '' : 's') +
          ' (2+ connected machines). Prefix-Aware routing keeps sequential turns warm and automatically spills concurrent bursts to idle peers.';
        banner.hidden = false;
      } else {
        banner.hidden = true;
      }
    }

    if (!liveModels || !liveModels.length) {
      body.appendChild(el('div', 'mr-rt-empty', 'No live models detected yet. Connect one or more GPU workers to inspect and customize routing.'));
      return;
    }

    var tbl = el('table', 'mr-rt-table');
    var thead = el('thead');
    var hr = el('tr');
    ['Model ID', 'Connected Machines', 'Active Strategy', ''].forEach(function(col) {
      hr.appendChild(el('th', null, col));
    });
    thead.appendChild(hr);
    tbl.appendChild(thead);

    var tbody = el('tbody');
    liveModels.forEach(function(m) {
      var tr = el('tr');

      var tdModel = el('td');
      var mc = el('div', 'mr-rt-model-cell');
      mc.appendChild(el('span', 'mr-rt-model-id', m.model_id));
      var badges = el('div', 'mr-rt-badges');
      if (m.is_unified_pool) {
        badges.appendChild(el('span', 'badge badge-cyan', 'Unified Pool'));
      } else if (m.group_name) {
        badges.appendChild(el('span', 'badge badge-accent', 'Group: ' + m.group_name));
      } else if (m.has_custom_override) {
        badges.appendChild(el('span', 'badge badge-warn', 'Custom'));
      } else {
        badges.appendChild(el('span', 'badge', 'Default'));
      }
      var wCount = (m.workers || []).length;
      if (wCount >= 2) {
        badges.appendChild(el('span', 'badge badge-active', wCount + ' machines'));
      }
      mc.appendChild(badges);
      tdModel.appendChild(mc);
      tr.appendChild(tdModel);

      var tdWorkers = el('td');
      var wl = el('div', 'mr-rt-worker-list');
      if (!wCount) {
        wl.appendChild(el('span', 'mr-rt-muted', 'No matching workers online'));
      } else {
        m.workers.forEach(function(w) {
          var chip = el('span', 'mr-rt-chip' + ((w.consecutive_errors || 0) > 0 ? ' err' : ''));
          var dot = el('span', 'mr-rt-chip-dot' + (w.loaded_state === 'loaded' ? ' loaded' : (w.loaded_state === 'unloaded' ? ' unloaded' : '')));
          dot.title = 'Model residency: ' + (w.loaded_state || 'unknown');
          chip.appendChild(dot);
          chip.appendChild(el('span', 'mr-rt-chip-name', w.worker_name));

          var parts = [(w.in_flight || 0) + '/' + (w.max_concurrent || 0)];
          if (m.is_unified_pool && w.native_model_id && w.native_model_id !== m.model_id) {
            parts.unshift('\u2192 ' + w.native_model_id);
          }
          if (w.loaded_state === 'loaded') parts.push('Loaded');
          else if (w.loaded_state === 'unloaded') parts.push('Unloaded');
          var ctxTxt = formatCtx(w.context_window);
          if (ctxTxt) parts.push(ctxTxt);
          if (w.ewma_ttft_ms != null) parts.push(Math.round(w.ewma_ttft_ms) + 'ms');
          if ((w.prompt_tokens_total || 0) > 0) {
            var hitPct = Math.round(((w.cached_prompt_tokens_total || 0) / w.prompt_tokens_total) * 100);
            parts.push(hitPct + '% cache');
          }
          if (w.is_draining) parts.push('Draining');
          chip.appendChild(el('span', 'mr-rt-chip-meta', parts.join(' \u00b7 ')));
          wl.appendChild(chip);
        });
      }
      tdWorkers.appendChild(wl);
      tr.appendChild(tdWorkers);

      var tdStrat = el('td');
      tdStrat.appendChild(el('div', 'mr-rt-strat-name', formatStrategyLabel(m.effective_policy)));
      tdStrat.appendChild(el('div', 'mr-rt-strat-sub', formatStrategySub(m.effective_policy)));
      tr.appendChild(tdStrat);

      var tdAct = el('td');
      tdAct.style.textAlign = 'right';
      var btn = el('button', 'btn btn-sm btn-secondary', m.group_id ? 'Edit Group' : 'Configure');
      btn.type = 'button';
      btn.addEventListener('click', function() {
        if (m.group_id) {
          var existing = ((config && config.groups) || []).find(function(g) { return g.id === m.group_id; });
          if (existing) { openGroupModal(existing); return; }
        }
        openPolicyModal('model', m);
      });
      tdAct.appendChild(btn);
      tr.appendChild(tdAct);

      tbody.appendChild(tr);
    });
    tbl.appendChild(tbody);
    body.appendChild(tbl);
  }

  function renderGroups(groups) {
    var body = document.getElementById('mr-rt-groups-body');
    if (!body) return;
    body.textContent = '';

    if (!groups || !groups.length) {
      body.appendChild(el('div', 'mr-rt-empty', 'No custom model groups yet. Click "+ Tie Models Together" to share a routing policy or pool differently-named models across machines (e.g. Ollama + LM Studio).'));
      return;
    }

    var list = el('div', 'mr-rt-group-list');
    groups.forEach(function(g) {
      var card = el('div', 'mr-rt-group-card');
      var info = el('div');

      var title = el('div', 'mr-rt-group-title');
      title.appendChild(el('strong', null, g.name));
      if (g.mode === 'unified_pool') {
        title.appendChild(el('span', 'badge badge-cyan', 'Unified Pool \u2192 ' + (g.pool_model_id || g.name)));
      } else {
        title.appendChild(el('span', 'badge badge-accent', 'Shared Policy'));
      }
      title.appendChild(el('span', 'badge', formatStrategyLabel(g.policy)));
      info.appendChild(title);

      var sub = el('div', 'mr-rt-strat-sub', formatStrategySub(g.policy));
      info.appendChild(sub);

      var targets = el('div', 'mr-rt-group-targets');
      (g.targets || []).forEach(function(t) {
        var label = t.worker_name ? (t.worker_name + ': ' + t.model_id) : t.model_id;
        targets.appendChild(el('span', 'mr-rt-target-pill', label));
      });
      info.appendChild(targets);

      var actions = el('div', 'mr-rt-group-actions');
      var editBtn = el('button', 'btn btn-sm btn-secondary', 'Edit');
      editBtn.type = 'button';
      editBtn.addEventListener('click', function() { openGroupModal(g); });
      var delBtn = el('button', 'btn btn-sm btn-danger', 'Delete');
      delBtn.type = 'button';
      delBtn.addEventListener('click', async function() {
        if (!confirm('Delete custom group "' + g.name + '"?')) return;
        if (!lastState || !lastState.config) return;
        var nextCfg = cloneJson(lastState.config);
        nextCfg.groups = (nextCfg.groups || []).filter(function(item) { return item.id !== g.id; });
        try {
          await saveRoutingConfig(nextCfg);
          showToast('Deleted group "' + g.name + '"', false);
        } catch (err) {
          showToast(err.message || 'Failed to delete group', true);
        }
      });
      actions.appendChild(editBtn);
      actions.appendChild(delBtn);

      card.appendChild(info);
      card.appendChild(actions);
      list.appendChild(card);
    });
    body.appendChild(list);
  }

  function renderAll(state) {
    if (!state) return;
    renderTelemetry(state.telemetry);

    var defSummary = document.getElementById('mr-rt-default-summary');
    if (defSummary && state.config) {
      var dp = mergePolicy(state.config.default_policy);
      defSummary.textContent = formatStrategyLabel(dp) + ' \u00b7 ' + formatStrategySub(dp);
    }

    var dbg = document.getElementById('mr-rt-debug-headers');
    if (dbg && state.config) {
      dbg.checked = Boolean(state.config.expose_debug_headers);
    }

    renderLiveModels(state.live_models || [], state.config);
    renderGroups((state.config && state.config.groups) || []);
    if (cfg.showWorkersTable) {
      renderWorkersCard(state.workers || []);
    }

    if (cfg.syncWorkerKpiId) {
      var kpi = document.getElementById(cfg.syncWorkerKpiId);
      if (kpi) {
        var count = (state.workers || []).length;
        kpi.textContent = String(count);
        kpi.style.color = count > 0 ? 'var(--success)' : 'var(--warning)';
      }
    }

    window.__mrLastRoutingWorkers = state.workers || [];
    try {
      window.dispatchEvent(new CustomEvent('mr:routing-updated', { detail: state }));
    } catch (_) {}
  }

  // ── Modal helpers ──────────────────────────────────────────────────────────

  function showModalError(msg) {
    var node = document.getElementById('mr-rt-modal-error');
    if (!node) return;
    if (!msg) { node.hidden = true; node.textContent = ''; return; }
    node.textContent = msg;
    node.hidden = false;
  }

  function closeModal() {
    var bd = document.getElementById('mr-rt-modal-backdrop');
    if (bd) bd.hidden = true;
    showModalError('');
  }

  function buildPolicyEditor(initialPolicy, suggestedWorkerNames) {
    var draft = mergePolicy(initialPolicy);
    var wrap = el('div');
    wrap.style.display = 'flex';
    wrap.style.flexDirection = 'column';
    wrap.style.gap = '14px';

    // Strategy selector
    var stratField = el('div', 'mr-rt-field');
    stratField.appendChild(el('label', null, 'Routing Strategy'));
    var stratSel = el('select', 'mr-rt-select');
    [
      ['prefix_aware', 'Prefix-Aware (Default) \u2014 Stick sequential turns, spill concurrent bursts'],
      ['priority_failover', 'Priority / Failover \u2014 Prefer top machines first, overflow when busy'],
      ['least_loaded', 'Least Loaded \u2014 Lowest load ratio with fastest-TTFT tie-breaker'],
      ['round_robin', 'Round Robin \u2014 Cycle evenly across eligible machines']
    ].forEach(function(opt) {
      var o = el('option', null, opt[1]);
      o.value = opt[0];
      if (draft.strategy === opt[0]) o.selected = true;
      stratSel.appendChild(o);
    });
    stratField.appendChild(stratSel);
    wrap.appendChild(stratField);

    // Dynamic strategy-specific options box
    var dynBox = el('div');
    dynBox.style.display = 'flex';
    dynBox.style.flexDirection = 'column';
    dynBox.style.gap = '12px';
    wrap.appendChild(dynBox);

    function renderStrategyOptions() {
      dynBox.textContent = '';
      if (draft.strategy === 'prefix_aware') {
        var spField = el('div', 'mr-rt-field');
        spField.appendChild(el('label', null, 'Concurrent Burst Spillover'));
        var spSel = el('select', 'mr-rt-select');
        [
          ['spill_on_busy', 'Spill on Busy (Default) \u2014 Immediately farm concurrent requests to an idle machine'],
          ['cost_balanced', 'Cost Balanced \u2014 Compare estimated prefill time vs. busy worker load'],
          ['strict_sticky', 'Strict Sticky \u2014 Stay on warm worker until max_concurrent is full']
        ].forEach(function(opt) {
          var o = el('option', null, opt[1]);
          o.value = opt[0];
          if (draft.spillover_mode === opt[0]) o.selected = true;
          spSel.appendChild(o);
        });
        spSel.addEventListener('change', function() { draft.spillover_mode = spSel.value; });
        spField.appendChild(spSel);
        dynBox.appendChild(spField);

        var row2 = el('div', 'mr-rt-row-2');
        var minField = el('div', 'mr-rt-field');
        minField.appendChild(el('label', null, 'Min Prefix Match (bytes)'));
        var minIn = el('input', 'mr-rt-input');
        minIn.type = 'number'; minIn.min = '64'; minIn.max = '65536'; minIn.value = String(draft.min_prefix_bytes);
        minIn.addEventListener('input', function() { draft.min_prefix_bytes = Math.max(64, parseInt(minIn.value, 10) || 512); });
        minField.appendChild(minIn);
        minField.appendChild(el('div', 'mr-rt-help', 'Default 512 bytes (~128 tokens) avoids pinning trivial short prompts.'));
        row2.appendChild(minField);

        var ttlField = el('div', 'mr-rt-field');
        ttlField.appendChild(el('label', null, 'Prefix Cache TTL (seconds)'));
        var ttlIn = el('input', 'mr-rt-input');
        ttlIn.type = 'number'; ttlIn.min = '10'; ttlIn.max = '86400'; ttlIn.value = String(draft.prefix_ttl_secs);
        ttlIn.addEventListener('input', function() { draft.prefix_ttl_secs = Math.max(10, parseInt(ttlIn.value, 10) || 900); });
        ttlField.appendChild(ttlIn);
        ttlField.appendChild(el('div', 'mr-rt-help', 'How long a warm prefix stays tracked before expiring (default 900s).'));
        row2.appendChild(ttlField);
        dynBox.appendChild(row2);

        var qLabel = el('label', 'mr-rt-check-item');
        var qCb = el('input'); qCb.type = 'checkbox'; qCb.checked = Boolean(draft.prefix_aware_queue);
        qCb.addEventListener('change', function() { draft.prefix_aware_queue = qCb.checked; });
        qLabel.appendChild(qCb);
        qLabel.appendChild(el('span', null, 'Prefix-aware queue drain (match warm queued prompts first with anti-starvation cap)'));
        dynBox.appendChild(qLabel);
      } else if (draft.strategy === 'priority_failover') {
        if (!draft.worker_priority.length && suggestedWorkerNames && suggestedWorkerNames.length) {
          draft.worker_priority = suggestedWorkerNames.slice();
        }
        var pField = el('div', 'mr-rt-field');
        pField.appendChild(el('label', null, 'Worker Priority Order (highest priority first)'));
        var prioList = el('div', 'mr-rt-prio-list');
        pField.appendChild(prioList);

        function renderPrio() {
          prioList.textContent = '';
          if (!draft.worker_priority.length) {
            prioList.appendChild(el('div', 'mr-rt-help', 'Add worker names below (e.g. rtx-5090, mac-studio).'));
          }
          draft.worker_priority.forEach(function(name, idx) {
            var item = el('div', 'mr-rt-prio-item');
            var left = el('div');
            left.style.display = 'flex'; left.style.alignItems = 'center'; left.style.gap = '8px';
            left.appendChild(el('span', 'mr-rt-prio-rank', '#' + (idx + 1)));
            left.appendChild(el('strong', null, name));
            item.appendChild(left);

            var btns = el('div', 'mr-rt-prio-btns');
            var up = el('button', 'mr-rt-icon-btn', '\u2191'); up.type = 'button'; up.disabled = idx === 0;
            up.addEventListener('click', function() {
              var tmp = draft.worker_priority[idx - 1];
              draft.worker_priority[idx - 1] = draft.worker_priority[idx];
              draft.worker_priority[idx] = tmp;
              renderPrio();
            });
            var dn = el('button', 'mr-rt-icon-btn', '\u2193'); dn.type = 'button'; dn.disabled = idx === draft.worker_priority.length - 1;
            dn.addEventListener('click', function() {
              var tmp = draft.worker_priority[idx + 1];
              draft.worker_priority[idx + 1] = draft.worker_priority[idx];
              draft.worker_priority[idx] = tmp;
              renderPrio();
            });
            var rm = el('button', 'mr-rt-icon-btn', '\u00d7'); rm.type = 'button';
            rm.addEventListener('click', function() {
              draft.worker_priority.splice(idx, 1);
              renderPrio();
            });
            btns.appendChild(up); btns.appendChild(dn); btns.appendChild(rm);
            item.appendChild(btns);
            prioList.appendChild(item);
          });
        }
        renderPrio();

        var addRow = el('div', 'mr-rt-manual-add');
        var addIn = el('input', 'mr-rt-input');
        addIn.type = 'text'; addIn.placeholder = 'Add worker or provider name (e.g. rtx-5090)';
        var addBtn = el('button', 'btn btn-sm btn-secondary', 'Add');
        addBtn.type = 'button';
        addBtn.addEventListener('click', function() {
          var v = addIn.value.trim();
          if (!v) return;
          if (draft.worker_priority.indexOf(v) === -1) {
            draft.worker_priority.push(v);
            renderPrio();
          }
          addIn.value = '';
        });
        addRow.appendChild(addIn);
        addRow.appendChild(addBtn);
        pField.appendChild(addRow);
        dynBox.appendChild(pField);

        var stLabel = el('label', 'mr-rt-check-item');
        var stCb = el('input'); stCb.type = 'checkbox'; stCb.checked = Boolean(draft.sticky_after_overflow);
        stCb.addEventListener('change', function() { draft.sticky_after_overflow = stCb.checked; });
        stLabel.appendChild(stCb);
        stLabel.appendChild(el('span', null, 'Keep overflowed conversation sticky on its warm secondary machine while idle'));
        dynBox.appendChild(stLabel);
      } else if (draft.strategy === 'least_loaded') {
        dynBox.appendChild(el('div', 'mr-rt-help', 'Routes each request to the machine with the lowest in_flight / max_concurrent ratio, breaking ties by fastest observed EWMA TTFT. Ideal for stateless workloads or embeddings.'));
      } else {
        dynBox.appendChild(el('div', 'mr-rt-help', 'Rotates deterministically across all eligible workers. Useful for benchmarking or stateless throughput tests.'));
      }
    }

    stratSel.addEventListener('change', function() {
      draft.strategy = stratSel.value;
      renderStrategyOptions();
    });
    renderStrategyOptions();

    // Collapsible Advanced Guardrails
    var details = el('details', 'mr-rt-details');
    details.appendChild(el('summary', null, 'Advanced Guardrails'));
    var dBody = el('div', 'mr-rt-details-body');
    [
      ['respect_context_window', 'Respect context window (avoid workers whose advertised context is smaller than the prompt)'],
      ['prefer_loaded_models', 'Prefer models already loaded in VRAM over unloaded/sleeping instances'],
      ['allow_client_overrides', 'Allow clients to override routing per request via provider.sort']
    ].forEach(function(item) {
      var lbl = el('label', 'mr-rt-check-item');
      var cb = el('input'); cb.type = 'checkbox'; cb.checked = Boolean(draft[item[0]]);
      cb.addEventListener('change', function() { draft[item[0]] = cb.checked; });
      lbl.appendChild(cb);
      lbl.appendChild(el('span', null, item[1]));
      dBody.appendChild(lbl);
    });
    details.appendChild(dBody);
    wrap.appendChild(details);

    return {
      element: wrap,
      getValue: function() { return cloneJson(draft); }
    };
  }

  function openPolicyModal(kind, liveModel) {
    if (!lastState || !lastState.config) return;
    showModalError('');
    var isDefault = kind === 'default';
    var titleEl = document.getElementById('mr-rt-modal-title');
    var subEl = document.getElementById('mr-rt-modal-subtitle');
    var bodyEl = document.getElementById('mr-rt-modal-body');
    var footEl = document.getElementById('mr-rt-modal-foot');

    titleEl.textContent = isDefault ? 'Edit Default Routing Policy' : ('Configure Routing \u2014 ' + liveModel.model_id);
    subEl.textContent = isDefault
      ? 'Applies automatically to every exact model without a custom override or group.'
      : 'Custom per-model override for exact model ID "' + liveModel.model_id + '".';

    var initialPolicy = isDefault
      ? lastState.config.default_policy
      : ((lastState.config.model_overrides && lastState.config.model_overrides[liveModel.model_id]) || liveModel.effective_policy);

    var suggestedWorkers = isDefault
      ? (lastState.workers || []).map(function(w) { return w.worker_name; })
      : (liveModel.workers || []).map(function(w) { return w.worker_name; });

    var dedupSuggested = [];
    suggestedWorkers.forEach(function(n) { if (n && dedupSuggested.indexOf(n) === -1) dedupSuggested.push(n); });

    var editor = buildPolicyEditor(initialPolicy, dedupSuggested);
    bodyEl.textContent = '';
    bodyEl.appendChild(editor.element);

    footEl.textContent = '';
    if (!isDefault && liveModel.has_custom_override) {
      var resetBtn = el('button', 'btn btn-sm btn-danger', 'Reset to Default');
      resetBtn.type = 'button';
      resetBtn.addEventListener('click', async function() {
        var nextCfg = cloneJson(lastState.config);
        if (nextCfg.model_overrides) delete nextCfg.model_overrides[liveModel.model_id];
        resetBtn.disabled = true;
        try {
          await saveRoutingConfig(nextCfg);
          closeModal();
          showToast('Reset "' + liveModel.model_id + '" to default routing policy', false);
        } catch (err) {
          showModalError(err.message || 'Failed to reset policy');
          resetBtn.disabled = false;
        }
      });
      footEl.appendChild(resetBtn);
    }

    var right = el('div', 'mr-rt-foot-right');
    var cancelBtn = el('button', 'btn btn-sm btn-secondary', 'Cancel');
    cancelBtn.type = 'button';
    cancelBtn.addEventListener('click', closeModal);

    var saveBtn = el('button', 'btn btn-sm', 'Save Policy');
    saveBtn.type = 'button';
    saveBtn.addEventListener('click', async function() {
      var nextCfg = cloneJson(lastState.config);
      if (isDefault) {
        nextCfg.default_policy = editor.getValue();
      } else {
        if (!nextCfg.model_overrides) nextCfg.model_overrides = {};
        nextCfg.model_overrides[liveModel.model_id] = editor.getValue();
      }
      saveBtn.disabled = true;
      try {
        await saveRoutingConfig(nextCfg);
        closeModal();
        showToast(isDefault ? 'Saved default routing policy' : ('Saved routing policy for "' + liveModel.model_id + '"'), false);
      } catch (err) {
        showModalError(err.message || 'Failed to save routing policy');
        saveBtn.disabled = false;
      }
    });

    right.appendChild(cancelBtn);
    right.appendChild(saveBtn);
    footEl.appendChild(right);

    document.getElementById('mr-rt-modal-backdrop').hidden = false;
  }

  function openGroupModal(existingGroup) {
    if (!lastState || !lastState.config) return;
    showModalError('');
    var isEdit = Boolean(existingGroup);
    var draft = isEdit ? cloneJson(existingGroup) : {
      id: '',
      name: '',
      mode: 'shared_policy',
      pool_model_id: '',
      targets: [],
      policy: cloneJson(lastState.config.default_policy || DEFAULT_POLICY)
    };
    if (!Array.isArray(draft.targets)) draft.targets = [];

    var titleEl = document.getElementById('mr-rt-modal-title');
    var subEl = document.getElementById('mr-rt-modal-subtitle');
    var bodyEl = document.getElementById('mr-rt-modal-body');
    var footEl = document.getElementById('mr-rt-modal-foot');

    titleEl.textContent = isEdit ? ('Edit Group \u2014 ' + draft.name) : 'Tie Models Together';
    subEl.textContent = 'Select exact models from your connected machines \u2014 no glob patterns required.';
    bodyEl.textContent = '';

    // Name input
    var nameField = el('div', 'mr-rt-field');
    nameField.appendChild(el('label', null, 'Group Name'));
    var nameIn = el('input', 'mr-rt-input');
    nameIn.type = 'text';
    nameIn.placeholder = 'e.g. Coding Cluster or Qwen3 32B Pool';
    nameIn.value = draft.name || '';
    nameIn.addEventListener('input', function() { draft.name = nameIn.value; });
    nameField.appendChild(nameIn);
    bodyEl.appendChild(nameField);

    // Mode toggle cards
    var modeField = el('div', 'mr-rt-field');
    modeField.appendChild(el('span', 'mr-rt-field-label', 'How should these models be tied together?'));
    var modeGrid = el('div', 'mr-rt-mode-grid');

    function makeModeCard(modeVal, titleTxt, descTxt) {
      var lbl = el('label', 'mr-rt-mode-card' + (draft.mode === modeVal ? ' active' : ''));
      var rb = el('input');
      rb.type = 'radio'; rb.name = 'mr-rt-group-mode'; rb.value = modeVal;
      rb.checked = draft.mode === modeVal;
      rb.addEventListener('change', function() {
        if (!rb.checked) return;
        draft.mode = modeVal;
        cardShared.classList.toggle('active', modeVal === 'shared_policy');
        cardPool.classList.toggle('active', modeVal === 'unified_pool');
        poolField.hidden = modeVal !== 'unified_pool';
        renderTargetCheckboxes();
      });
      var txt = el('div');
      txt.appendChild(el('strong', null, titleTxt));
      txt.appendChild(el('span', null, descTxt));
      lbl.appendChild(rb);
      lbl.appendChild(txt);
      return lbl;
    }

    var cardShared = makeModeCard(
      'shared_policy',
      'Shared Routing Policy',
      'Checked models keep their own names and share one routing strategy.'
    );
    var cardPool = makeModeCard(
      'unified_pool',
      'Unified Pooled Model',
      'Combine differently-named models across machines under one model ID (auto-rewrites JSON "model").'
    );
    modeGrid.appendChild(cardShared);
    modeGrid.appendChild(cardPool);
    modeField.appendChild(modeGrid);
    bodyEl.appendChild(modeField);

    // Unified pool model ID input
    var poolField = el('div', 'mr-rt-field');
    poolField.hidden = draft.mode !== 'unified_pool';
    poolField.appendChild(el('label', null, 'Unified Model ID (exposed in /v1/models)'));
    var poolIn = el('input', 'mr-rt-input');
    poolIn.type = 'text';
    poolIn.placeholder = 'e.g. qwen3-32b-pool';
    poolIn.value = draft.pool_model_id || '';
    poolIn.addEventListener('input', function() { draft.pool_model_id = poolIn.value; });
    poolField.appendChild(poolIn);
    poolField.appendChild(el('div', 'mr-rt-help', 'Clients can request this exact model ID; ModelRelay automatically translates it to each machine\u2019s native model name.'));
    bodyEl.appendChild(poolField);

    // Checkboxes for exact targets
    var targetsField = el('div', 'mr-rt-field');
    targetsField.appendChild(el('span', 'mr-rt-field-label', 'Models & Machines in This Group'));
    var checkBox = el('div', 'mr-rt-check-box');
    targetsField.appendChild(checkBox);

    function targetMatches(a, b) {
      return (a.worker_name || null) === (b.worker_name || null) && a.model_id === b.model_id;
    }

    function hasTarget(workerName, modelId) {
      var wn = workerName || null;
      return draft.targets.some(function(t) { return targetMatches(t, { worker_name: wn, model_id: modelId }); });
    }

    function toggleTarget(workerName, modelId, checked) {
      var wn = workerName || null;
      draft.targets = draft.targets.filter(function(t) { return !targetMatches(t, { worker_name: wn, model_id: modelId }); });
      if (checked) {
        draft.targets.push(wn ? { worker_name: wn, model_id: modelId } : { model_id: modelId });
      }
    }

    function renderTargetCheckboxes() {
      checkBox.textContent = '';
      var workers = (lastState && lastState.workers) || [];
      var listedKeys = {};

      if (draft.mode === 'unified_pool') {
        if (!workers.length) {
          checkBox.appendChild(el('div', 'mr-rt-help', 'No connected workers right now. Use the manual add input below to pre-configure targets.'));
        }
        workers.forEach(function(w) {
          var exactModels = (w.models || []).filter(function(m) { return m && m !== '*'; });
          if (!exactModels.length) return;
          checkBox.appendChild(el('div', 'mr-rt-check-group-label', w.worker_name + ' (' + w.provider + ')'));
          exactModels.forEach(function(m) {
            var key = w.worker_name + '::' + m;
            if (listedKeys[key]) return;
            listedKeys[key] = true;
            var lbl = el('label', 'mr-rt-check-item');
            var cb = el('input'); cb.type = 'checkbox';
            cb.checked = hasTarget(w.worker_name, m) || hasTarget(null, m);
            cb.addEventListener('change', function() {
              // Remove any unscoped duplicate if present and toggle scoped target
              toggleTarget(null, m, false);
              toggleTarget(w.worker_name, m, cb.checked);
            });
            lbl.appendChild(cb);
            lbl.appendChild(el('code', null, m));
            checkBox.appendChild(lbl);
          });
        });
      } else {
        // shared_policy mode: deduplicated exact models across connected workers
        var modelWorkersMap = {};
        var modelOrder = [];
        workers.forEach(function(w) {
          (w.models || []).forEach(function(m) {
            if (!m || m === '*') return;
            if (!modelWorkersMap[m]) { modelWorkersMap[m] = []; modelOrder.push(m); }
            if (modelWorkersMap[m].indexOf(w.worker_name) === -1) modelWorkersMap[m].push(w.worker_name);
          });
        });
        if (!modelOrder.length) {
          checkBox.appendChild(el('div', 'mr-rt-help', 'No connected models right now. Use the manual add input below to pre-configure targets.'));
        }
        modelOrder.forEach(function(m) {
          listedKeys['::' + m] = true;
          var lbl = el('label', 'mr-rt-check-item');
          var cb = el('input'); cb.type = 'checkbox';
          cb.checked = draft.targets.some(function(t) { return t.model_id === m; });
          cb.addEventListener('change', function() {
            draft.targets = draft.targets.filter(function(t) { return t.model_id !== m; });
            if (cb.checked) draft.targets.push({ model_id: m });
          });
          lbl.appendChild(cb);
          lbl.appendChild(el('code', null, m));
          lbl.appendChild(el('span', 'mr-rt-help', '(' + modelWorkersMap[m].join(', ') + ')'));
          checkBox.appendChild(lbl);
        });
      }

      // Preserve any already-saved offline targets so editing never drops sleeping machines
      var offlineTargets = draft.targets.filter(function(t) {
        if (draft.mode === 'unified_pool') {
          var k = (t.worker_name || '') + '::' + t.model_id;
          return !listedKeys[k];
        }
        return !listedKeys['::' + t.model_id];
      });
      if (offlineTargets.length) {
        checkBox.appendChild(el('div', 'mr-rt-check-group-label', 'Saved / Offline Targets'));
        offlineTargets.forEach(function(t) {
          var lbl = el('label', 'mr-rt-check-item');
          var cb = el('input'); cb.type = 'checkbox'; cb.checked = true;
          cb.addEventListener('change', function() {
            toggleTarget(t.worker_name || null, t.model_id, cb.checked);
          });
          lbl.appendChild(cb);
          lbl.appendChild(el('code', null, t.worker_name ? (t.worker_name + ': ' + t.model_id) : t.model_id));
          lbl.appendChild(el('span', 'mr-rt-help', '(offline)'));
          checkBox.appendChild(lbl);
        });
      }
    }

    renderTargetCheckboxes();

    // Manual offline target add row
    var manualRow = el('div', 'mr-rt-manual-add');
    var manWorker = el('input', 'mr-rt-input');
    manWorker.type = 'text'; manWorker.placeholder = 'Worker name (optional)';
    var manModel = el('input', 'mr-rt-input');
    manModel.type = 'text'; manModel.placeholder = 'Exact model ID (for offline machine)';
    var manAddBtn = el('button', 'btn btn-sm btn-secondary', 'Add Target');
    manAddBtn.type = 'button';
    manAddBtn.addEventListener('click', function() {
      var mId = manModel.value.trim();
      var wName = manWorker.value.trim();
      if (!mId || mId === '*') {
        showModalError('Enter a non-empty exact model ID (no wildcards).');
        return;
      }
      showModalError('');
      toggleTarget(wName || null, mId, true);
      manModel.value = '';
      manWorker.value = '';
      renderTargetCheckboxes();
    });
    manualRow.appendChild(manWorker);
    manualRow.appendChild(manModel);
    manualRow.appendChild(manAddBtn);
    targetsField.appendChild(manualRow);
    bodyEl.appendChild(targetsField);

    // Strategy editor for this group
    var allWorkerNames = [];
    ((lastState && lastState.workers) || []).forEach(function(w) {
      if (w.worker_name && allWorkerNames.indexOf(w.worker_name) === -1) allWorkerNames.push(w.worker_name);
    });
    var policyEditor = buildPolicyEditor(draft.policy, allWorkerNames);
    bodyEl.appendChild(policyEditor.element);

    // Footer buttons
    footEl.textContent = '';
    var right = el('div', 'mr-rt-foot-right');
    var cancelBtn = el('button', 'btn btn-sm btn-secondary', 'Cancel');
    cancelBtn.type = 'button';
    cancelBtn.addEventListener('click', closeModal);

    var saveBtn = el('button', 'btn btn-sm', isEdit ? 'Save Group' : 'Create Group');
    saveBtn.type = 'button';
    saveBtn.addEventListener('click', async function() {
      var trimmedName = (draft.name || '').trim();
      if (!trimmedName) { showModalError('Please enter a group name.'); return; }
      if (!draft.targets.length) { showModalError('Check at least one model target.'); return; }
      var poolId = draft.mode === 'unified_pool' ? ((draft.pool_model_id || '').trim() || trimmedName) : null;
      if (draft.mode === 'unified_pool' && (!poolId || poolId === '*')) {
        showModalError('Please enter a valid Unified Model ID.');
        return;
      }

      var finalGroup = {
        id: draft.id || '',
        name: trimmedName,
        mode: draft.mode,
        pool_model_id: poolId,
        targets: cloneJson(draft.targets),
        policy: policyEditor.getValue()
      };

      var nextCfg = cloneJson(lastState.config);
      if (!Array.isArray(nextCfg.groups)) nextCfg.groups = [];
      if (isEdit) {
        var idx = nextCfg.groups.findIndex(function(g) { return g.id === existingGroup.id; });
        if (idx >= 0) nextCfg.groups[idx] = finalGroup;
        else nextCfg.groups.push(finalGroup);
      } else {
        nextCfg.groups.push(finalGroup);
      }

      saveBtn.disabled = true;
      try {
        await saveRoutingConfig(nextCfg);
        closeModal();
        showToast(isEdit ? ('Updated group "' + trimmedName + '"') : ('Created group "' + trimmedName + '"'), false);
      } catch (err) {
        showModalError(err.message || 'Failed to save group');
        saveBtn.disabled = false;
      }
    });
    right.appendChild(cancelBtn);
    right.appendChild(saveBtn);
    footEl.appendChild(right);

    document.getElementById('mr-rt-modal-backdrop').hidden = false;
  }

  async function pollRouting() {
    var modelsEl = document.getElementById('mr-rt-models-body');
    if (cfg.useOssAdminAuth && !ossAdminToken()) {
      if (modelsEl) {
        modelsEl.textContent = '';
        modelsEl.appendChild(el('div', 'mr-rt-empty', 'Enter your admin token above to inspect and configure smart multi-worker routing.'));
      }
      return;
    }
    try {
      var resp = await fetch(requestEndpoint(false), fetchOptions(false));
      if (resp.status === 401 || resp.status === 403) {
        if (modelsEl) {
          modelsEl.textContent = '';
          var msg = el('div', 'mr-rt-empty', cfg.useOssAdminAuth ? 'Invalid admin token.' : 'Sign in with an active API key to manage smart routing.');
          msg.style.color = 'var(--danger)';
          modelsEl.appendChild(msg);
        }
        return;
      }
      if (!resp.ok) throw new Error('HTTP ' + resp.status);
      var data = await resp.json();
      lastState = data;
      renderAll(data);
    } catch (_) {
      if (!lastState && modelsEl) {
        modelsEl.textContent = '';
        modelsEl.appendChild(el('div', 'mr-rt-empty', 'Could not load smart routing state right now.'));
      }
    }
  }

  // Bind top-level controls
  var editDefBtn = document.getElementById('mr-rt-edit-default-btn');
  if (editDefBtn) editDefBtn.addEventListener('click', function() { openPolicyModal('default', null); });

  var newGrpBtn = document.getElementById('mr-rt-new-group-btn');
  if (newGrpBtn) newGrpBtn.addEventListener('click', function() { openGroupModal(null); });

  var closeBtn = document.getElementById('mr-rt-modal-close');
  if (closeBtn) closeBtn.addEventListener('click', closeModal);

  var backdrop = document.getElementById('mr-rt-modal-backdrop');
  if (backdrop) {
    backdrop.addEventListener('click', function(e) {
      if (e.target === backdrop) closeModal();
    });
  }

  document.addEventListener('keydown', function(e) {
    if (e.key === 'Escape' && backdrop && !backdrop.hidden) closeModal();
  });

  var dbgToggle = document.getElementById('mr-rt-debug-headers');
  if (dbgToggle) {
    dbgToggle.addEventListener('change', async function() {
      if (!lastState || !lastState.config) return;
      var nextCfg = cloneJson(lastState.config);
      nextCfg.expose_debug_headers = dbgToggle.checked;
      try {
        await saveRoutingConfig(nextCfg);
        showToast(dbgToggle.checked ? 'Enabled X-ModelRelay-* debug headers' : 'Disabled X-ModelRelay-* debug headers', false);
      } catch (err) {
        dbgToggle.checked = !dbgToggle.checked;
        showToast(err.message || 'Failed to update debug header setting', true);
      }
    });
  }

  window.__mrPollRouting = pollRouting;
  pollRouting();
  setInterval(pollRouting, POLL_MS);
})();
"#;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn renders_oss_routing_panel_with_expected_controls_and_no_globs() {
        let assets = render_routing_panel(&RoutingPanelConfig {
            get_url: String::new(),
            save_url: String::new(),
            save_method: "PUT".to_string(),
            csrf_token: None,
            use_oss_admin_auth: true,
            show_workers_table: false,
            sync_worker_kpi_id: None,
        });
        assert!(assets.css.contains(".mr-rt-telemetry"));
        assert!(assets.html.contains("Smart Multi-Worker Routing"));
        assert!(assets.html.contains("+ Tie Models Together"));
        assert!(assets.html.contains("Exact Models &amp; Unified Pools"));
        assert!(assets.html.contains("mr-rt-kpi-verified"));
        assert!(!assets.html.contains("mr-rt-workers-body"));
        assert!(assets.js.contains(r#""useOssAdminAuth":true"#));
        assert!(assets.js.contains("spill_on_busy"));
        assert!(assets.js.contains("unified_pool"));
    }

    #[test]
    fn renders_cloud_routing_panel_with_csrf_and_workers_table() {
        let assets = render_routing_panel(&RoutingPanelConfig {
            get_url: "/dashboard/routing".to_string(),
            save_url: "/dashboard/routing".to_string(),
            save_method: "POST".to_string(),
            csrf_token: Some("csrf-abc-123".to_string()),
            use_oss_admin_auth: false,
            show_workers_table: true,
            sync_worker_kpi_id: Some("sub-kpi-workers".to_string()),
        });
        assert!(assets.html.contains("mr-rt-workers-body"));
        assert!(
            assets
                .html
                .contains("Connected Machines &amp; Cache Telemetry")
        );
        assert!(assets.js.contains(r#""csrfToken":"csrf-abc-123""#));
        assert!(assets.js.contains(r#""saveMethod":"POST""#));
        assert!(assets.js.contains(r#""syncWorkerKpiId":"sub-kpi-workers""#));
    }

    #[test]
    fn escapes_closing_script_tag_in_config_json() {
        let assets = render_routing_panel(&RoutingPanelConfig {
            get_url: "/dashboard/routing".to_string(),
            save_url: "/dashboard/routing".to_string(),
            save_method: "POST".to_string(),
            csrf_token: Some("</script><script>alert(1)</script>".to_string()),
            use_oss_admin_auth: false,
            show_workers_table: false,
            sync_worker_kpi_id: None,
        });
        assert!(!assets.js.contains("</script><script>alert(1)"));
        assert!(assets.js.contains(r"<\/script>"));
    }
}
