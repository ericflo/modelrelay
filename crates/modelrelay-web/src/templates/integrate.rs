use super::{CloudWizardConfig, cloud_config_script, page_shell_custom};

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
      padding: 4px 10px; border-radius: 999px; font-family: var(--font-mono); font-size: 0.76rem;
      background: var(--accent-soft); border: 1px solid var(--accent-border); color: var(--accent-fg);
      cursor: pointer; transition: background 0.15s, color 0.15s;
    }
    .model-chip:hover { background: var(--accent); color: #fff; }
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

    /* Pi Thinking & Context Controls */
    .pi-tune-card {
      background: var(--bg-subtle); border: 1px solid var(--border);
      border-radius: var(--radius-md); padding: 16px; margin-bottom: 16px;
    }
    .pi-tune-grid {
      display: grid; grid-template-columns: 1.4fr 0.8fr 0.8fr; gap: 12px; align-items: end;
    }
    .pi-tune-card select, .pi-tune-card input {
      width: 100%; padding: 9px 11px; background: var(--bg-input);
      border: 1px solid var(--border-strong); border-radius: 8px;
      color: var(--text-primary); font-size: 0.84rem; font-family: var(--font-mono);
    }
    .pi-tune-card select { font-family: inherit; }
    .pi-tune-card select:focus, .pi-tune-card input:focus {
      border-color: var(--accent); box-shadow: 0 0 0 3px var(--accent-soft);
    }
    .pi-ctx-presets { display: flex; gap: 6px; flex-wrap: wrap; margin-top: 10px; align-items: center; }
    .pi-preset-btn {
      padding: 3px 9px; border-radius: 999px; font-family: var(--font-mono); font-size: 0.74rem;
      background: var(--bg-surface); border: 1px solid var(--border-strong); color: var(--text-secondary);
      cursor: pointer; transition: border-color 0.15s, color 0.15s, background 0.15s;
    }
    .pi-preset-btn:hover, .pi-preset-btn.active {
      border-color: var(--accent-border); background: var(--accent-soft); color: var(--accent-fg);
    }
    .pi-note-grid {
      display: grid; grid-template-columns: 1fr 1fr; gap: 12px; margin-top: 14px;
    }
    .pi-note-box {
      background: var(--bg-subtle); border: 1px solid var(--border);
      border-radius: var(--radius-md); padding: 14px 16px; font-size: 0.84rem; color: var(--text-secondary);
    }
    .pi-note-box strong { display: block; color: var(--text-strong); margin-bottom: 4px; font-size: 0.86rem; }

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
      .config-grid, .ep-grid, .pi-why, .pi-tune-grid, .pi-note-grid { grid-template-columns: 1fr; }
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
  const ctxInput = $('#pi-ctx-window');
  const maxTokInput = $('#pi-max-tokens');
  const thinkSelect = $('#pi-thinking-mode');

  const defaultUrl = (cloudCfg && cloudCfg.serverUrl)
    ? cloudCfg.serverUrl
    : (window.location.hostname === 'modelrelay.io' ? 'https://api.modelrelay.io' : (localStorage.getItem('mr_server_url') || 'https://api.modelrelay.io'));

  urlInput.value = defaultUrl;
  keyInput.value = (cloudCfg && cloudCfg.apiKey)
    ? cloudCfg.apiKey
    : (localStorage.getItem('mr_test_api_key') || '');
  modelInput.value = localStorage.getItem('mr_model_name') || '';
  if (ctxInput) ctxInput.value = localStorage.getItem('mr_pi_ctx') || '131072';
  if (maxTokInput) maxTokInput.value = localStorage.getItem('mr_pi_max_tokens') || '32768';
  if (thinkSelect) thinkSelect.value = localStorage.getItem('mr_pi_thinking') || 'openai';

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
  function ctxVal() {
    const n = parseInt((ctxInput && ctxInput.value) || '131072', 10);
    return Number.isFinite(n) && n >= 4096 ? n : 131072;
  }
  function maxTokVal() {
    const c = ctxVal();
    const n = parseInt((maxTokInput && maxTokInput.value) || '32768', 10);
    const v = Number.isFinite(n) && n >= 1024 ? n : 32768;
    return Math.min(v, c);
  }
  function thinkMode() { return (thinkSelect && thinkSelect.value) || 'openai'; }

  function buildPiExtensionTs() {
    const base = sv() + '/v1';
    const ctx = ctxVal();
    const maxTok = maxTokVal();
    const mode = thinkMode();
    const isReasoning = mode !== 'off';

    let thinkingMapBlock = '';
    if (mode === 'qwen-chat-template') {
      thinkingMapBlock = [
        '          ...(reasoning && {',
        '            thinkingLevelMap: {',
        '              off: "off",',
        '              minimal: null,',
        '              low: "low",',
        '              medium: "medium",',
        '              high: "high",',
        '            },',
        '          }),',
      ].join('\n');
    } else if (mode === 'deepseek') {
      thinkingMapBlock = [
        '          ...(reasoning && {',
        '            thinkingLevelMap: {',
        '              off: "disabled",',
        '              minimal: null,',
        '              low: "low",',
        '              medium: "medium",',
        '              high: "high",',
        '            },',
        '          }),',
      ].join('\n');
    } else if (isReasoning) {
      thinkingMapBlock = [
        '          ...(reasoning && {',
        '            thinkingLevelMap: {',
        '              off: "none",',
        '              minimal: null,',
        '              low: "low",',
        '              medium: "medium",',
        '              high: "high",',
        '            },',
        '          }),',
      ].join('\n');
    }

    const compatLines = [
      '            supportsStore: false,',
      '            supportsDeveloperRole: false,',
      '            supportsReasoningEffort: ' + (mode === 'qwen-chat-template' ? 'false' : (isReasoning ? 'reasoning' : 'false')) + ',',
      '            supportsUsageInStreaming: true,',
      '            supportsStrictMode: false,',
      '            maxTokensField: "max_tokens" as const,',
    ];
    if (mode === 'openai') {
      compatLines.push('            ...(reasoning && { thinkingFormat: "openai" as const }),');
    } else if (mode === 'llama-budget') {
      compatLines.push('            ...(reasoning && {');
      compatLines.push('              thinkingFormat: "openai" as const,');
      compatLines.push('              thinkingTokenBudgetField: "thinking_budget_tokens" as const,');
      compatLines.push('            }),');
    } else if (mode === 'qwen-chat-template') {
      compatLines.push('            ...(reasoning && {');
      compatLines.push('              thinkingFormat: "qwen-chat-template" as const,');
      compatLines.push('              thinkingTokenBudgetField: "thinking_budget_tokens" as const,');
      compatLines.push('            }),');
    } else if (mode === 'deepseek') {
      compatLines.push('            ...(reasoning && {');
      compatLines.push('              thinkingFormat: "deepseek" as const,');
      compatLines.push('              requiresReasoningContentOnAssistantMessages: true,');
      compatLines.push('            }),');
    }

    const lines = [
      'import type { ExtensionAPI } from "@earendil-works/pi-coding-agent";',
      '',
      'const DEFAULT_BASE_URL = "' + base + '";',
      '// Match your worker backend context limit (e.g. llama-server -c / vLLM --max-model-len)',
      'const DEFAULT_CONTEXT_WINDOW = Number(process.env.MODELRELAY_CONTEXT_WINDOW) || ' + ctx + ';',
      'const DEFAULT_MAX_TOKENS = Number(process.env.MODELRELAY_MAX_TOKENS) || ' + maxTok + ';',
      '',
      'type RelayModel = {',
      '  id: string;',
      '  context_window?: number;',
      '  context_length?: number;',
      '  max_model_len?: number;',
      '  max_tokens?: number;',
      '  max_completion_tokens?: number;',
      '  reasoning?: boolean;',
      '  meta?: { n_ctx?: number; n_ctx_train?: number };',
      '  architecture?: { input_modalities?: string[] };',
      '};',
      '',
      'export default function (pi: ExtensionAPI) {',
      '  pi.registerProvider("modelrelay", {',
      '    name: "ModelRelay",',
      '    baseUrl: DEFAULT_BASE_URL,',
      '    api: "openai-completions",',
      '    async refreshModels({ credential, stored, publish, allowNetwork, baseUrl, signal }) {',
      '      const root = (baseUrl ?? DEFAULT_BASE_URL).replace(/\\/+$/, "");',
      '      const cached = (stored?.models ?? [])',
      '        .filter((m) => m.provider === "modelrelay" && "maxTokens" in m)',
      '        .map((m) => ({',
      '          id: m.id,',
      '          name: m.name,',
      '          reasoning: m.reasoning,',
      '          thinkingLevelMap: m.thinkingLevelMap,',
      '          input: m.input,',
      '          inputLimits: m.inputLimits,',
      '          cost: m.cost,',
      '          contextWindow: m.contextWindow,',
      '          maxTokens: m.maxTokens,',
      '          samplingParams: m.samplingParams,',
      '          samplingParamsByThinkingLevel: m.samplingParamsByThinkingLevel,',
      '          compat: m.compat,',
      '        }));',
      '',
      '      if (!allowNetwork || credential?.type !== "api_key" || !credential.key) {',
      '        return cached;',
      '      }',
      '',
      '      const res = await fetch(`${root}/models`, {',
      '        headers: { Authorization: `Bearer ${credential.key}` },',
      '        signal,',
      '      });',
      '      if (!res.ok) return cached;',
      '',
      '      const { data = [] } = (await res.json()) as { data?: RelayModel[] };',
      '      const models = data.map((m) => {',
      '        const contextWindow =',
      '          m.meta?.n_ctx ||',
      '          m.context_window ||',
      '          m.context_length ||',
      '          m.max_model_len ||',
      '          m.meta?.n_ctx_train ||',
      '          DEFAULT_CONTEXT_WINDOW;',
      '        const maxTokens = Math.min(',
      '          m.max_completion_tokens || m.max_tokens || DEFAULT_MAX_TOKENS,',
      '          contextWindow,',
      '        );',
      '        const reasoning = m.reasoning ?? ' + (isReasoning ? 'true' : 'false') + ';',
      '        const hasVision = m.architecture?.input_modalities?.includes("image") === true;',
      '',
      '        return {',
      '          id: m.id,',
      '          name: m.id,',
      '          reasoning,',
    ];
    if (thinkingMapBlock) lines.push(thinkingMapBlock);
    lines.push(
      '          input: (hasVision ? ["text", "image"] : ["text"]) as ("text" | "image")[],',
      '          cost: { input: 0, output: 0, cacheRead: 0, cacheWrite: 0 },',
      '          contextWindow,',
      '          maxTokens,',
      '          compat: {',
      compatLines.join('\n'),
      '          },',
      '        };',
      '      });',
      '',
      '      await publish({',
      '        persist: {',
      '          models: models.map((m) => ({',
      '            ...m,',
      '            api: "openai-completions" as const,',
      '            provider: "modelrelay",',
      '            baseUrl: root,',
      '          })),',
      '          checkedAt: Date.now(),',
      '        },',
      '      });',
      '',
      '      return models;',
      '    },',
      '  });',
      '}'
    );
    return lines.join('\n');
  }

  function buildPiOverridesJson() {
    const model = mn();
    const ctx = ctxVal();
    const maxTok = maxTokVal();
    const mode = thinkMode();
    const override = {
      reasoning: mode !== 'off',
      input: ['text', 'image'],
      contextWindow: ctx,
      maxTokens: maxTok,
    };
    if (mode !== 'off') {
      override.thinkingLevelMap = mode === 'qwen-chat-template'
        ? { off: 'off', minimal: null, low: 'low', medium: 'medium', high: 'high' }
        : { off: 'none', minimal: null, low: 'low', medium: 'medium', high: 'high', xhigh: 'xhigh' };
      override.compat = mode === 'qwen-chat-template'
        ? { thinkingFormat: 'qwen-chat-template', thinkingTokenBudgetField: 'thinking_budget_tokens' }
        : mode === 'deepseek'
        ? { thinkingFormat: 'deepseek', requiresReasoningContentOnAssistantMessages: true }
        : mode === 'llama-budget'
        ? { thinkingFormat: 'openai', thinkingTokenBudgetField: 'thinking_budget_tokens' }
        : { thinkingFormat: 'openai', supportsReasoningEffort: true };
    }
    return JSON.stringify({
      $schema: 'https://pi.dev/schemas/models.schema.json',
      providers: {
        modelrelay: {
          modelOverrides: {
            [model]: override,
          },
        },
      },
    }, null, 2);
  }

  function syncPresetChips() {
    const cur = String(ctxVal());
    $$('.pi-preset-btn').forEach(btn => {
      btn.classList.toggle('active', btn.dataset.ctx === cur);
    });
  }

  function renderAllSnippets() {
    const s = sv(), a = ak(), m = mn();
    $$('[data-tpl]').forEach(el => {
      const raw = el.getAttribute('data-tpl');
      el.textContent = raw
        .replace(/__SERVER_URL__/g, s)
        .replace(/__API_KEY__/g, a)
        .replace(/__MODEL_NAME__/g, m);
    });
    const ts = buildPiExtensionTs();
    const extEl = $('#pi-ext-code');
    if (extEl) extEl.textContent = ts;
    const cmdEl = $('#pi-ext-cmd-code');
    if (cmdEl) cmdEl.textContent = "mkdir -p ~/.pi/agent/extensions\ncat <<'EOF' > ~/.pi/agent/extensions/modelrelay.ts\n" + ts + "\nEOF";
    const ovEl = $('#pi-overrides-code');
    if (ovEl) ovEl.textContent = buildPiOverridesJson();
    syncPresetChips();
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
  if (ctxInput) ctxInput.addEventListener('input', () => { localStorage.setItem('mr_pi_ctx', ctxInput.value.trim()); renderAllSnippets(); });
  if (maxTokInput) maxTokInput.addEventListener('input', () => { localStorage.setItem('mr_pi_max_tokens', maxTokInput.value.trim()); renderAllSnippets(); });
  if (thinkSelect) thinkSelect.addEventListener('change', () => { localStorage.setItem('mr_pi_thinking', thinkSelect.value); renderAllSnippets(); });
  $$('.pi-preset-btn').forEach(btn => {
    btn.addEventListener('click', () => {
      if (ctxInput && btn.dataset.ctx) {
        ctxInput.value = btn.dataset.ctx;
        localStorage.setItem('mr_pi_ctx', btn.dataset.ctx);
      }
      if (maxTokInput && btn.dataset.maxtok) {
        maxTokInput.value = btn.dataset.maxtok;
        localStorage.setItem('mr_pi_max_tokens', btn.dataset.maxtok);
      }
      renderAllSnippets();
    });
  });
  renderAllSnippets();

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
        const models = (payload.data || []).map(x => x.id).filter(Boolean);
        if (models.length === 0) {
          discoverStatus.textContent = 'Connected, but no workers are advertising models right now.';
          return;
        }
        discoverStatus.textContent = models.length + ' live model' + (models.length === 1 ? '' : 's') + ' (click to select):';
        models.forEach(id => {
          const chip = document.createElement('button');
          chip.type = 'button';
          chip.className = 'model-chip';
          chip.textContent = id;
          chip.addEventListener('click', () => {
            modelInput.value = id;
            localStorage.setItem('mr_model_name', id);
            renderAllSnippets();
          });
          discoverChips.appendChild(chip);
        });
        if (!modelInput.value.trim() && models[0]) {
          modelInput.value = models[0];
          localStorage.setItem('mr_model_name', models[0]);
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

    let pi_extension_ts = r#"import type { ExtensionAPI } from "@earendil-works/pi-coding-agent";

const DEFAULT_BASE_URL = "https://api.modelrelay.io/v1";
// Match your worker backend context limit (e.g. llama-server -c / vLLM --max-model-len)
const DEFAULT_CONTEXT_WINDOW = Number(process.env.MODELRELAY_CONTEXT_WINDOW) || 131072;
const DEFAULT_MAX_TOKENS = Number(process.env.MODELRELAY_MAX_TOKENS) || 32768;

type RelayModel = {
  id: string;
  context_window?: number;
  context_length?: number;
  max_model_len?: number;
  max_tokens?: number;
  max_completion_tokens?: number;
  reasoning?: boolean;
  meta?: { n_ctx?: number; n_ctx_train?: number };
  architecture?: { input_modalities?: string[] };
};

export default function (pi: ExtensionAPI) {
  pi.registerProvider("modelrelay", {
    name: "ModelRelay",
    baseUrl: DEFAULT_BASE_URL,
    api: "openai-completions",
    async refreshModels({ credential, stored, publish, allowNetwork, baseUrl, signal }) {
      const root = (baseUrl ?? DEFAULT_BASE_URL).replace(/\/+$/, "");
      const cached = (stored?.models ?? [])
        .filter((m) => m.provider === "modelrelay" && "maxTokens" in m)
        .map((m) => ({
          id: m.id,
          name: m.name,
          reasoning: m.reasoning,
          thinkingLevelMap: m.thinkingLevelMap,
          input: m.input,
          inputLimits: m.inputLimits,
          cost: m.cost,
          contextWindow: m.contextWindow,
          maxTokens: m.maxTokens,
          samplingParams: m.samplingParams,
          samplingParamsByThinkingLevel: m.samplingParamsByThinkingLevel,
          compat: m.compat,
        }));

      if (!allowNetwork || credential?.type !== "api_key" || !credential.key) {
        return cached;
      }

      const res = await fetch(`${root}/models`, {
        headers: { Authorization: `Bearer ${credential.key}` },
        signal,
      });
      if (!res.ok) return cached;

      const { data = [] } = (await res.json()) as { data?: RelayModel[] };
      const models = data.map((m) => {
        const contextWindow =
          m.meta?.n_ctx ||
          m.context_window ||
          m.context_length ||
          m.max_model_len ||
          m.meta?.n_ctx_train ||
          DEFAULT_CONTEXT_WINDOW;
        const maxTokens = Math.min(
          m.max_completion_tokens || m.max_tokens || DEFAULT_MAX_TOKENS,
          contextWindow,
        );
        const reasoning = m.reasoning ?? true;
        const hasVision = m.architecture?.input_modalities?.includes("image") === true;

        return {
          id: m.id,
          name: m.id,
          reasoning,
          ...(reasoning && {
            thinkingLevelMap: {
              off: "none",
              minimal: null,
              low: "low",
              medium: "medium",
              high: "high",
            },
          }),
          input: (hasVision ? ["text", "image"] : ["text"]) as ("text" | "image")[],
          cost: { input: 0, output: 0, cacheRead: 0, cacheWrite: 0 },
          contextWindow,
          maxTokens,
          compat: {
            supportsStore: false,
            supportsDeveloperRole: false,
            supportsReasoningEffort: reasoning,
            supportsUsageInStreaming: true,
            supportsStrictMode: false,
            maxTokensField: "max_tokens" as const,
            ...(reasoning && { thinkingFormat: "openai" as const }),
          },
        };
      });

      await publish({
        persist: {
          models: models.map((m) => ({
            ...m,
            api: "openai-completions" as const,
            provider: "modelrelay",
            baseUrl: root,
          })),
          checkedAt: Date.now(),
        },
      });

      return models;
    },
  });
}"#;

    let pi_extension_oneliner = format!(
        "mkdir -p ~/.pi/agent/extensions\ncat <<'EOF' > ~/.pi/agent/extensions/modelrelay.ts\n{pi_extension_ts}\nEOF"
    );

    let pi_auth_json =
        "{\n  \"modelrelay\": {\n    \"type\": \"api_key\",\n    \"key\": \"__API_KEY__\"\n  }\n}";

    let pi_auth_keychain = "{\n  \"modelrelay\": {\n    \"type\": \"api_key\",\n    \"key\": \"!security find-generic-password -ws 'modelrelay'\"\n  }\n}";

    let pi_overrides_json = "{\n  \"$schema\": \"https://pi.dev/schemas/models.schema.json\",\n  \"providers\": {\n    \"modelrelay\": {\n      \"modelOverrides\": {\n        \"qwen3.5-27b\": {\n          \"reasoning\": true,\n          \"input\": [\"text\", \"image\"],\n          \"contextWindow\": 131072,\n          \"maxTokens\": 32768,\n          \"thinkingLevelMap\": {\n            \"off\": \"none\",\n            \"minimal\": null,\n            \"low\": \"low\",\n            \"medium\": \"medium\",\n            \"high\": \"high\",\n            \"xhigh\": \"xhigh\"\n          },\n          \"compat\": {\n            \"thinkingFormat\": \"openai\",\n            \"supportsReasoningEffort\": true\n          }\n        }\n      }\n    }\n  }\n}";

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
    let ssr_pi_ts = esc_attr(pi_extension_ts);
    let ssr_pi_cmd = esc_attr(&pi_extension_oneliner);
    let t_pi_auth = esc_attr(pi_auth_json);
    let t_pi_keychain = esc_attr(pi_auth_keychain);
    let ssr_pi_overrides = esc_attr(pi_overrides_json);

    let logged_in = cloud_config.is_some();
    let extra_css = format!("<style>{integrate_css}</style>");

    let body = format!(
        r##"<div class="int-hero">
      <div class="sec-eyebrow">Connect Clients</div>
      <h1>Pi Coding Agent &amp; <code>api.modelrelay.io</code></h1>
      <p>Every GPU worker you connect appears behind one authenticated HTTPS endpoint. Verify it in seconds with <code>curl</code>, then add ModelRelay as a live-discovering provider in Pi.</p>
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
        <span class="discover-status" id="discover-status">Queries <code>GET /v1/models</code> on your relay to populate model IDs.</span>
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
          <p>Returns the live list of models currently advertised across your connected workers.</p>
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
          <p class="tab-note">Verify your token and see which models your connected workers are currently serving:</p>
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
        <span class="badge badge-accent">Provider Extension + auth.json</span>
      </div>

      <p style="color:var(--text-secondary);margin-bottom:18px;max-width:760px;">
        Instead of hardcoding individual models and pasting plaintext API keys into <code>models.json</code>, register ModelRelay once as a <strong>Pi provider extension</strong> and keep your token cleanly in <code>~/.pi/agent/auth.json</code>.
      </p>

      <div class="pi-why">
        <div class="pi-why-item">
          <strong>Live model discovery</strong>
          <span>Queries <code>/v1/models</code> automatically whenever you open <code>/model</code> or run <code>pi update --models</code>.</span>
        </div>
        <div class="pi-why-item">
          <strong>Thinking levels &amp; context limits</strong>
          <span>Conveys <code>reasoning</code>, <code>thinkingLevelMap</code>, <code>compat</code>, <code>contextWindow</code>, and <code>maxTokens</code> so <code>/thinking</code> and auto-compaction work accurately.</span>
        </div>
        <div class="pi-why-item">
          <strong>Clean credentials in <code>auth.json</code></strong>
          <span>Never store API keys in <code>models.json</code>. Supports direct <code>auth.json</code>, <code>/login modelrelay</code>, or keychain commands.</span>
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
            <h3>Add ModelRelay as a Provider (<code>~/.pi/agent/extensions/modelrelay.ts</code>)</h3>
          </div>
          <p>Drop this provider extension into <code>~/.pi/agent/extensions/</code>. Set your worker&rsquo;s context window, max output tokens, and thinking mode below so Pi conveys <code>/thinking</code> levels and compacts before hitting your GPU&rsquo;s KV-cache limit:</p>

          <div class="pi-tune-card">
            <div class="pi-tune-grid">
              <div class="cfg-field">
                <label for="pi-thinking-mode">Thinking / Reasoning Mode</label>
                <select id="pi-thinking-mode">
                  <option value="openai">OpenAI reasoning_effort (llama.cpp / vLLM / Ollama)</option>
                  <option value="llama-budget">llama.cpp reasoning_effort + thinking_budget_tokens</option>
                  <option value="qwen-chat-template">Qwen3 / Qwen3.5 chat_template_kwargs (enable_thinking)</option>
                  <option value="deepseek">DeepSeek / GLM (thinking.type + reasoning_content)</option>
                  <option value="off">Non-reasoning models (thinking off)</option>
                </select>
              </div>
              <div class="cfg-field">
                <label for="pi-ctx-window">Context Window (tokens)</label>
                <input id="pi-ctx-window" type="number" min="4096" step="4096" value="131072">
              </div>
              <div class="cfg-field">
                <label for="pi-max-tokens">Max Output Tokens</label>
                <input id="pi-max-tokens" type="number" min="1024" step="1024" value="32768">
              </div>
            </div>
            <div class="pi-ctx-presets">
              <span style="font-size:0.76rem;color:var(--text-muted);">Quick context presets (match your <code>llama-server -c</code> / vLLM <code>--max-model-len</code>):</span>
              <button type="button" class="pi-preset-btn" data-ctx="32768" data-maxtok="8192">32k / 8k</button>
              <button type="button" class="pi-preset-btn" data-ctx="65536" data-maxtok="16384">64k / 16k</button>
              <button type="button" class="pi-preset-btn active" data-ctx="131072" data-maxtok="32768">128k / 32k</button>
              <button type="button" class="pi-preset-btn" data-ctx="262144" data-maxtok="65536">256k / 64k</button>
            </div>
          </div>

          <div class="tab-group" style="margin-bottom:0;">
            <div class="tab-bar">
              <button type="button" class="tab-btn active" data-tab="pi-ext-file">modelrelay.ts</button>
              <button type="button" class="tab-btn" data-tab="pi-ext-cmd">One-command install</button>
            </div>
            <div class="tab-panel active" data-tab="pi-ext-file" style="padding:14px;">
              <div class="code-shell" style="margin:0;">
                <div class="code-shell-header"><span>~/.pi/agent/extensions/modelrelay.ts</span><button type="button" class="copy-btn copy-code-btn">Copy</button></div>
                <pre><code id="pi-ext-code" class="code-text">{ssr_pi_ts}</code></pre>
              </div>
            </div>
            <div class="tab-panel" data-tab="pi-ext-cmd" style="padding:14px;">
              <div class="code-shell" style="margin:0;">
                <div class="code-shell-header"><span>shell &mdash; writes ~/.pi/agent/extensions/modelrelay.ts</span><button type="button" class="copy-btn copy-code-btn">Copy</button></div>
                <pre><code id="pi-ext-cmd-code" class="code-text">{ssr_pi_cmd}</code></pre>
              </div>
            </div>
          </div>

          <div class="pi-note-grid">
            <div class="pi-note-box">
              <strong>Why <code>thinkingLevelMap</code> matters</strong>
              Mapping <code>off: &quot;none&quot;</code> makes <code>/thinking off</code> actively disable thinking instead of leaving backend defaults on, and hiding <code>minimal: null</code> avoids HTTP 400 errors on Jinja templates that only accept <code>low</code>/<code>medium</code>/<code>high</code>.
            </div>
            <div class="pi-note-box">
              <strong>Why <code>contextWindow</code> &amp; <code>maxTokens</code> matter</strong>
              Pi uses <code>contextWindow</code> for its context meter, <code>max_tokens</code> clamping, and auto-compaction, while <code>supportsUsageInStreaming: true</code> feeds real token counts back on every turn.
            </div>
          </div>
        </div>

        <!-- Step 3 -->
        <div class="pi-step">
          <div class="pi-step-head">
            <span class="pi-num">3</span>
            <h3>Add Your Token to <code>~/.pi/agent/auth.json</code></h3>
          </div>
          <p>Store your ModelRelay API key under the <code>"modelrelay"</code> provider key in <code>~/.pi/agent/auth.json</code> &mdash; or run <code>/login modelrelay</code> inside Pi to save it interactively.</p>
          <div class="tab-group" style="margin-bottom:0;">
            <div class="tab-bar">
              <button type="button" class="tab-btn active" data-tab="auth-file">~/.pi/agent/auth.json</button>
              <button type="button" class="tab-btn" data-tab="auth-login">/login command</button>
              <button type="button" class="tab-btn" data-tab="auth-cmd">Keychain / Secret Manager</button>
            </div>
            <div class="tab-panel active" data-tab="auth-file" style="padding:14px;">
              <p class="tab-note">Merge this entry into your existing <code>~/.pi/agent/auth.json</code> (keep permissions private with <code>chmod 600 ~/.pi/agent/auth.json</code>):</p>
              <div class="code-shell" style="margin:0;">
                <div class="code-shell-header"><span>~/.pi/agent/auth.json</span><button type="button" class="copy-btn copy-code-btn">Copy</button></div>
                <pre><code class="code-text" data-tpl="{t_pi_auth}"></code></pre>
              </div>
            </div>
            <div class="tab-panel" data-tab="auth-login" style="padding:14px;">
              <p class="tab-note">Start Pi and run <code>/login</code>. Because <code>modelrelay</code> is registered as a provider, Pi prompts for your API key and writes <code>~/.pi/agent/auth.json</code> for you:</p>
              <div class="code-shell" style="margin:0;">
                <div class="code-shell-header"><span>inside Pi TUI</span><button type="button" class="copy-btn copy-code-btn">Copy</button></div>
                <pre><code class="code-text">/login modelrelay</code></pre>
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
            <h3>Verify, Pick a Model &amp; Set Thinking Level</h3>
          </div>
          <p>Refresh the catalog, select any live model with <code>/model</code>, and dial reasoning depth with <code>/thinking</code> (<code>Shift+Tab</code> cycles levels live):</p>
          <div class="code-shell" style="margin-bottom:0;">
            <div class="code-shell-header"><span>verify in terminal</span><button type="button" class="copy-btn copy-code-btn">Copy</button></div>
            <pre><code class="code-text"># Fetch live models from api.modelrelay.io and inspect context/thinking support
pi update --models
pi --list-models

# Launch Pi, pick a model (/model), and choose a thinking level (/thinking, Ctrl+S to save default)
pi</code></pre>
          </div>
        </div>
      </div>

      <details class="opt-box">
        <summary>Per-model context limits, thinking formats &amp; vision overrides in <code>models.json</code></summary>
        <div class="opt-body">
          <p>Running mixed models across GPU boxes (for example a 64k Qwen3.5 worker alongside a 256k coder or non-reasoning model)? Pi layers <code>modelOverrides</code> from <code>~/.pi/agent/models.json</code> on top of your registered <code>modelrelay</code> provider &mdash; zero secrets in <code>models.json</code>, and live discovery stays intact. See <a href="/docs/pi#thinking">Pi Thinking &amp; Context Docs &rarr;</a> for all backend formats (<code>thinkingFormat</code>, <code>thinkingTokenBudgetField</code>, and <code>samplingParamsByThinkingLevel</code>).</p>
          <div class="code-shell" style="margin-bottom:0;">
            <div class="code-shell-header"><span>~/.pi/agent/models.json (optional per-model overrides)</span><button type="button" class="copy-btn copy-code-btn">Copy</button></div>
            <pre><code id="pi-overrides-code" class="code-text">{ssr_pi_overrides}</code></pre>
          </div>
        </div>
      </details>
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
