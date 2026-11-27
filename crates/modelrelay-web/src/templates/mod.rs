mod dashboard;
mod integrate;
mod setup;

pub use dashboard::dashboard_page;
pub use integrate::{integrate_page, integrate_page_with_config};
pub use setup::{setup_wizard_page, setup_wizard_page_with_config, setup_wizard_page_with_urls};

/// Canonical dynamic Pi Coding Agent provider extension (`~/.pi/agent/extensions/modelrelay.ts`),
/// served at `https://modelrelay.io/pi/modelrelay.ts` and embedded in docs & `/integrate`.
pub const PI_EXTENSION_TS: &str = include_str!("../../assets/modelrelay.ts");

/// Optional configuration for embedding interactive pages in a cloud context.
///
/// When provided, page scripts pre-fill the public relay URL, active API key,
/// and session-authenticated worker polling endpoint.
pub struct CloudWizardConfig {
    /// The public relay server URL (e.g. `https://api.modelrelay.io`).
    pub server_url: String,
    /// The user's API key to pre-fill for curl/Pi snippets and live tests.
    pub api_key: Option<String>,
    /// The user's worker secret (same as API key in hosted mode).
    pub worker_secret: Option<String>,
    /// Proxy endpoint for polling connected workers (e.g. `/dashboard/workers`).
    pub workers_poll_url: String,
}

/// Escape untrusted text for safe insertion into HTML text nodes and attributes.
#[must_use]
pub fn html_escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&#x27;")
}

pub(crate) fn cloud_config_script(config: Option<&CloudWizardConfig>) -> String {
    let Some(cfg) = config else {
        return String::new();
    };
    let escape = |s: &str| {
        s.replace('\\', "\\\\")
            .replace('"', "\\\"")
            .replace('<', "\\u003c")
            .replace('>', "\\u003e")
    };
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

fn meta_description_for(title: &str, path: &str) -> &'static str {
    let t = title.to_lowercase();
    if t.contains("sign up") {
        "Create your ModelRelay account. Run local GPU workers, get an API key in minutes, and route OpenAI / Anthropic / Responses API traffic through your own hardware."
    } else if t.contains("log in") {
        "Log in to ModelRelay to manage your workers, API keys, and subscription."
    } else if t.contains("integrat") || t.contains("pi & api") || path.starts_with("/integrate") {
        "Drop-in OpenAI, Anthropic, and Responses API endpoints at api.modelrelay.io. Set up Pi coding agent as a dynamic provider and test with curl."
    } else if t.contains("download") || path.starts_with("/downloads") {
        "Download ModelRelay Desktop for macOS, Linux, and Windows, plus pre-built worker and server CLI binaries and Docker images."
    } else if t.contains("docs") || t.contains("documentation") || path.starts_with("/docs") {
        "ModelRelay documentation — api.modelrelay.io reference, Pi coding agent provider setup, desktop app, GPU workers, self-hosting, and architecture."
    } else if t.contains("pricing") || path == "/pricing" {
        "Simple, transparent pricing for ModelRelay. $20/month flat rate — unlimited workers, unlimited requests, no per-request fees."
    } else if t.contains("setup") || path.starts_with("/setup") {
        "Install and run a ModelRelay worker on your GPU box in minutes. Step-by-step setup for macOS, Linux, and Windows."
    } else if t.contains("dashboard") || path.starts_with("/dashboard") {
        "Your ModelRelay dashboard — manage workers, API keys, live models, and subscription billing."
    } else if t.contains("checkout") || t.contains("billing") {
        "Manage your ModelRelay subscription, billing, and payment method."
    } else {
        "ModelRelay — Run your local GPU workers behind a managed cloud relay. OpenAI, Anthropic, and Responses API compatible."
    }
}

/// Zero-FOUC theme initializer executed in `<head>` before body paint.
const THEME_INIT_JS: &str = "(function(){try{var t=localStorage.getItem('mr_theme');if(t==='light'||t==='dark'){document.documentElement.setAttribute('data-theme',t);}}catch(_){}})();";

