use super::{CloudWizardConfig, PI_EXTENSION_TS, cloud_config_script, page_shell_custom};

/// Build the Pi & API integration page (unauthenticated / self-hosted default).
#[must_use]
pub fn integrate_page() -> String {
    integrate_page_with_config(None)
}

/// Build the Pi & API integration page, optionally pre-configured for logged-in cloud users.
#[must_use]
#[allow(clippy::too_many_lines)]
pub fn integrate_page_with_config(cloud_config: Option<&CloudWizardConfig>) -> String {
    let integrate_css = r"
    .int-hero { margin-bottom: 28px; }
    .int-hero h1 { font-size: 2.1rem; font-weight: 800; letter-spacing: -0.03em; margin-bottom: 10px; }
    .int-hero p { color: var(--text-secondary); font-size: 1rem; max-width: 720px; }
    .int-jump {
      display: flex; gap: 10px; flex-wrap: wrap; margin-top: 18px;
    }

    .cloud-banner {
      display: none; align-items: center; justify-content: space-between; gap: 12px; flex-wrap: wrap;
      padding: 12px 16px; border-radius: var(--radius-md); margin-bottom: 18px;
      background: var(--success-soft); border: 1px solid rgba(5, 150, 105, 0.3);
      color: var(--success-fg); font-size: 0.88rem;
    }

    .config-card {
      background: var(--bg-surface); border: 1px solid var(--border);
      border-radius: var(--radius-lg); padding: 22px; margin-bottom: 36px;
      box-shadow: var(--shadow-card);
    }
    .config-grid {
      display: grid; grid-template-columns: 1.1fr 1.2fr 1fr; gap: 14px; align-items: end;
    }
    .cfg-field { display: flex; flex-direction: column; gap: 6px; }
    .cfg-field label {
      font-size: 0.72rem; font-weight: 700; text-transform: uppercase;
      letter-spacing: 0.08em; color: var(--text-muted);
    }
    .cfg-input-wrap { position: relative; display: flex; align-items: center; }
    .cfg-field input {
      width: 100%; padding: 10px 12px; background: var(--bg-input);
      border: 1px solid var(--border-strong); border-radius: 8px;
      color: var(--text-primary); font-family: var(--font-mono); font-size: 0.84rem;
      transition: border-color 0.15s, box-shadow 0.15s;
    }
    .cfg-field input:focus { border-color: var(--accent); box-shadow: 0 0 0 3px var(--accent-soft); }
    .cfg-input-wrap input { padding-right: 60px; }
    .cfg-toggle {
      position: absolute; right: 6px; padding: 4px 8px; border-radius: 5px;
      background: var(--bg-hover); border: 1px solid var(--border);
      color: var(--text-secondary); font-size: 0.72rem; font-weight: 600; cursor: pointer;
    }
    .cfg-toggle:hover { color: var(--text-strong); }

    .discover-row {
      display: flex; align-items: center; gap: 10px; flex-wrap: wrap;
      margin-top: 14px; padding-top: 14px; border-top: 1px solid var(--border);
    }
    .discover-chips { display: flex; gap: 6px; flex-wrap: wrap; align-items: center; }
    .model-chip {
      display: inline-flex; align-items: center; gap: 6px;
      padding: 4px 10px; border-radius: 999px; font-family: var(--font-mono); font-size: 0.76rem;
      background: var(--accent-soft); border: 1px solid var(--accent-border); color: var(--accent-fg);
      cursor: pointer; transition: background 0.15s, color 0.15s;
    }
    .model-chip:hover { background: var(--accent); color: #fff; }
    .model-chip-meta { opacity: 0.75; font-size: 0.7rem; }
    .discover-status { font-size: 0.82rem; color: var(--text-muted); }

    /* Sections */
    .int-section { margin-bottom: 52px; scroll-margin-top: 84px; }
    .sec-head {
      display: flex; align-items: baseline; justify-content: space-between; gap: 12px; flex-wrap: wrap;
      padding-bottom: 14px; margin-bottom: 20px; border-bottom: 1px solid var(--border);
    }
    .sec-head h2 { font-size: 1.45rem; font-weight: 700; letter-spacing: -0.02em; color: var(--text-strong); }
    .sec-eyebrow {
      font-size: 0.72rem; font-weight: 700; text-transform: uppercase; letter-spacing: 0.09em;
      color: var(--accent-bright); margin-bottom: 4px;
    }

    /* Endpoint Reference Grid */
    .ep-grid {
      display: grid; grid-template-columns: repeat(2, 1fr); gap: 12px; margin-bottom: 24px;
    }
    .ep-card {
      background: var(--bg-surface); border: 1px solid var(--border);
      border-radius: var(--radius-md); padding: 16px 18px;
    }
    .ep-line { display: flex; align-items: center; gap: 8px; margin-bottom: 6px; flex-wrap: wrap; }
    .http-verb {
      font-family: var(--font-mono); font-size: 0.72rem; font-weight: 700;
      padding: 2px 7px; border-radius: 5px;
    }
    .verb-get { background: var(--success-soft); color: var(--success-fg); border: 1px solid rgba(5, 150, 105, 0.3); }
    .verb-post { background: var(--cyan-soft); color: var(--cyan-fg); border: 1px solid rgba(2, 132, 199, 0.3); }
    .ep-path { font-family: var(--font-mono); font-size: 0.86rem; color: var(--text-strong); font-weight: 600; }
    .ep-card p { font-size: 0.85rem; color: var(--text-secondary); margin: 0; }

    /* Tabs */
    .tab-group {
      background: var(--bg-surface); border: 1px solid var(--border);
      border-radius: var(--radius-lg); overflow: hidden; margin-bottom: 20px;
    }
    .tab-bar {
      display: flex; gap: 2px; overflow-x: auto; padding: 6px 8px 0;
      background: var(--bg-subtle); border-bottom: 1px solid var(--border);
    }
    .tab-btn {
      padding: 9px 14px; background: transparent; border: none;
      border-bottom: 2px solid transparent; color: var(--text-muted);
      font-size: 0.84rem; font-weight: 600; font-family: inherit;
      cursor: pointer; white-space: nowrap; transition: color 0.15s, border-color 0.15s;
    }
    .tab-btn:hover { color: var(--text-strong); }
    .tab-btn.active { color: var(--text-strong); border-bottom-color: var(--accent-bright); }
    .tab-panel { display: none; padding: 20px; }
    .tab-panel.active { display: block; }
    .tab-note { font-size: 0.86rem; color: var(--text-secondary); margin-bottom: 10px; }

    /* Pi Step Cards */
    .pi-steps { display: flex; flex-direction: column; gap: 18px; }
    .pi-step {
      background: var(--bg-surface); border: 1px solid var(--border);
      border-radius: var(--radius-lg); padding: 24px;
    }
    .pi-step-head { display: flex; align-items: center; gap: 12px; margin-bottom: 10px; }
    .pi-num {
      width: 28px; height: 28px; border-radius: 8px;
      background: var(--accent-soft); border: 1px solid var(--accent-border);
      color: var(--accent-fg); font-size: 0.82rem; font-weight: 700;
      display: inline-flex; align-items: center; justify-content: center; flex-shrink: 0;
    }
    .pi-step h3 { font-size: 1.05rem; font-weight: 700; color: var(--text-strong); }
    .pi-step p { font-size: 0.9rem; color: var(--text-secondary); margin-bottom: 12px; }
    .pi-why {
      display: grid; grid-template-columns: repeat(3, 1fr); gap: 12px; margin-bottom: 22px;
    }
    .pi-why-item {
      background: var(--bg-surface); border: 1px solid var(--border);
      border-radius: var(--radius-md); padding: 16px;
    }
    .pi-why-item strong { display: block; font-size: 0.9rem; color: var(--text-strong); margin-bottom: 4px; }
    .pi-why-item span { font-size: 0.84rem; color: var(--text-secondary); }

    /* Collapsible callout */
    details.opt-box {
      background: var(--bg-surface); border: 1px solid var(--border);
      border-radius: var(--radius-lg); padding: 18px 22px; margin-top: 18px;
    }
    details.opt-box summary {
      cursor: pointer; font-weight: 600; font-size: 0.94rem; color: var(--text-strong);
      list-style: none; display: flex; align-items: center; justify-content: space-between;
    }
    details.opt-box summary::-webkit-details-marker { display: none; }
    details.opt-box summary::after { content: '+'; color: var(--accent-bright); font-size: 1.2rem; font-weight: 400; }
    details.opt-box[open] summary::after { content: '−'; }
    details.opt-box .opt-body { margin-top: 14px; }
    details.opt-box p { font-size: 0.88rem; color: var(--text-secondary); margin-bottom: 10px; }

    /* Live Playground */
    .playground {
      background: var(--bg-surface); border: 1px solid var(--border);
      border-radius: var(--radius-lg); padding: 24px; position: relative; overflow: hidden;
    }
    .playground::before {
      content: ''; position: absolute; top: 0; left: 0; right: 0; height: 2px;
      background: linear-gradient(90deg, var(--accent), var(--cyan));
    }
    .pg-controls { display: flex; gap: 12px; align-items: center; flex-wrap: wrap; margin-bottom: 14px; }
    .pg-controls select {
      padding: 8px 12px; background: var(--bg-input); border: 1px solid var(--border-strong);
      border-radius: 8px; color: var(--text-primary); font-size: 0.85rem; font-family: inherit;
    }
    .pg-toggle {
      display: inline-flex; align-items: center; gap: 6px; font-size: 0.84rem;
      color: var(--text-secondary); cursor: pointer;
    }
    .pg-toggle input { accent-color: var(--accent); }
    .pg-input-row { display: flex; gap: 10px; margin-bottom: 14px; }
    .pg-input-row input {
      flex: 1; padding: 11px 14px; background: var(--bg-input);
      border: 1px solid var(--border-strong); border-radius: 9px;
      color: var(--text-primary); font-size: 0.92rem; font-family: inherit;
    }
    .pg-input-row input:focus { border-color: var(--accent); box-shadow: 0 0 0 3px var(--accent-soft); }
    .pg-output {
      background: var(--bg-code); border: 1px solid var(--code-border); border-radius: var(--radius-md);
      padding: 16px; min-height: 120px; max-height: 360px; overflow-y: auto;
      font-family: var(--font-mono); font-size: 0.85rem; line-height: 1.65;
      color: var(--code-fg); white-space: pre-wrap; word-break: break-word;
    }
    .pg-output.streaming { border-color: var(--accent-border); }
    .pg-placeholder { color: var(--code-muted); font-style: italic; }
    .pg-error { color: #f87171; }
    .pg-meta {
      display: flex; justify-content: space-between; align-items: center; flex-wrap: wrap; gap: 8px;
      margin-top: 10px; font-size: 0.78rem; color: var(--text-muted); font-family: var(--font-mono);
    }

    @media (max-width: 820px) {
      .config-grid, .ep-grid, .pi-why { grid-template-columns: 1fr; }
      .pg-input-row { flex-direction: column; }
    }
    ";

    let integrate_js = r#"
(function() {
  const $ = s => document.querySelector(s);
  const $$ = s => document.querySelectorAll(s);
  const cloudCfg = window.__mrCloudConfig || null;

  const urlInput = $('#int-server-url');
  const keyInput = $('#int-api-key');
  const modelInput = $('#int-model-name');

  const defaultUrl = (cloudCfg && cloudCfg.serverUrl)
    ? cloudCfg.serverUrl
    : (window.location.hostname === 'modelrelay.io' ? 'https://api.modelrelay.io' : (localStorage.getItem('mr_server_url') || 'https://api.modelrelay.io'));

  urlInput.value = defaultUrl;
  keyInput.value = (cloudCfg && cloudCfg.apiKey)
    ? cloudCfg.apiKey
    : (localStorage.getItem('mr_test_api_key') || '');
  modelInput.value = localStorage.getItem('mr_model_name') || '';

  if (cloudCfg && cloudCfg.apiKey) {
    const banner = $('#int-cloud-banner');
    if (banner) banner.style.display = 'flex';
  }

  const keyToggle = $('#toggle-api-key');
  if (keyToggle) {
    keyToggle.addEventListener('click', () => {
      const hidden = keyInput.type === 'password';
      keyInput.type = hidden ? 'text' : 'password';
      keyToggle.textContent = hidden ? 'Hide' : 'Show';
    });
  }

  function sv() { return urlInput.value.trim().replace(/\/+$/, '') || 'https://api.modelrelay.io'; }
  function ak() { return keyInput.value.trim() || 'mr_live_your_api_key'; }
  function mn() { return modelInput.value.trim() || 'qwen3.5-27b'; }

  function renderAllSnippets() {
    const s = sv(), a = ak(), m = mn();
    $$('[data-tpl]').forEach(el => {
      const raw = el.getAttribute('data-tpl');
      el.textContent = raw
        .replace(/__SERVER_URL__/g, s)
        .replace(/__API_KEY__/g, a)
        .replace(/__MODEL_NAME__/g, m);
    });
  }

  // Tab switching
  $$('.tab-group').forEach(group => {
    const btns = group.querySelectorAll('.tab-btn');
    const panels = group.querySelectorAll('.tab-panel');
    btns.forEach(btn => {
      btn.addEventListener('click', () => {
        btns.forEach(b => b.classList.remove('active'));
        panels.forEach(p => p.classList.remove('active'));
        btn.classList.add('active');
        const target = group.querySelector('.tab-panel[data-tab="' + btn.dataset.tab + '"]');
        if (target) target.classList.add('active');
      });
    });
  });

  // Copy for code blocks inside integrate
  document.addEventListener('click', e => {
    const btn = e.target.closest('.copy-code-btn');
    if (!btn) return;
    const container = btn.closest('.code-shell') || btn.closest('.code-block');
    const target = container ? container.querySelector('.code-text') : null;
    if (!target) return;
    navigator.clipboard.writeText(target.textContent.trim()).then(() => {
      const prev = btn.textContent;
      btn.textContent = 'Copied';
      btn.classList.add('copied');
      setTimeout(() => { btn.textContent = prev; btn.classList.remove('copied'); }, 1500);
    });
  });

  urlInput.addEventListener('input', () => { localStorage.setItem('mr_server_url', urlInput.value.trim()); renderAllSnippets(); });
  keyInput.addEventListener('input', () => { localStorage.setItem('mr_test_api_key', keyInput.value.trim()); renderAllSnippets(); });
  modelInput.addEventListener('input', () => { localStorage.setItem('mr_model_name', modelInput.value.trim()); renderAllSnippets(); });
  renderAllSnippets();

  function formatModelMeta(entry) {
    const parts = [];
    const ctx = entry.meta?.n_ctx || entry.context_window || entry.context_length || entry.max_model_len || entry.max_context_length || entry.n_ctx || entry.contextWindow;
    if (typeof ctx === 'number' && ctx > 0) {
      parts.push(ctx >= 1024 ? Math.round(ctx / 1024) + 'k' : String(ctx));
    }
    const efforts = entry.reasoning_efforts || entry.supported_reasoning_efforts || entry.capabilities?.reasoning_efforts || entry.thinking_levels;
    if (Array.isArray(efforts) && efforts.length > 0) {
      parts.push(efforts.join('/'));
    } else if (entry.reasoning === true || entry.capabilities?.reasoning === true) {
      parts.push('reasoning');
    }
    return parts.join(' · ');
  }

  // Discover live models via GET /v1/models
  const discoverBtn = $('#discover-models-btn');
  const discoverChips = $('#discover-chips');
  const discoverStatus = $('#discover-status');
  if (discoverBtn) {
    discoverBtn.addEventListener('click', async () => {
      const url = sv();
      const key = keyInput.value.trim();
      discoverBtn.disabled = true;
      discoverStatus.textContent = 'Querying ' + url + '/v1/models…';
      discoverChips.textContent = '';
      try {
        const headers = {};
        if (key) headers['Authorization'] = 'Bearer ' + key;
        const res = await fetch(url + '/v1/models', { headers });
        if (!res.ok) {
          discoverStatus.textContent = 'HTTP ' + res.status + (res.status === 401 ? ' — check your API key.' : ' — could not fetch models.');
          return;
        }
        const payload = await res.json();
        const entries = (payload.data || []).filter(x => x && x.id);
        if (entries.length === 0) {
          discoverStatus.textContent = 'Connected, but no workers are advertising models right now.';
          return;
        }
        discoverStatus.textContent = entries.length + ' live model' + (entries.length === 1 ? '' : 's') + ' (click to select):';
        entries.forEach(entry => {
          const chip = document.createElement('button');
          chip.type = 'button';
          chip.className = 'model-chip';
          chip.appendChild(document.createTextNode(entry.id));
          const meta = formatModelMeta(entry);
          if (meta) {
            const tag = document.createElement('span');
            tag.className = 'model-chip-meta';
            tag.textContent = '· ' + meta;
            chip.appendChild(tag);
          }
          chip.addEventListener('click', () => {
            modelInput.value = entry.id;
            localStorage.setItem('mr_model_name', entry.id);
            renderAllSnippets();
          });
          discoverChips.appendChild(chip);
        });
        if (!modelInput.value.trim() && entries[0]) {
          modelInput.value = entries[0].id;
          localStorage.setItem('mr_model_name', entries[0].id);
          renderAllSnippets();
        }
      } catch (err) {
        discoverStatus.textContent = 'Request failed: ' + err.message;
      } finally {
        discoverBtn.disabled = false;
      }
    });
  }

  // Live Playground
  const pgPrompt = $('#pg-prompt');
  const pgSend = $('#pg-send');
  const pgStop = $('#pg-stop');
  const pgOutput = $('#pg-output');
  const pgStatus = $('#pg-status');
  const pgStream = $('#pg-stream');
  const pgFormat = $('#pg-format');
  let abortCtrl = null;

  function buildReq(format, url, key, model, prompt, streaming) {
    if (format === 'messages') {
      return {
        endpoint: url + '/v1/messages',
        headers: { 'Content-Type': 'application/json', 'x-api-key': key, 'anthropic-version': '2023-06-01' },
        body: { model, max_tokens: 512, messages: [{ role: 'user', content: prompt }], stream: streaming },
      };
    }
    if (format === 'responses') {
      return {
        endpoint: url + '/v1/responses',
        headers: { 'Content-Type': 'application/json', 'Authorization': 'Bearer ' + key },
        body: { model, input: prompt, stream: streaming },
      };
    }
    return {
      endpoint: url + '/v1/chat/completions',
      headers: { 'Content-Type': 'application/json', 'Authorization': 'Bearer ' + key },
      body: { model, messages: [{ role: 'user', content: prompt }], stream: streaming },
    };
  }

  function extractDelta(format, line) {
    if (!line.startsWith('data: ')) return null;
    const raw = line.slice(6).trim();
    if (!raw || raw === '[DONE]') return null;
    try {
      const ev = JSON.parse(raw);
      if (format === 'messages') return ev.type === 'content_block_delta' ? (ev.delta?.text || null) : null;
      if (format === 'responses') return ev.type === 'response.output_text.delta' ? (ev.delta || null) : null;
      return ev.choices?.[0]?.delta?.content || null;
    } catch (_) { return null; }
  }

  async function runPlayground() {
    const url = sv();
    const key = keyInput.value.trim();
    const model = modelInput.value.trim();
    const prompt = pgPrompt.value.trim();
    if (!key) { pgOutput.innerHTML = '<span class="pg-placeholder">Enter your API key above to send a live request.</span>'; return; }
    if (!model) { pgOutput.innerHTML = '<span class="pg-placeholder">Enter a model name (or click "Fetch live models") above first.</span>'; return; }
    if (!prompt) return;

    const format = pgFormat.value;
    const streaming = pgStream.checked;
    const req = buildReq(format, url, key, model, prompt, streaming);

    abortCtrl = new AbortController();
    pgSend.style.display = 'none';
    pgStop.style.display = '';
    pgOutput.textContent = streaming ? 'Connecting…' : 'Waiting for response…';
    pgOutput.classList.toggle('streaming', streaming);
    pgStatus.textContent = '';
    const t0 = performance.now();

    try {
      const res = await fetch(req.endpoint, {
        method: 'POST', headers: req.headers, body: JSON.stringify(req.body), signal: abortCtrl.signal,
      });
      if (!res.ok) {
        const txt = await res.text().catch(() => '');
        pgOutput.innerHTML = '<span class="pg-error">HTTP ' + res.status + '</span>\n' + txt.slice(0, 400);
        pgStatus.textContent = 'Error · ' + Math.round(performance.now() - t0) + 'ms';
        return;
      }
      if (!streaming) {
        const data = await res.json();
        const text = format === 'messages'
          ? (data.content?.[0]?.text || JSON.stringify(data, null, 2))
          : format === 'responses'
          ? (data.output?.[0]?.content?.[0]?.text || data.output_text || JSON.stringify(data, null, 2))
          : (data.choices?.[0]?.message?.content || JSON.stringify(data, null, 2));
        pgOutput.textContent = text;
        pgStatus.textContent = '200 OK · ' + Math.round(performance.now() - t0) + 'ms';
        return;
      }
      pgOutput.textContent = '';
      const reader = res.body.getReader();
      const dec = new TextDecoder();
      let buf = '', chunks = 0;
      while (true) {
        const { done, value } = await reader.read();
        if (done) break;
        buf += dec.decode(value, { stream: true });
        const lines = buf.split('\n');
        buf = lines.pop() || '';
        for (const line of lines) {
          const delta = extractDelta(format, line);
          if (delta) {
            chunks++;
            pgOutput.appendChild(document.createTextNode(delta));
            pgOutput.scrollTop = pgOutput.scrollHeight;
          }
        }
        pgStatus.textContent = chunks + ' chunks · ' + Math.round(performance.now() - t0) + 'ms';
      }
      pgStatus.textContent = 'Done · ' + chunks + ' chunks · ' + Math.round(performance.now() - t0) + 'ms';
    } catch (err) {
      if (err.name === 'AbortError') {
        pgStatus.textContent = 'Cancelled · ' + Math.round(performance.now() - t0) + 'ms';
      } else {
        pgOutput.innerHTML = '<span class="pg-error">' + err.message + '</span>';
        pgStatus.textContent = 'Request failed';
      }
    } finally {
      pgOutput.classList.remove('streaming');
      pgSend.style.display = '';
      pgStop.style.display = 'none';
      abortCtrl = null;
    }
  }

  pgSend.addEventListener('click', runPlayground);
  pgPrompt.addEventListener('keydown', e => { if (e.key === 'Enter') runPlayground(); });
  pgStop.addEventListener('click', () => { if (abortCtrl) abortCtrl.abort(); });
})();
    "#;

    let curl_models =
        "curl __SERVER_URL__/v1/models \\\n  -H \"Authorization: Bearer __API_KEY__\"";

    let curl_chat = "curl __SERVER_URL__/v1/chat/completions \\\n  -H \"Content-Type: application/json\" \\\n  -H \"Authorization: Bearer __API_KEY__\" \\\n  -d '{\n    \"model\": \"__MODEL_NAME__\",\n    \"messages\": [{\"role\": \"user\", \"content\": \"Explain WebSockets in one sentence.\"}]\n  }'";

    let curl_stream = "curl -N __SERVER_URL__/v1/chat/completions \\\n  -H \"Content-Type: application/json\" \\\n  -H \"Authorization: Bearer __API_KEY__\" \\\n  -d '{\n    \"model\": \"__MODEL_NAME__\",\n    \"stream\": true,\n    \"messages\": [{\"role\": \"user\", \"content\": \"Write a haiku about local GPUs.\"}]\n  }'";

    let curl_responses = "curl __SERVER_URL__/v1/responses \\\n  -H \"Content-Type: application/json\" \\\n  -H \"Authorization: Bearer __API_KEY__\" \\\n  -d '{\n    \"model\": \"__MODEL_NAME__\",\n    \"input\": \"List three benefits of running models locally.\"\n  }'";

    let curl_messages = "curl __SERVER_URL__/v1/messages \\\n  -H \"Content-Type: application/json\" \\\n  -H \"x-api-key: __API_KEY__\" \\\n  -H \"anthropic-version: 2023-06-01\" \\\n  -d '{\n    \"model\": \"__MODEL_NAME__\",\n    \"max_tokens\": 256,\n    \"messages\": [{\"role\": \"user\", \"content\": \"Hello from Anthropic Messages!\"}]\n  }'";

    let pi_extension_tpl =
        PI_EXTENSION_TS.replace("https://api.modelrelay.io/v1", "__SERVER_URL__/v1");

    let pi_extension_curl = "curl -fsSL https://modelrelay.io/pi/modelrelay.ts \\\n  --create-dirs -o ~/.pi/agent/extensions/modelrelay.ts";

    let pi_auth_shell = "mkdir -p ~/.pi/agent && node -e '\nconst fs = require(\"node:fs\"), p = process.env.HOME + \"/.pi/agent/auth.json\";\nconst cur = fs.existsSync(p) ? JSON.parse(fs.readFileSync(p, \"utf8\")) : {};\ncur.modelrelay = { type: \"api_key\", key: \"__API_KEY__\" };\nfs.writeFileSync(p, JSON.stringify(cur, null, 2) + \"\\n\", { mode: 0o600 });\nfs.chmodSync(p, 0o600);\nconsole.log(\"Saved ModelRelay key to \" + p);\n'";

    let pi_auth_json =
        "{\n  \"modelrelay\": {\n    \"type\": \"api_key\",\n    \"key\": \"__API_KEY__\"\n  }\n}";

    let pi_auth_keychain = "{\n  \"modelrelay\": {\n    \"type\": \"api_key\",\n    \"key\": \"!security find-generic-password -ws 'modelrelay'\"\n  }\n}";

    let codex_setup_shell = "# 1. Save key privately (chmod 600 — never paste into an LLM)\nmkdir -p ~/.config/modelrelay ~/.codex\n(umask 077 && printf '%s\\n' '__API_KEY__' > ~/.config/modelrelay/api_key)\n\n# 2. Add modelrelay provider alongside commercial OpenAI\ngrep -q '\\[model_providers\\.modelrelay\\]' ~/.codex/config.toml 2>/dev/null || cat >> ~/.codex/config.toml <<'EOF'\n\n[model_providers.modelrelay]\nname = \"ModelRelay\"\nbase_url = \"__SERVER_URL__/v1\"\nenv_key = \"MODELRELAY_API_KEY\"\nwire_api = \"chat\"\nEOF\n\n# 3. Run Codex against ModelRelay on demand (plain `codex` stays on OpenAI)\nMODELRELAY_API_KEY=\"$(cat ~/.config/modelrelay/api_key)\" \\\n  codex -c model_provider=modelrelay -m __MODEL_NAME__";

    let claude_setup_shell = "# 1. Save key privately (chmod 600 — never paste into an LLM)\nmkdir -p ~/.config/modelrelay\n(umask 077 && printf '%s\\n' '__API_KEY__' > ~/.config/modelrelay/api_key)\n\n# 2. Add a separate `claude-local` helper (plain `claude` stays on Anthropic)\ncat >> ~/.zshrc <<'EOF'\n\nclaude-local() {\n  ANTHROPIC_BASE_URL=\"__SERVER_URL__\" \\\n  ANTHROPIC_AUTH_TOKEN=\"$(tr -d '\\r\\n' < ~/.config/modelrelay/api_key)\" \\\n  ANTHROPIC_MODEL=\"${1:-__MODEL_NAME__}\" \\\n  claude \"${@:2}\"\n}\nEOF\nsource ~/.zshrc && claude-local __MODEL_NAME__";

    let pi_static_models_json = "{\n  \"$schema\": \"https://pi.dev/schemas/models.schema.json\",\n  \"providers\": {\n    \"modelrelay\": {\n      \"baseUrl\": \"__SERVER_URL__/v1\",\n      \"api\": \"openai-completions\",\n      \"apiKey\": \"$MODELRELAY_API_KEY\",\n      \"models\": [\n        {\n          \"id\": \"__MODEL_NAME__\",\n          \"reasoning\": true,\n          \"contextWindow\": 131072,\n          \"maxTokens\": 32768\n        }\n      ]\n    }\n  }\n}";

    let esc_attr = |s: &str| {
        s.replace('&', "&amp;")
            .replace('"', "&quot;")
            .replace('\'', "&#x27;")
            .replace('<', "&lt;")
            .replace('>', "&gt;")
    };

    let t_models = esc_attr(curl_models);
    let t_chat = esc_attr(curl_chat);
    let t_stream = esc_attr(curl_stream);
    let t_responses = esc_attr(curl_responses);
    let t_messages = esc_attr(curl_messages);
    let t_pi_ts = esc_attr(&pi_extension_tpl);
    let ssr_pi_ts = esc_attr(PI_EXTENSION_TS);
    let t_pi_auth_shell = esc_attr(pi_auth_shell);
    let t_pi_auth = esc_attr(pi_auth_json);
    let t_pi_keychain = esc_attr(pi_auth_keychain);
    let t_codex_setup = esc_attr(codex_setup_shell);
    let t_claude_setup = esc_attr(claude_setup_shell);
    let t_pi_static = esc_attr(pi_static_models_json);
    let ssr_pi_static = esc_attr(
        &pi_static_models_json
            .replace("__SERVER_URL__", "https://api.modelrelay.io")
            .replace("__MODEL_NAME__", "qwen3.5-27b"),
    );

    let logged_in = cloud_config.is_some();
    let extra_css = format!("<style>{integrate_css}</style>");

    let body = format!(
        r##"<div class="int-hero">
      <div class="sec-eyebrow">Connect Clients</div>
      <h1>Pi Coding Agent &amp; <code>api.modelrelay.io</code></h1>
      <p>Every GPU worker you connect appears behind one authenticated HTTPS endpoint. ModelRelay passes through each model&rsquo;s live metadata from <code>/v1/models</code> so Pi picks up context limits and reasoning efforts automatically.</p>
      <div class="int-jump">
        <a href="#pi" class="btn">Jump to Pi Setup &darr;</a>
        <a href="#endpoint" class="btn btn-secondary">api.modelrelay.io &amp; curl</a>
        <a href="#playground" class="btn btn-secondary">Try It Live</a>
      </div>
    </div>

    <div id="int-cloud-banner" class="cloud-banner">
      <span>Logged in &mdash; your relay endpoint and active API key are pre-filled in every snippet below.</span>
      <a href="/dashboard" style="color:var(--success-fg);font-weight:600;">Manage keys &rarr;</a>
    </div>

    <div class="config-card">
      <div class="config-grid">
        <div class="cfg-field">
          <label for="int-server-url">Relay Endpoint</label>
          <input id="int-server-url" type="text" placeholder="https://api.modelrelay.io">
        </div>
        <div class="cfg-field">
          <label for="int-api-key">API Key</label>
          <div class="cfg-input-wrap">
            <input id="int-api-key" type="password" placeholder="mr_live_...">
            <button type="button" class="cfg-toggle" id="toggle-api-key">Show</button>
          </div>
        </div>
        <div class="cfg-field">
          <label for="int-model-name">Model ID</label>
          <input id="int-model-name" type="text" placeholder="qwen3.5-27b">
        </div>
      </div>
      <div class="discover-row">
        <button type="button" class="btn btn-sm btn-secondary" id="discover-models-btn">Fetch live models</button>
        <span class="discover-status" id="discover-status">Queries <code>GET /v1/models</code> on your relay to inspect live models, context windows, and reasoning efforts.</span>
        <div class="discover-chips" id="discover-chips"></div>
      </div>
    </div>

    <!-- ═══ 1. api.modelrelay.io Endpoint & curl ═══ -->
    <section class="int-section" id="endpoint">
      <div class="sec-head">
        <div>
          <div class="sec-eyebrow">Part 1 &mdash; HTTP API</div>
          <h2>The <code>api.modelrelay.io</code> Endpoint</h2>
        </div>
        <span class="badge badge-cyan">TLS &middot; SSE Streaming &middot; End-to-End Cancel</span>
      </div>
      <p style="color:var(--text-secondary);margin-bottom:18px;max-width:760px;">
        <code>https://api.modelrelay.io</code> is a single OpenAI- and Anthropic-compatible gateway in front of your connected workers. Send your API key via <code>Authorization: Bearer &lt;key&gt;</code> (or <code>x-api-key</code> for Anthropic Messages). Requests queue automatically if workers are busy, stream SSE chunks without buffering, and cancel on the backend if the client disconnects.
      </p>

      <div class="ep-grid">
        <div class="ep-card">
          <div class="ep-line"><span class="http-verb verb-get">GET</span><span class="ep-path">/v1/models</span></div>
          <p>Returns live models across your workers, preserving backend metadata (context length, max tokens, reasoning efforts, and modalities).</p>
        </div>
        <div class="ep-card">
          <div class="ep-line"><span class="http-verb verb-post">POST</span><span class="ep-path">/v1/chat/completions</span></div>
          <p>Standard OpenAI Chat Completions (streaming and non-streaming). Works with llama.cpp, Ollama, vLLM, and LM Studio.</p>
        </div>
        <div class="ep-card">
          <div class="ep-line"><span class="http-verb verb-post">POST</span><span class="ep-path">/v1/responses</span></div>
          <p>OpenAI Responses API format with streaming event pass-through.</p>
        </div>
        <div class="ep-card">
          <div class="ep-line"><span class="http-verb verb-post">POST</span><span class="ep-path">/v1/messages</span></div>
          <p>Anthropic Messages API format using <code>x-api-key</code> and <code>anthropic-version</code> headers.</p>
        </div>
      </div>

      <div class="tab-group">
        <div class="tab-bar">
          <button type="button" class="tab-btn active" data-tab="c-models">List Models</button>
          <button type="button" class="tab-btn" data-tab="c-chat">Chat Completions</button>
          <button type="button" class="tab-btn" data-tab="c-stream">Streaming SSE</button>
          <button type="button" class="tab-btn" data-tab="c-responses">Responses API</button>
          <button type="button" class="tab-btn" data-tab="c-messages">Anthropic Messages</button>
        </div>
        <div class="tab-panel active" data-tab="c-models">
          <p class="tab-note">Verify your token and inspect live models plus metadata relayed from your workers:</p>
          <div class="code-shell">
            <div class="code-shell-header"><span>curl &mdash; GET /v1/models</span><button type="button" class="copy-btn copy-code-btn">Copy</button></div>
            <pre><code class="code-text" data-tpl="{t_models}"></code></pre>
          </div>
        </div>
        <div class="tab-panel" data-tab="c-chat">
          <p class="tab-note">Send a standard OpenAI Chat Completions request routed by the <code>model</code> field:</p>
          <div class="code-shell">
            <div class="code-shell-header"><span>curl &mdash; POST /v1/chat/completions</span><button type="button" class="copy-btn copy-code-btn">Copy</button></div>
            <pre><code class="code-text" data-tpl="{t_chat}"></code></pre>
          </div>
        </div>
        <div class="tab-panel" data-tab="c-stream">
          <p class="tab-note">Pass <code>-N</code> to disable curl buffering and watch SSE deltas stream straight from your GPU. Press <code>Ctrl+C</code> mid-stream to cancel generation on the worker:</p>
          <div class="code-shell">
            <div class="code-shell-header"><span>curl &mdash; Streaming SSE</span><button type="button" class="copy-btn copy-code-btn">Copy</button></div>
            <pre><code class="code-text" data-tpl="{t_stream}"></code></pre>
          </div>
        </div>
        <div class="tab-panel" data-tab="c-responses">
          <p class="tab-note">Route OpenAI Responses API calls through the same endpoint and token:</p>
          <div class="code-shell">
            <div class="code-shell-header"><span>curl &mdash; POST /v1/responses</span><button type="button" class="copy-btn copy-code-btn">Copy</button></div>
            <pre><code class="code-text" data-tpl="{t_responses}"></code></pre>
          </div>
        </div>
        <div class="tab-panel" data-tab="c-messages">
          <p class="tab-note">Use the Anthropic Messages endpoint when your backend or client speaks Anthropic format:</p>
          <div class="code-shell">
            <div class="code-shell-header"><span>curl &mdash; POST /v1/messages</span><button type="button" class="copy-btn copy-code-btn">Copy</button></div>
            <pre><code class="code-text" data-tpl="{t_messages}"></code></pre>
          </div>
        </div>
      </div>
    </section>

    <!-- ═══ 2. Pi Coding Agent Setup ═══ -->
    <section class="int-section" id="pi">
      <div class="sec-head">
        <div>
          <div class="sec-eyebrow">Part 2 &mdash; Recommended Client</div>
          <h2>Pi Coding Agent Setup</h2>
        </div>
        <span class="badge badge-accent">Zero-Config Dynamic Discovery</span>
      </div>

      <p style="color:var(--text-secondary);margin-bottom:18px;max-width:760px;">
        Install one small provider extension and put your API key in <code>~/.pi/agent/auth.json</code>. ModelRelay passes through your server&rsquo;s <code>/v1/models</code> metadata, so Pi picks up context limits, max output tokens, reasoning efforts, and vision support dynamically &mdash; no manual tuning or static model lists.
      </p>

      <div class="pi-why">
        <div class="pi-why-item">
          <strong>Auto-discovered metadata</strong>
          <span>Reads context window, max output tokens, reasoning efforts, and vision modalities straight from <code>GET /v1/models</code>.</span>
        </div>
        <div class="pi-why-item">
          <strong>Dynamic <code>/thinking</code> levels</strong>
          <span>Maps your server&rsquo;s advertised <code>reasoning_efforts</code> (or <code>thinkingLevelMap</code>) directly into Pi&rsquo;s <code>/thinking</code> picker.</span>
        </div>
        <div class="pi-why-item">
          <strong>Clean credentials in <code>auth.json</code></strong>
          <span>No API keys in config files. Supports <code>~/.pi/agent/auth.json</code>, <code>/login modelrelay</code>, or keychain <code>!command</code> lookups.</span>
        </div>
      </div>

      <div class="pi-steps">
        <!-- Step 1 -->
        <div class="pi-step">
          <div class="pi-step-head">
            <span class="pi-num">1</span>
            <h3>Install Pi</h3>
          </div>
          <p>Install the Pi coding agent if you don&rsquo;t have it yet (see <a href="https://pi.dev" target="_blank" rel="noopener">pi.dev</a>):</p>
          <div class="tab-group" style="margin-bottom:0;">
            <div class="tab-bar">
              <button type="button" class="tab-btn active" data-tab="pi-sh">macOS / Linux</button>
              <button type="button" class="tab-btn" data-tab="pi-ps">Windows (PowerShell)</button>
              <button type="button" class="tab-btn" data-tab="pi-npm">npm</button>
            </div>
            <div class="tab-panel active" data-tab="pi-sh" style="padding:14px;">
              <div class="code-shell" style="margin:0;">
                <div class="code-shell-header"><span>shell</span><button type="button" class="copy-btn copy-code-btn">Copy</button></div>
                <pre><code class="code-text">curl -fsSL https://pi.dev/install.sh | sh</code></pre>
              </div>
            </div>
            <div class="tab-panel" data-tab="pi-ps" style="padding:14px;">
              <div class="code-shell" style="margin:0;">
                <div class="code-shell-header"><span>powershell</span><button type="button" class="copy-btn copy-code-btn">Copy</button></div>
                <pre><code class="code-text">powershell -c "irm https://pi.dev/install.ps1 | iex"</code></pre>
              </div>
            </div>
            <div class="tab-panel" data-tab="pi-npm" style="padding:14px;">
              <div class="code-shell" style="margin:0;">
                <div class="code-shell-header"><span>npm (Node 22.19+)</span><button type="button" class="copy-btn copy-code-btn">Copy</button></div>
                <pre><code class="code-text">npm install -g --ignore-scripts @earendil-works/pi-coding-agent</code></pre>
              </div>
            </div>
          </div>
        </div>

        <!-- Step 2 -->
        <div class="pi-step">
          <div class="pi-step-head">
            <span class="pi-num">2</span>
            <h3>Enable Live Model Discovery (1 command)</h3>
          </div>
          <p>Zero <code>models.json</code> config required &mdash; one command installs our hosted provider file so Pi pulls live models, runtime context windows, reasoning efforts, and vision support dynamically from your server APIs, out of the box for <strong>llama.cpp</strong> (<code>/props</code> &amp; router mode), <strong>vLLM</strong> (<code>max_model_len</code>), <strong>LM Studio</strong> (<code>/api/v1/models</code> &amp; <code>/api/v0/models</code>), <strong>Ollama</strong> (<code>/api/show</code>), and custom OpenAI-compatible servers (self-hosting? Export <code>MODELRELAY_BASE_URL</code>):</p>

          <div class="tab-group" style="margin-bottom:0;">
            <div class="tab-bar">
              <button type="button" class="tab-btn active" data-tab="pi-ext-cmd">One-line install</button>
              <button type="button" class="tab-btn" data-tab="pi-ext-file">Inspect modelrelay.ts</button>
            </div>
            <div class="tab-panel active" data-tab="pi-ext-cmd" style="padding:14px;">
              <div class="code-shell" style="margin:0;">
                <div class="code-shell-header"><span>shell &mdash; downloads <a href="/pi/modelrelay.ts" style="color:inherit;text-decoration:underline;">/pi/modelrelay.ts</a></span><button type="button" class="copy-btn copy-code-btn">Copy</button></div>
                <pre><code id="pi-ext-cmd-code" class="code-text">{pi_extension_curl}</code></pre>
              </div>
            </div>
            <div class="tab-panel" data-tab="pi-ext-file" style="padding:14px;">
              <div class="code-shell" style="margin:0;">
                <div class="code-shell-header"><span>~/.pi/agent/extensions/modelrelay.ts</span><button type="button" class="copy-btn copy-code-btn">Copy</button></div>
                <pre><code id="pi-ext-code" class="code-text" data-tpl="{t_pi_ts}">{ssr_pi_ts}</code></pre>
              </div>
            </div>
          </div>
        </div>

        <!-- Step 3 -->
        <div class="pi-step">
          <div class="pi-step-head">
            <span class="pi-num">3</span>
            <h3>Sign In (Browser OAuth or 1-Command Paste)</h3>
          </div>
          <p>Run <code>/login modelrelay</code> inside Pi for one-click browser OAuth &mdash; or paste the pre-filled command directly into your terminal (never paste API keys into an LLM chat).</p>
          <div class="tab-group" style="margin-bottom:0;">
            <div class="tab-bar">
              <button type="button" class="tab-btn active" data-tab="auth-login">/login modelrelay (OAuth)</button>
              <button type="button" class="tab-btn" data-tab="auth-shell">1-Command Terminal Paste</button>
              <button type="button" class="tab-btn" data-tab="auth-file">~/.pi/agent/auth.json</button>
              <button type="button" class="tab-btn" data-tab="auth-cmd">Keychain / Secret Manager</button>
            </div>
            <div class="tab-panel active" data-tab="auth-login" style="padding:14px;">
              <p class="tab-note">Start Pi and run <code>/login modelrelay</code>. The provider opens ModelRelay in your browser, approves via PKCE OAuth, and writes <code>~/.pi/agent/auth.json</code> automatically &mdash; zero key copy-paste:</p>
              <div class="code-shell" style="margin:0;">
                <div class="code-shell-header"><span>inside Pi TUI</span><button type="button" class="copy-btn copy-code-btn">Copy</button></div>
                <pre><code class="code-text">/login modelrelay</code></pre>
              </div>
            </div>
            <div class="tab-panel" data-tab="auth-shell" style="padding:14px;">
              <p class="tab-note">Paste directly into your local terminal to merge your pre-filled key into <code>~/.pi/agent/auth.json</code> (<code>chmod 600</code>, preserves existing providers):</p>
              <div class="code-shell" style="margin:0;">
                <div class="code-shell-header"><span>run in your terminal</span><button type="button" class="copy-btn copy-code-btn">Copy</button></div>
                <pre><code class="code-text" data-tpl="{t_pi_auth_shell}"></code></pre>
              </div>
            </div>
            <div class="tab-panel" data-tab="auth-file" style="padding:14px;">
              <p class="tab-note">Or merge this entry manually into <code>~/.pi/agent/auth.json</code> (<code>chmod 600 ~/.pi/agent/auth.json</code>):</p>
              <div class="code-shell" style="margin:0;">
                <div class="code-shell-header"><span>~/.pi/agent/auth.json</span><button type="button" class="copy-btn copy-code-btn">Copy</button></div>
                <pre><code class="code-text" data-tpl="{t_pi_auth}"></code></pre>
              </div>
            </div>
            <div class="tab-panel" data-tab="auth-cmd" style="padding:14px;">
              <p class="tab-note">Prefix <code>key</code> with <code>!</code> to resolve your ModelRelay API key from macOS Keychain, 1Password CLI, or any command at startup without saving plaintext secrets:</p>
              <div class="code-shell" style="margin:0;">
                <div class="code-shell-header"><span>~/.pi/agent/auth.json</span><button type="button" class="copy-btn copy-code-btn">Copy</button></div>
                <pre><code class="code-text" data-tpl="{t_pi_keychain}"></code></pre>
              </div>
            </div>
          </div>
        </div>

        <!-- Step 4 -->
        <div class="pi-step">
          <div class="pi-step-head">
            <span class="pi-num">4</span>
            <h3>Launch Pi &amp; Pick a Model</h3>
          </div>
          <p>Start interactive <code>pi</code> and open <code>/model</code> &mdash; Pi loads <code>modelrelay.ts</code> on startup and whenever <code>/model</code> opens, automatically refreshing your live models, context windows, and thinking levels into <code>~/.pi/agent/models-store.json</code> (note: <code>pi update --models</code> only refreshes built-in providers and does not load user extensions):</p>
          <div class="code-shell" style="margin-bottom:0;">
            <div class="code-shell-header"><span>start Pi</span><button type="button" class="copy-btn copy-code-btn">Copy</button></div>
            <pre><code class="code-text">pi   # run /model to pick a relayed model (Ctrl+S saves default) and /thinking (Shift+Tab) to adjust effort
# After your first interactive refresh, non-interactive commands see the cached catalog too:
pi --list-models modelrelay</code></pre>
          </div>
        </div>
      </div>

      <details class="opt-box">
        <summary>Prefer zero extensions? Configure a static model in <code>~/.pi/agent/models.json</code></summary>
        <div class="opt-body">
          <p>If you only serve one fixed model and don&rsquo;t need live <code>/v1/models</code> discovery, skip Step 2 and declare the model statically in <code>~/.pi/agent/models.json</code> (note: Pi&rsquo;s <code>models.json</code> does not query <code>/v1/models</code>, so you specify the model ID and limits manually). See <a href="/docs/pi">Pi Docs &rarr;</a>.</p>
          <div class="code-shell" style="margin-bottom:0;">
            <div class="code-shell-header"><span>~/.pi/agent/models.json (zero-extension static config)</span><button type="button" class="copy-btn copy-code-btn">Copy</button></div>
            <pre><code id="pi-overrides-code" class="code-text" data-tpl="{t_pi_static}">{ssr_pi_static}</code></pre>
          </div>
        </div>
      </details>
    </section>

    <!-- ═══ 2B. Codex CLI & Claude Code (Pre-Filled Terminal Paste) ═══ -->
    <section class="int-section" id="clients">
      <div class="sec-head">
        <div>
          <div class="sec-eyebrow">Other Coding Agents &mdash; Pre-Filled Copy-Paste</div>
          <h2>Codex CLI &amp; Claude Code</h2>
        </div>
        <span class="badge">Keeps commercial providers intact</span>
      </div>
      <p style="color:var(--text-secondary);margin-bottom:16px;max-width:760px;">
        Copy and run directly in your local terminal (your key is pre-filled when logged in and saved to <code>~/.config/modelrelay/api_key</code> with <code>chmod 600</code> &mdash; no need to paste keys into an LLM chat).
      </p>
      <div class="tab-group">
        <div class="tab-bar">
          <button type="button" class="tab-btn active" data-tab="client-codex">Codex CLI</button>
          <button type="button" class="tab-btn" data-tab="client-claude">Claude Code</button>
        </div>
        <div class="tab-panel active" data-tab="client-codex">
          <p class="tab-note">Adds a separate <code>modelrelay</code> provider to <code>~/.codex/config.toml</code> while leaving plain <code>codex</code> on commercial OpenAI:</p>
          <div class="code-shell">
            <div class="code-shell-header"><span>run in your terminal &mdash; Codex CLI</span><button type="button" class="copy-btn copy-code-btn">Copy</button></div>
            <pre><code id="client-codex-code" class="code-text" data-tpl="{t_codex_setup}"></code></pre>
          </div>
        </div>
        <div class="tab-panel" data-tab="client-claude">
          <p class="tab-note">Adds a separate <code>claude-local</code> helper function while leaving plain <code>claude</code> on commercial Anthropic (requires a backend supporting <code>/v1/messages</code>, such as recent <code>llama-server</code>):</p>
          <div class="code-shell">
            <div class="code-shell-header"><span>run in your terminal &mdash; Claude Code (replace ~/.zshrc with ~/.bashrc on bash)</span><button type="button" class="copy-btn copy-code-btn">Copy</button></div>
            <pre><code id="client-claude-code" class="code-text" data-tpl="{t_claude_setup}"></code></pre>
          </div>
        </div>
      </div>
    </section>

    <!-- ═══ 3. Live Playground ═══ -->
    <section class="int-section" id="playground">
      <div class="sec-head">
        <div>
          <div class="sec-eyebrow">Interactive Verification</div>
          <h2>Try <code>api.modelrelay.io</code> Live</h2>
        </div>
        <span class="badge">Runs directly from your browser</span>
      </div>
      <div class="playground">
        <div class="pg-controls">
          <select id="pg-format" aria-label="API format">
            <option value="chat">POST /v1/chat/completions</option>
            <option value="responses">POST /v1/responses</option>
            <option value="messages">POST /v1/messages (Anthropic)</option>
          </select>
          <label class="pg-toggle"><input type="checkbox" id="pg-stream" checked> Stream SSE</label>
        </div>
        <div class="pg-input-row">
          <input id="pg-prompt" type="text" value="Say hello from my local GPU in one crisp sentence." placeholder="Enter a test prompt…">
          <button type="button" id="pg-send" class="btn">Send Request</button>
          <button type="button" id="pg-stop" class="btn btn-danger" style="display:none;">Cancel</button>
        </div>
        <div id="pg-output" class="pg-output"><span class="pg-placeholder">Response will stream here&hellip;</span></div>
        <div class="pg-meta">
          <span id="pg-status">Ready</span>
          <span>Need deeper reference? <a href="/docs/pi">Pi Docs</a> &middot; <a href="/docs/api">API Docs</a></span>
        </div>
      </div>
    </section>"##
    );

    let cloud_cfg_script = cloud_config_script(cloud_config);
    let extra_body_end = format!("{cloud_cfg_script}<script>{integrate_js}</script>");

    page_shell_custom(
        "Pi & API Integration",
        "/integrate",
        &body,
        logged_in,
        &extra_css,
        &extra_body_end,
    )
}
