/// Shared HTML page shell used by the admin dashboard and commercial cloud routes.
///
/// `logged_in` controls the nav links: when `true`, shows Dashboard + Pricing + Log out;
/// when `false`, shows Dashboard + Pricing only (login/signup are reached via their own pages).
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