/// Shared base stylesheet supporting system-aware light and dark modes plus a
/// manual `[data-theme]` override toggled from the top navbar.
#[allow(clippy::too_many_lines)]
const BASE_CSS: &str = r#"
    :root {
      color-scheme: dark;
      --bg-canvas: #090b10;
      --bg-surface: #10131c;
      --bg-elevated: #171c28;
      --bg-subtle: rgba(255, 255, 255, 0.028);
      --bg-hover: rgba(255, 255, 255, 0.055);
      --bg-nav: rgba(9, 11, 16, 0.84);
      --bg-drawer: rgba(16, 19, 28, 0.98);
      --bg-footer: rgba(6, 8, 13, 0.72);
      --bg-section-alt: rgba(16, 19, 28, 0.48);
      --bg-input: #06080d;
      --bg-code: #06080d;
      --bg-code-head: rgba(255, 255, 255, 0.03);
      --code-fg: #e2e8f0;
      --code-muted: #94a3b8;
      --code-border: rgba(148, 163, 184, 0.16);
      --inline-code-bg: rgba(148, 163, 184, 0.11);
      --inline-code-fg: #cbd5e1;
      --border: rgba(148, 163, 184, 0.14);
      --border-strong: rgba(148, 163, 184, 0.26);
      --text-strong: #ffffff;
      --text-primary: #f1f5f9;
      --text-secondary: #94a3b8;
      --text-muted: #64748b;
      --accent: #6366f1;
      --accent-hover: #5254e8;
      --accent-bright: #818cf8;
      --accent-fg: #c7d2fe;
      --accent-soft: rgba(99, 102, 241, 0.14);
      --accent-border: rgba(99, 102, 241, 0.36);
      --cyan: #38bdf8;
      --cyan-fg: #7dd3fc;
      --cyan-soft: rgba(56, 189, 248, 0.12);
      --success: #34d399;
      --success-fg: #a7f3d0;
      --success-soft: rgba(52, 211, 153, 0.12);
      --warning: #fbbf24;
      --warning-fg: #fde68a;
      --warning-soft: rgba(251, 191, 36, 0.12);
      --danger: #f87171;
      --danger-fg: #fca5a5;
      --danger-soft: rgba(248, 113, 113, 0.12);
      --callout-fg: #dbeafe;
      --hero-hl-from: #a5b4fc;
      --hero-hl-to: #38bdf8;
      --radius-sm: 6px;
      --radius-md: 10px;
      --radius-lg: 14px;
      --radius-xl: 18px;
      --font-sans: -apple-system, BlinkMacSystemFont, "Inter", "Segoe UI", Roboto, Helvetica, Arial, sans-serif;
      --font-mono: ui-monospace, "SFMono-Regular", Menlo, Monaco, Consolas, "Liberation Mono", monospace;
      --shadow-card: 0 1px 2px rgba(0, 0, 0, 0.35), 0 12px 32px rgba(0, 0, 0, 0.25);
      --shadow-hero: 0 24px 60px rgba(0, 0, 0, 0.5), 0 0 0 1px rgba(99, 102, 241, 0.14);
    }

    @media (prefers-color-scheme: light) {
      :root:not([data-theme="dark"]) {
        color-scheme: light;
        --bg-canvas: #f8f6f2;
        --bg-surface: #ffffff;
        --bg-elevated: #f1eee8;
        --bg-subtle: rgba(15, 23, 42, 0.03);
        --bg-hover: rgba(15, 23, 42, 0.055);
        --bg-nav: rgba(248, 246, 242, 0.88);
        --bg-drawer: rgba(255, 255, 255, 0.98);
        --bg-footer: #f1eee8;
        --bg-section-alt: #f2efe9;
        --bg-input: #ffffff;
        --bg-code: #0f172a;
        --bg-code-head: #1e293b;
        --code-fg: #f1f5f9;
        --code-muted: #94a3b8;
        --code-border: rgba(15, 23, 42, 0.16);
        --inline-code-bg: rgba(15, 23, 42, 0.065);
        --inline-code-fg: #1e293b;
        --border: rgba(15, 23, 42, 0.1);
        --border-strong: rgba(15, 23, 42, 0.2);
        --text-strong: #0f172a;
        --text-primary: #1e293b;
        --text-secondary: #475569;
        --text-muted: #64748b;
        --accent: #4338ca;
        --accent-hover: #3730a3;
        --accent-bright: #4338ca;
        --accent-fg: #3730a3;
        --accent-soft: rgba(67, 56, 202, 0.09);
        --accent-border: rgba(67, 56, 202, 0.28);
        --cyan: #0284c7;
        --cyan-fg: #0369a1;
        --cyan-soft: rgba(2, 132, 199, 0.1);
        --success: #059669;
        --success-fg: #065f46;
        --success-soft: rgba(5, 150, 105, 0.1);
        --warning: #d97706;
        --warning-fg: #92400e;
        --warning-soft: rgba(217, 119, 6, 0.11);
        --danger: #dc2626;
        --danger-fg: #991b1b;
        --danger-soft: rgba(220, 38, 38, 0.1);
        --callout-fg: #1e3a8a;
        --hero-hl-from: #234071;
        --hero-hl-to: #4338ca;
        --shadow-card: 0 1px 2px rgba(15, 23, 42, 0.05), 0 8px 24px rgba(15, 23, 42, 0.06);
        --shadow-hero: 0 20px 50px rgba(15, 23, 42, 0.14), 0 0 0 1px rgba(15, 23, 42, 0.08);
      }
    }

    :root[data-theme="light"] {
      color-scheme: light;
      --bg-canvas: #f8f6f2;
      --bg-surface: #ffffff;
      --bg-elevated: #f1eee8;
      --bg-subtle: rgba(15, 23, 42, 0.03);
      --bg-hover: rgba(15, 23, 42, 0.055);
      --bg-nav: rgba(248, 246, 242, 0.88);
      --bg-drawer: rgba(255, 255, 255, 0.98);
      --bg-footer: #f1eee8;
      --bg-section-alt: #f2efe9;
      --bg-input: #ffffff;
      --bg-code: #0f172a;
      --bg-code-head: #1e293b;
      --code-fg: #f1f5f9;
      --code-muted: #94a3b8;
      --code-border: rgba(15, 23, 42, 0.16);
      --inline-code-bg: rgba(15, 23, 42, 0.065);
      --inline-code-fg: #1e293b;
      --border: rgba(15, 23, 42, 0.1);
      --border-strong: rgba(15, 23, 42, 0.2);
      --text-strong: #0f172a;
      --text-primary: #1e293b;
      --text-secondary: #475569;
      --text-muted: #64748b;
      --accent: #4338ca;
      --accent-hover: #3730a3;
      --accent-bright: #4338ca;
      --accent-fg: #3730a3;
      --accent-soft: rgba(67, 56, 202, 0.09);
      --accent-border: rgba(67, 56, 202, 0.28);
      --cyan: #0284c7;
      --cyan-fg: #0369a1;
      --cyan-soft: rgba(2, 132, 199, 0.1);
      --success: #059669;
      --success-fg: #065f46;
      --success-soft: rgba(5, 150, 105, 0.1);
      --warning: #d97706;
      --warning-fg: #92400e;
      --warning-soft: rgba(217, 119, 6, 0.11);
      --danger: #dc2626;
      --danger-fg: #991b1b;
      --danger-soft: rgba(220, 38, 38, 0.1);
      --callout-fg: #1e3a8a;
      --hero-hl-from: #234071;
      --hero-hl-to: #4338ca;
      --shadow-card: 0 1px 2px rgba(15, 23, 42, 0.05), 0 8px 24px rgba(15, 23, 42, 0.06);
      --shadow-hero: 0 20px 50px rgba(15, 23, 42, 0.14), 0 0 0 1px rgba(15, 23, 42, 0.08);
    }

    /* Theme toggle icons: dark shows Sun (switch to light), light shows Moon (switch to dark) */
    .icon-sun { display: block; }
    .icon-moon { display: none; }
    @media (prefers-color-scheme: light) {
      :root:not([data-theme="dark"]) .icon-sun { display: none; }
      :root:not([data-theme="dark"]) .icon-moon { display: block; }
    }
    :root[data-theme="light"] .icon-sun { display: none; }
    :root[data-theme="light"] .icon-moon { display: block; }

    *, *::before, *::after { box-sizing: border-box; margin: 0; padding: 0; }
    html { scroll-behavior: smooth; }
    body {
      font-family: var(--font-sans);
      background: var(--bg-canvas);
      color: var(--text-primary);
      line-height: 1.6;
      min-height: 100vh;
      display: flex;
      flex-direction: column;
      -webkit-font-smoothing: antialiased;
      -moz-osx-font-smoothing: grayscale;
      overflow-x: hidden;
    }

    a { color: var(--accent-bright); text-decoration: none; transition: color 0.15s ease; }
    a:hover { color: var(--accent-hover); text-decoration: underline; }
    code, pre { font-family: var(--font-mono); }
    :not(pre) > code {
      background: var(--inline-code-bg);
      border: 1px solid var(--border);
      padding: 0.12em 0.42em;
      border-radius: 5px;
      font-size: 0.86em;
      color: var(--inline-code-fg);
    }

    /* Skip navigation */
    .skip-nav {
      position: absolute; top: -100%; left: 16px; z-index: 1000;
      background: var(--accent); color: #fff; padding: 8px 16px; border-radius: 0 0 8px 8px;
      font-size: 0.875rem; font-weight: 600; text-decoration: none;
    }
    .skip-nav:focus { top: 0; text-decoration: none; }

    :focus-visible { outline: 2px solid var(--accent-bright); outline-offset: 2px; }
    input:focus-visible, select:focus-visible, textarea:focus-visible { outline: none; }

    /* Containers */
    .container { max-width: 1140px; margin: 0 auto; padding: 0 24px; width: 100%; }
    .container--narrow { max-width: 960px; }

    /* ── Unified Top Navigation ── */
    .site-nav {
      position: sticky; top: 0; z-index: 100;
      background: var(--bg-nav);
      backdrop-filter: blur(14px);
      -webkit-backdrop-filter: blur(14px);
      border-bottom: 1px solid var(--border);
    }
    .site-nav-inner {
      display: flex; align-items: center; justify-content: space-between;
      height: 64px; gap: 20px;
    }
    .brand {
      display: inline-flex; align-items: center; gap: 10px;
      font-size: 1.05rem; font-weight: 700; letter-spacing: -0.02em;
      color: var(--text-strong); text-decoration: none; flex-shrink: 0;
    }
    .brand:hover { color: var(--text-strong); text-decoration: none; }
    .brand-icon {
      width: 28px; height: 28px; border-radius: 7px; display: block;
      box-shadow: 0 1px 4px rgba(0, 0, 0, 0.2);
    }
    .brand-accent { color: var(--accent-bright); }

    .nav-center { display: flex; align-items: center; gap: 4px; }
    .nav-item {
      display: inline-flex; align-items: center; gap: 6px;
      padding: 6px 12px; border-radius: 8px;
      font-size: 0.875rem; font-weight: 500; color: var(--text-secondary);
      text-decoration: none; transition: color 0.15s ease, background 0.15s ease;
    }
    .nav-item:hover { color: var(--text-strong); background: var(--bg-hover); text-decoration: none; }
    .nav-item.active { color: var(--text-strong); background: var(--accent-soft); font-weight: 600; }

    .nav-right { display: flex; align-items: center; gap: 8px; flex-shrink: 0; }
    .nav-icon-btn {
      display: inline-flex; align-items: center; justify-content: center;
      width: 34px; height: 34px; border-radius: 8px;
      background: transparent; cursor: pointer;
      color: var(--text-secondary); border: 1px solid transparent;
      transition: color 0.15s, background 0.15s, border-color 0.15s;
    }
    .nav-icon-btn:hover {
      color: var(--text-strong); background: var(--bg-hover);
      border-color: var(--border); text-decoration: none;
    }
    .nav-dash-btn {
      display: inline-flex; align-items: center; gap: 7px;
      padding: 6px 14px; border-radius: 8px;
      font-size: 0.85rem; font-weight: 600;
      background: var(--accent); color: #fff; border: 1px solid rgba(255, 255, 255, 0.12);
      text-decoration: none; transition: background 0.15s, box-shadow 0.15s;
    }
    .nav-dash-btn:hover {
      background: var(--accent-hover); color: #fff; text-decoration: none;
      box-shadow: 0 0 18px rgba(99, 102, 241, 0.3);
    }
    .nav-dash-btn.active {
      background: var(--accent-soft);
      border-color: var(--accent-border);
      color: var(--accent-fg);
    }
    .nav-dash-dot {
      width: 7px; height: 7px; border-radius: 50%; background: var(--success);
      box-shadow: 0 0 8px rgba(52, 211, 153, 0.65);
    }
    .nav-login-btn {
      display: inline-flex; align-items: center; gap: 6px;
      padding: 6px 14px; border-radius: 8px;
      font-size: 0.85rem; font-weight: 600;
      background: var(--bg-hover); color: var(--text-strong);
      border: 1px solid var(--border-strong); text-decoration: none;
      transition: background 0.15s, border-color 0.15s;
    }
    .nav-login-btn:hover {
      background: var(--accent-soft); border-color: var(--accent-border);
      color: var(--text-strong); text-decoration: none;
    }
    .nav-logout-form { display: inline-flex; }
    .nav-logout-btn {
      padding: 6px 11px; border-radius: 8px;
      background: transparent; border: 1px solid transparent;
      color: var(--text-muted); font-size: 0.84rem; font-weight: 500;
      cursor: pointer; font-family: inherit;
      transition: color 0.15s, background 0.15s, border-color 0.15s;
    }
    .nav-logout-btn:hover {
      color: var(--text-strong); background: var(--bg-hover);
      border-color: var(--border);
    }

    .nav-mobile-tools { display: none; align-items: center; gap: 6px; }

    /* Mobile Hamburger + Drawer */
    .nav-hamburger {
      display: flex; background: transparent; border: 1px solid var(--border);
      border-radius: 8px; cursor: pointer; width: 38px; height: 38px;
      flex-direction: column; justify-content: center; align-items: center; gap: 5px;
    }
    .nav-hamburger span {
      display: block; width: 18px; height: 1.75px; background: var(--text-secondary);
      border-radius: 2px; transition: transform 0.2s, opacity 0.2s;
    }
    .nav-hamburger.open span:nth-child(1) { transform: translateY(6.75px) rotate(45deg); }
    .nav-hamburger.open span:nth-child(2) { opacity: 0; }
    .nav-hamburger.open span:nth-child(3) { transform: translateY(-6.75px) rotate(-45deg); }
    .nav-mobile-drawer {
      display: none; flex-direction: column; gap: 4px;
      padding: 12px 24px 20px; border-top: 1px solid var(--border);
      background: var(--bg-drawer);
    }
    .nav-mobile-drawer.open { display: flex; }
    .nav-mobile-drawer .nav-item { padding: 10px 12px; font-size: 0.95rem; }
    .nav-mobile-actions {
      display: flex; flex-direction: column; gap: 8px;
      margin-top: 10px; padding-top: 12px; border-top: 1px solid var(--border);
    }
    .nav-mobile-actions .nav-dash-btn,
    .nav-mobile-actions .nav-login-btn,
    .nav-mobile-actions .nav-logout-btn {
      justify-content: center; width: 100%; padding: 10px 14px;
    }

    /* ── Main Content ── */
    main { flex: 1; }
    .content { padding: 48px 0 80px; }
    .content h1 { font-size: 2rem; font-weight: 700; letter-spacing: -0.025em; margin-bottom: 16px; color: var(--text-strong); }
    .subtitle { color: var(--text-secondary); font-size: 1rem; margin-bottom: 28px; }

    /* ── Reusable Surfaces & Components ── */
    .card {
      background: var(--bg-surface);
      border: 1px solid var(--border);
      border-radius: var(--radius-lg);
      padding: 26px;
      margin-bottom: 20px;
      box-shadow: var(--shadow-card);
    }
    .card h2 { font-size: 1.15rem; font-weight: 600; margin-bottom: 10px; color: var(--text-strong); }
    .card p { color: var(--text-secondary); }
    .back-link { margin-top: 16px; font-size: 0.9rem; }

    .btn {
      display: inline-flex; align-items: center; justify-content: center; gap: 8px;
      padding: 10px 20px; border-radius: 9px;
      background: var(--accent); color: #fff; border: 1px solid rgba(255, 255, 255, 0.12);
      font-size: 0.92rem; font-weight: 600; font-family: inherit;
      cursor: pointer; text-decoration: none;
      transition: background 0.15s ease, box-shadow 0.15s ease, transform 0.1s ease;
    }
    .btn:hover {
      background: var(--accent-hover); color: #fff; text-decoration: none;
      box-shadow: 0 4px 18px rgba(67, 56, 202, 0.28);
    }
    .btn:disabled { opacity: 0.55; cursor: not-allowed; }
    .btn-lg { padding: 12px 24px; font-size: 0.96rem; }
    .btn-sm { padding: 6px 13px; font-size: 0.82rem; border-radius: 7px; }
    .btn-secondary {
      background: var(--bg-surface); color: var(--text-strong);
      border: 1px solid var(--border-strong);
    }
    .btn-secondary:hover {
      background: var(--bg-elevated); border-color: var(--accent-border);
      color: var(--text-strong); box-shadow: none;
    }
    .btn-danger {
      background: var(--danger-soft); color: var(--danger-fg);
      border: 1px solid rgba(220, 38, 38, 0.3);
    }
    .btn-danger:hover { background: rgba(220, 38, 38, 0.2); box-shadow: none; color: var(--danger-fg); }

    .badge {
      display: inline-flex; align-items: center; gap: 6px;
      padding: 3px 10px; border-radius: 999px;
      font-size: 0.76rem; font-weight: 600; letter-spacing: 0.01em;
      background: var(--bg-hover); color: var(--text-secondary);
      border: 1px solid var(--border);
    }
    .badge-active { background: var(--success-soft); color: var(--success-fg); border-color: rgba(5, 150, 105, 0.3); }
    .badge-warn { background: var(--warning-soft); color: var(--warning-fg); border-color: rgba(217, 119, 6, 0.3); }
    .badge-cancel { background: var(--danger-soft); color: var(--danger-fg); border-color: rgba(220, 38, 38, 0.3); }
    .badge-accent { background: var(--accent-soft); color: var(--accent-fg); border-color: var(--accent-border); }
    .badge-cyan { background: var(--cyan-soft); color: var(--cyan-fg); border-color: rgba(2, 132, 199, 0.3); }

    /* ── Code Blocks (crisp terminal surface in both light & dark themes) ── */
    .code-shell {
      background: var(--bg-code);
      border: 1px solid var(--code-border);
      border-radius: var(--radius-md);
      overflow: hidden;
      margin: 12px 0 18px;
    }
    .code-shell-header {
      display: flex; align-items: center; justify-content: space-between; gap: 12px;
      padding: 9px 14px; background: var(--bg-code-head);
      border-bottom: 1px solid rgba(148, 163, 184, 0.16);
      font-size: 0.78rem; color: var(--code-muted); font-family: var(--font-mono);
    }
    .code-shell-header a { color: #a5b4fc; }
    .code-shell pre {
      margin: 0; padding: 16px; overflow-x: auto;
      font-size: 0.84rem; line-height: 1.65; color: var(--code-fg);
      scrollbar-width: thin; scrollbar-color: rgba(148, 163, 184, 0.35) transparent;
    }
    pre {
      scrollbar-width: thin; scrollbar-color: rgba(148, 163, 184, 0.35) transparent;
    }
    .code-block {
      background: var(--bg-code);
      border: 1px solid var(--code-border);
      border-radius: var(--radius-md);
      padding: 15px 68px 15px 16px;
      font-family: var(--font-mono);
      font-size: 0.83rem;
      line-height: 1.65;
      color: var(--code-fg);
      overflow-x: auto;
      position: relative;
      margin: 10px 0 16px;
      white-space: pre;
    }
    .copy-btn {
      display: inline-flex; align-items: center; gap: 5px;
      padding: 4px 10px; border-radius: 6px;
      background: var(--bg-hover);
      border: 1px solid var(--border-strong);
      color: var(--text-secondary);
      font-size: 0.74rem; font-weight: 600; font-family: var(--font-sans);
      cursor: pointer; transition: all 0.15s ease;
    }
    .copy-btn:hover { background: var(--accent-soft); border-color: var(--accent-border); color: var(--text-strong); }
    .copy-btn.copied { background: var(--success-soft); border-color: rgba(5, 150, 105, 0.45); color: var(--success-fg); }
    .code-shell .copy-btn, .code-block > .copy-btn, .home-panel .copy-btn, .prompt-box .copy-btn {
      background: rgba(255, 255, 255, 0.08);
      border-color: rgba(148, 163, 184, 0.3);
      color: #cbd5e1;
    }
    .code-shell .copy-btn:hover, .code-block > .copy-btn:hover, .home-panel .copy-btn:hover, .prompt-box .copy-btn:hover {
      background: rgba(255, 255, 255, 0.16); color: #fff;
    }
    .code-shell .copy-btn.copied, .code-block > .copy-btn.copied, .home-panel .copy-btn.copied, .prompt-box .copy-btn.copied {
      background: rgba(52, 211, 153, 0.18); border-color: rgba(52, 211, 153, 0.5); color: #6ee7b7;
    }
    .code-block > .copy-btn { position: absolute; top: 9px; right: 9px; }

    /* Tables & Key Display */
    .info-table { margin-top: 12px; width: 100%; border-collapse: collapse; }
    .info-table td {
      padding: 9px 0; border-bottom: 1px solid var(--border);
      color: var(--text-secondary); font-size: 0.9rem;
    }
    .info-table tr:last-child td { border-bottom: none; }
    .info-table td:first-child { font-weight: 600; color: var(--text-strong); width: 148px; }
    .key-display {
      padding: 12px 14px; background: var(--bg-input);
      border: 1px solid var(--border); border-radius: var(--radius-md);
      font-family: var(--font-mono); font-size: 0.86rem; color: var(--accent-fg);
      word-break: break-all;
    }
    .error-msg {
      background: var(--danger-soft); border: 1px solid rgba(220, 38, 38, 0.35);
      border-radius: var(--radius-md); padding: 11px 14px; margin-bottom: 16px;
      color: var(--danger-fg); font-size: 0.9rem;
    }

    /* Auth Split Layout */
    .auth-split {
      display: grid; grid-template-columns: 1.05fr 0.95fr; gap: 40px;
      min-height: calc(100vh - 280px); align-items: center;
    }
    .auth-value-headline {
      font-size: 2.3rem; font-weight: 800; letter-spacing: -0.03em; line-height: 1.14;
      color: var(--text-strong); margin-bottom: 14px;
    }
    .auth-value-sub { font-size: 1.02rem; color: var(--text-secondary); margin-bottom: 28px; max-width: 460px; }
    .auth-benefits { list-style: none; display: flex; flex-direction: column; gap: 12px; }
    .auth-benefits li {
      display: flex; align-items: flex-start; gap: 11px;
      color: var(--text-primary); font-size: 0.94rem; line-height: 1.5;
    }
    .auth-benefits li svg { flex-shrink: 0; margin-top: 3px; }
    .auth-benefits li > span { min-width: 0; flex: 1; }
    .auth-form-panel {
      background: var(--bg-surface); border: 1px solid var(--border);
      border-radius: var(--radius-xl); padding: 36px; box-shadow: var(--shadow-card);
    }
    .auth-form-inner h2 { font-size: 1.35rem; font-weight: 700; margin-bottom: 20px; color: var(--text-strong); }
    .auth-form .form-group { margin-bottom: 16px; }
    .auth-form label { display: block; font-size: 0.85rem; color: var(--text-secondary); margin-bottom: 6px; font-weight: 500; }
    .auth-form input {
      width: 100%; padding: 11px 14px; background: var(--bg-input);
      border: 1px solid var(--border-strong); border-radius: 9px;
      color: var(--text-primary); font-size: 0.95rem; font-family: inherit;
      transition: border-color 0.15s, box-shadow 0.15s;
    }
    .auth-form input:focus { border-color: var(--accent); box-shadow: 0 0 0 3px var(--accent-soft); }
    .auth-submit { width: 100%; margin-top: 8px; padding: 12px 20px; font-size: 0.96rem; }
    .auth-submit.loading .spinner {
      display: inline-block; width: 14px; height: 14px;
      border: 2px solid rgba(255, 255, 255, 0.3); border-top-color: #fff;
      border-radius: 50%; animation: spin 0.6s linear infinite; margin-right: 6px;
    }
    @keyframes spin { to { transform: rotate(360deg); } }
    .password-wrapper { position: relative; }
    .password-wrapper input { padding-right: 44px; }
    .password-toggle {
      position: absolute; right: 10px; top: 50%; transform: translateY(-50%);
      background: none; border: none; cursor: pointer; padding: 4px;
      display: flex; align-items: center; justify-content: center;
    }
    .auth-switch { margin-top: 18px; text-align: center; color: var(--text-muted); font-size: 0.86rem; }

    /* ── Unified Footer ── */
    .site-footer {
      border-top: 1px solid var(--border);
      background: var(--bg-footer);
      padding: 56px 0 40px;
      margin-top: auto;
    }
    .footer-grid {
      display: grid;
      grid-template-columns: 1.8fr 1fr 1fr 1fr;
      gap: 36px;
      margin-bottom: 36px;
    }
    .footer-brand p {
      color: var(--text-muted); font-size: 0.88rem; margin-top: 12px; max-width: 300px; line-height: 1.6;
    }
    .footer-col h4 {
      font-size: 0.74rem; font-weight: 700; text-transform: uppercase;
      letter-spacing: 0.08em; color: var(--text-muted); margin-bottom: 14px;
    }
    .footer-col ul { list-style: none; display: flex; flex-direction: column; gap: 9px; }
    .footer-col a { color: var(--text-secondary); font-size: 0.88rem; }
    .footer-col a:hover { color: var(--text-strong); text-decoration: none; }
    .footer-bottom {
      display: flex; justify-content: space-between; align-items: center; flex-wrap: wrap; gap: 12px;
      padding-top: 24px; border-top: 1px solid var(--border);
      color: var(--text-muted); font-size: 0.82rem;
    }

    @media (max-width: 860px) {
      .nav-center, .nav-right { display: none; }
      .nav-mobile-tools { display: flex; }
      .auth-split { grid-template-columns: 1fr; gap: 28px; }
      .footer-grid { grid-template-columns: 1fr 1fr; }
    }
    @media (max-width: 560px) {
      .container { padding: 0 16px; }
      .content { padding: 32px 0 56px; }
      .card { padding: 20px; }
      .auth-form-panel { padding: 24px; }
      .footer-grid { grid-template-columns: 1fr; gap: 24px; }
      .info-table td { display: block; padding: 4px 0; }
      .info-table td:first-child { width: auto; border-bottom: none; }
    }
"#;

const BASE_JS: &str = r"
(function() {
  var btn = document.getElementById('nav-hamburger');
  var drawer = document.getElementById('nav-mobile-drawer');
  if (btn && drawer) {
    btn.addEventListener('click', function() {
      var open = btn.classList.toggle('open');
      drawer.classList.toggle('open', open);
      btn.setAttribute('aria-expanded', open ? 'true' : 'false');
    });
  }
  function effectiveTheme() {
    var attr = document.documentElement.getAttribute('data-theme');
    if (attr === 'light' || attr === 'dark') return attr;
    return window.matchMedia && window.matchMedia('(prefers-color-scheme: light)').matches ? 'light' : 'dark';
  }
  document.querySelectorAll('.js-theme-toggle').forEach(function(toggle) {
    toggle.addEventListener('click', function() {
      var next = effectiveTheme() === 'light' ? 'dark' : 'light';
      document.documentElement.setAttribute('data-theme', next);
      try { localStorage.setItem('mr_theme', next); } catch (_) {}
    });
  });
  document.addEventListener('click', function(e) {
    var copyBtn = e.target.closest('[data-copy-target],[data-copy-text]');
    if (!copyBtn) return;
    var text = copyBtn.getAttribute('data-copy-text');
    if (text === null) {
      var targetId = copyBtn.getAttribute('data-copy-target');
      var el = targetId ? document.getElementById(targetId) : null;
      if (!el) return;
      text = el.textContent || '';
    }
    text = text.trim();
    if (!text) return;
    navigator.clipboard.writeText(text).then(function() {
      var prev = copyBtn.textContent;
      copyBtn.textContent = 'Copied';
      copyBtn.classList.add('copied');
      setTimeout(function() {
        copyBtn.textContent = prev;
        copyBtn.classList.remove('copied');
      }, 1600);
    });
  });
})();
";

const GITHUB_SVG: &str = r#"<svg width="18" height="18" viewBox="0 0 24 24" fill="currentColor" aria-hidden="true"><path d="M12 .5C5.65.5.5 5.65.5 12c0 5.09 3.3 9.4 7.88 10.92.58.1.79-.25.79-.56v-2.16c-3.2.7-3.88-1.37-3.88-1.37-.52-1.33-1.28-1.69-1.28-1.69-1.05-.72.08-.7.08-.7 1.16.08 1.77 1.19 1.77 1.19 1.03 1.76 2.7 1.25 3.36.96.1-.75.4-1.25.73-1.54-2.55-.29-5.24-1.28-5.24-5.68 0-1.25.45-2.28 1.18-3.08-.12-.29-.51-1.46.11-3.04 0 0 .97-.31 3.16 1.18a10.9 10.9 0 0 1 5.76 0c2.2-1.49 3.16-1.18 3.16-1.18.63 1.58.24 2.75.12 3.04.74.8 1.18 1.83 1.18 3.08 0 4.41-2.7 5.39-5.27 5.67.41.36.78 1.07.78 2.15v3.19c0 .31.21.67.8.56A11.5 11.5 0 0 0 23.5 12C23.5 5.65 18.35.5 12 .5Z"/></svg>"#;

const THEME_TOGGLE_BTN: &str = r#"<button type="button" class="nav-icon-btn js-theme-toggle" aria-label="Toggle light and dark theme" title="Toggle light / dark theme"><svg class="icon-sun" width="17" height="17" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true"><circle cx="12" cy="12" r="4"/><path d="M12 2v2M12 20v2M4.93 4.93l1.41 1.41M17.66 17.66l1.41 1.41M2 12h2M20 12h2M6.34 17.66l-1.41 1.41M19.07 4.93l-1.41 1.41"/></svg><svg class="icon-moon" width="17" height="17" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true"><path d="M21 12.79A9 9 0 1 1 11.21 3 7 7 0 0 0 21 12.79z"/></svg></button>"#;

fn nav_active(path: &str, prefix: &str) -> &'static str {
    if prefix == "/" {
        if path == "/" { " active" } else { "" }
    } else if path == prefix || path.starts_with(&format!("{prefix}/")) {
        " active"
    } else {
        ""
    }
}

/// Full-width page shell where the caller owns `<main>` sections and `.container`
/// blocks (used by Landing, Downloads, Docs, Pricing).
///
/// Shares the exact same unified navbar, theme toggle, design system, metadata,
/// and footer as [`page_shell_custom`] and [`page_shell`].
#[must_use]
#[allow(clippy::too_many_lines)]
pub fn page_shell_full(
    title: &str,
    path: &str,
    main_html: &str,
    logged_in: bool,
    extra_head: &str,
    extra_body_end: &str,
) -> String {
    let full_title = if title.contains("ModelRelay") {
        title.to_string()
    } else {
        format!("{title} — ModelRelay")
    };
    let description = meta_description_for(title, path);

    let act_home = nav_active(path, "/");
    let act_integrate = nav_active(path, "/integrate");
    let act_downloads = nav_active(path, "/downloads");
    let act_docs = nav_active(path, "/docs");
    let act_setup = nav_active(path, "/setup");
    let act_dash = nav_active(path, "/dashboard");

    let right_actions = if logged_in {
        format!(
            r#"{THEME_TOGGLE_BTN}
        <a href="https://github.com/ericflo/modelrelay" target="_blank" rel="noopener" class="nav-icon-btn" aria-label="GitHub">{GITHUB_SVG}</a>
        <a href="/dashboard" class="nav-dash-btn{act_dash}"><span class="nav-dash-dot" aria-hidden="true"></span>Dashboard</a>
        <form method="POST" action="/logout" class="nav-logout-form"><button type="submit" class="nav-logout-btn">Log out</button></form>"#
        )
    } else {
        format!(
            r#"{THEME_TOGGLE_BTN}
        <a href="https://github.com/ericflo/modelrelay" target="_blank" rel="noopener" class="nav-icon-btn" aria-label="GitHub">{GITHUB_SVG}</a>
        <a href="/login" class="nav-login-btn">Log in</a>"#
        )
    };

    let mobile_actions = if logged_in {
        format!(
            r#"<a href="/dashboard" class="nav-dash-btn{act_dash}"><span class="nav-dash-dot" aria-hidden="true"></span>Dashboard</a>
        <form method="POST" action="/logout"><button type="submit" class="nav-logout-btn">Log out</button></form>"#
        )
    } else {
        r#"<a href="/login" class="nav-login-btn">Log in</a>"#.to_string()
    };

    format!(
        r##"<!DOCTYPE html>
<html lang="en">
<head>
  <meta charset="utf-8">
  <meta name="viewport" content="width=device-width, initial-scale=1">
  <title>{full_title}</title>
  <meta name="description" content="{description}">
  <meta property="og:title" content="{full_title}">
  <meta property="og:description" content="{description}">
  <meta property="og:type" content="website">
  <meta property="og:url" content="https://modelrelay.io{path}">
  <meta property="og:image" content="https://modelrelay.io/og-image.png">
  <meta property="og:image:width" content="1200">
  <meta property="og:image:height" content="630">
  <meta property="og:image:alt" content="ModelRelay — One Endpoint for All Your GPU Boxes">
  <link rel="canonical" href="https://modelrelay.io{path}">
  <meta name="twitter:card" content="summary_large_image">
  <meta name="twitter:title" content="{full_title}">
  <meta name="twitter:description" content="{description}">
  <meta name="twitter:image" content="https://modelrelay.io/og-image.png">
  <meta name="twitter:image:alt" content="ModelRelay — One Endpoint for All Your GPU Boxes">
  <link rel="icon" type="image/png" sizes="32x32" href="/favicon-32.png">
  <link rel="icon" type="image/png" sizes="512x512" href="/icon-512.png">
  <link rel="apple-touch-icon" sizes="180x180" href="/apple-touch-icon.png">
  <link rel="shortcut icon" href="/favicon.ico">
  <script>{THEME_INIT_JS}</script>
  <script type="application/ld+json">
  {{
    "@context": "https://schema.org",
    "@type": "Organization",
    "name": "ModelRelay",
    "url": "https://modelrelay.io",
    "logo": "https://modelrelay.io/icon-512.png",
    "sameAs": [
      "https://github.com/ericflo/modelrelay"
    ]
  }}
  </script>
  <style>{BASE_CSS}</style>
  {extra_head}
</head>
<body>
  <a href="#main-content" class="skip-nav">Skip to content</a>
  <header class="site-nav">
    <nav class="container site-nav-inner" role="navigation" aria-label="Main navigation">
      <a href="/" class="brand">
        <img src="/favicon-32.png" alt="" width="28" height="28" class="brand-icon">
        <span>Model<span class="brand-accent">Relay</span></span>
      </a>
      <div class="nav-center">
        <a href="/" class="nav-item{act_home}">Overview</a>
        <a href="/integrate" class="nav-item{act_integrate}">Pi &amp; API</a>
        <a href="/downloads" class="nav-item{act_downloads}">Downloads</a>
        <a href="/docs" class="nav-item{act_docs}">Docs</a>
        <a href="/setup" class="nav-item{act_setup}">Worker Setup</a>
      </div>
      <div class="nav-right">
        {right_actions}
      </div>
      <div class="nav-mobile-tools">
        {THEME_TOGGLE_BTN}
        <button type="button" class="nav-hamburger" id="nav-hamburger" aria-label="Toggle navigation" aria-expanded="false">
          <span></span><span></span><span></span>
        </button>
      </div>
    </nav>
    <div class="nav-mobile-drawer" id="nav-mobile-drawer">
      <a href="/" class="nav-item{act_home}">Overview</a>
      <a href="/integrate" class="nav-item{act_integrate}">Pi &amp; API</a>
      <a href="/downloads" class="nav-item{act_downloads}">Downloads</a>
      <a href="/docs" class="nav-item{act_docs}">Docs</a>
      <a href="/setup" class="nav-item{act_setup}">Worker Setup</a>
      <a href="https://github.com/ericflo/modelrelay" target="_blank" rel="noopener" class="nav-item">GitHub</a>
      <div class="nav-mobile-actions">
        {mobile_actions}
      </div>
    </div>
  </header>

  <main id="main-content">
    {main_html}
  </main>

  <footer class="site-footer" role="contentinfo">
    <div class="container">
      <div class="footer-grid">
        <div class="footer-brand">
          <a href="/" class="brand">
            <img src="/favicon-32.png" alt="" width="26" height="26" class="brand-icon">
            <span>Model<span class="brand-accent">Relay</span></span>
          </a>
          <p>Stop configuring clients for every GPU box. Workers connect out over WebSocket; requests route in through one stable endpoint.</p>
        </div>
        <div class="footer-col">
          <h4>Product</h4>
          <ul>
            <li><a href="/">Overview</a></li>
            <li><a href="/integrate">Pi &amp; API Setup</a></li>
            <li><a href="/downloads">Downloads</a></li>
            <li><a href="/setup">Worker Setup</a></li>
            <li><a href="/dashboard">Dashboard</a></li>
          </ul>
        </div>
        <div class="footer-col">
          <h4>Documentation</h4>
          <ul>
            <li><a href="/docs">Quickstart</a></li>
            <li><a href="/docs/api">api.modelrelay.io</a></li>
            <li><a href="/docs/pi">Pi Coding Agent</a></li>
            <li><a href="/docs/workers">Desktop &amp; Workers</a></li>
            <li><a href="/docs/self-hosting">Self-Hosting</a></li>
            <li><a href="/AGENTS.md">AGENTS.md</a></li>
          </ul>
        </div>
        <div class="footer-col">
          <h4>Project</h4>
          <ul>
            <li><a href="https://github.com/ericflo/modelrelay" target="_blank" rel="noopener">GitHub</a></li>
            <li><a href="https://github.com/ericflo/modelrelay/releases/latest" target="_blank" rel="noopener">Releases</a></li>
            <li><a href="/docs/architecture">Architecture</a></li>
            <li><a href="/terms">Terms</a></li>
            <li><a href="/privacy">Privacy</a></li>
          </ul>
        </div>
      </div>
      <div class="footer-bottom">
        <span>&copy; 2026 ModelRelay &middot; MIT Licensed Open Source</span>
        <span>Endpoint: <code>https://api.modelrelay.io</code></span>
      </div>
    </div>
  </footer>
  <script>{BASE_JS}</script>
  {extra_body_end}
</body>
</html>"##
    )
}

/// Standard contained page shell with custom body HTML, extra `<head>` markup
/// (typically `<style>`), and extra scripts before `</body>`.
#[must_use]
pub fn page_shell_custom(
    title: &str,
    path: &str,
    body_html: &str,
    logged_in: bool,
    extra_css: &str,
    extra_body_end: &str,
) -> String {
    let wrapped = format!(
        r#"<div class="content"><div class="container container--narrow">{body_html}</div></div>"#
    );
    page_shell_full(title, path, &wrapped, logged_in, extra_css, extra_body_end)
}

/// Convenience wrapper: auto-adds `<h1>` before body content inside a contained shell.
#[must_use]
pub fn page_shell(title: &str, path: &str, body_content: &str, logged_in: bool) -> String {
    let body_html = format!("<h1>{title}</h1>\n{body_content}");
    page_shell_custom(title, path, &body_html, logged_in, "", "")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn page_shell_includes_organization_jsonld() {
        let html = page_shell("Test", "/", "<p>body</p>", false);
        assert!(html.contains(r#"<script type="application/ld+json">"#));
        assert!(html.contains(r#""@type": "Organization""#));
        assert!(html.contains(r#""name": "ModelRelay""#));
        assert!(html.contains(r#""url": "https://modelrelay.io""#));
    }

    #[test]
    fn page_shell_uses_signup_specific_description() {
        let html = page_shell("Sign Up", "/signup", "<p>body</p>", false);
        assert!(html.contains("Create your ModelRelay account"));
    }

    #[test]
    fn page_shell_uses_login_specific_description() {
        let html = page_shell("Log In", "/login", "<p>body</p>", false);
        assert!(html.contains("Log in to ModelRelay"));
    }

    #[test]
    fn page_shell_uses_integrate_specific_description() {
        let html = page_shell("Integrate", "/integrate", "<p>body</p>", true);
        assert!(html.contains("Drop-in OpenAI"));
    }

    #[test]
    fn page_shell_uses_default_description_for_unknown_title() {
        let html = page_shell("Random Page", "/random", "<p>body</p>", false);
        assert!(html.contains("Run your local GPU workers"));
    }

    #[test]
    fn page_shell_description_appears_in_both_meta_and_og() {
        let html = page_shell("Sign Up", "/signup", "<p>body</p>", false);
        let signup_snippet = "Create your ModelRelay account";
        assert!(html.matches(signup_snippet).count() >= 2);
    }

    #[test]
    fn nav_is_consistent_across_logged_in_and_logged_out() {
        let out_html = page_shell_full("Home", "/", "<p>hi</p>", false, "", "");
        let in_html = page_shell_full("Dashboard", "/dashboard", "<p>hi</p>", true, "", "");
        for link in [
            r#"href="/""#,
            r#"href="/integrate""#,
            r#"href="/downloads""#,
            r#"href="/docs""#,
            r#"href="/setup""#,
        ] {
            assert!(out_html.contains(link), "logged-out nav missing {link}");
            assert!(in_html.contains(link), "logged-in nav missing {link}");
        }
        assert!(in_html.contains(r#"href="/dashboard""#));
        assert!(out_html.contains(r#"href="/login""#));
        assert!(
            !out_html.contains(r#"href="/signup""#),
            "nav must not link to closed /signup route"
        );
    }

    #[test]
    fn page_shell_supports_system_theme_and_manual_toggle() {
        let html = page_shell_full("Home", "/", "<p>hi</p>", false, "", "");
        assert!(html.contains("prefers-color-scheme: light"));
        assert!(html.contains("js-theme-toggle"));
        assert!(html.contains("mr_theme"));
    }
}
