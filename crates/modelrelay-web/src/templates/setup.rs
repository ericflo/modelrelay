use std::fmt::Write;

use super::{CloudWizardConfig, cloud_config_script, page_shell_custom};

/// Build the worker onboarding setup wizard page (self-hosted default).
#[must_use]
pub fn setup_wizard_page() -> String {
    setup_wizard_page_with_config(None)
}

/// Build the worker setup wizard page, optionally pre-configured for logged-in cloud users.
#[must_use]
#[allow(clippy::too_many_lines)]
pub fn setup_wizard_page_with_config(cloud_config: Option<&CloudWizardConfig>) -> String {
    let wizard_css = r"
    .wiz-head { margin-bottom: 24px; }
    .wiz-head h1 { font-size: 2rem; font-weight: 800; letter-spacing: -0.03em; margin-bottom: 8px; }
    .wiz-head p { color: var(--text-secondary); }

    .desktop-shortcut {
      display: flex; align-items: center; justify-content: space-between; gap: 18px; flex-wrap: wrap;
      background: linear-gradient(135deg, rgba(99, 102, 241, 0.14), rgba(56, 189, 248, 0.08));
      border: 1px solid var(--accent-border); border-radius: var(--radius-lg);
      padding: 20px 24px; margin-bottom: 28px;
    }
    .desktop-shortcut h2 { font-size: 1.05rem; margin-bottom: 4px; color: var(--text-primary); }
    .desktop-shortcut p { font-size: 0.88rem; color: var(--text-secondary); margin: 0; max-width: 560px; }

    .wizard-progress {
      display: grid; grid-template-columns: repeat(8, minmax(76px, 1fr));
      gap: 4px; margin-bottom: 24px; overflow-x: auto;
    }
    .step-indicator {
      text-align: center; padding: 10px 6px; font-size: 0.74rem; font-weight: 600;
      color: var(--text-muted); border-bottom: 2px solid var(--border);
      cursor: pointer; user-select: none; transition: color 0.15s, border-color 0.15s;
    }
    .step-indicator:hover { color: var(--text-primary); }
    .step-indicator.active { color: var(--accent-bright); border-bottom-color: var(--accent-bright); }
    .step-indicator.done { color: var(--success); border-bottom-color: var(--success); }

    .wizard-step { display: none; }
    .wizard-step.active { display: block; }

    .wizard-card {
      background: var(--bg-surface); border: 1px solid var(--border);
      border-radius: var(--radius-lg); padding: 28px; margin-bottom: 18px;
      box-shadow: var(--shadow-card);
    }
    .wizard-card h2 {
      font-size: 1.18rem; margin-bottom: 10px; color: var(--text-primary);
      display: flex; align-items: center; gap: 10px;
    }
    .wizard-card p { color: var(--text-secondary); margin-bottom: 12px; font-size: 0.92rem; }
    .step-num {
      display: inline-flex; align-items: center; justify-content: center;
      width: 28px; height: 28px; border-radius: 8px;
      background: var(--accent-soft); border: 1px solid var(--accent-border);
      color: var(--accent-fg); font-size: 0.8rem; font-weight: 700; flex-shrink: 0;
    }

    .platform-tabs { display: flex; gap: 8px; margin: 14px 0 18px; flex-wrap: wrap; }
    .platform-tabs .tab {
      padding: 8px 16px; background: var(--bg-input); border: 1px solid var(--border-strong);
      border-radius: 8px; color: var(--text-secondary); cursor: pointer;
      font-size: 0.86rem; font-weight: 600; transition: all 0.15s;
    }
    .platform-tabs .tab:hover { border-color: var(--accent-border); color: var(--text-strong); }
    .platform-tabs .tab.active { background: var(--accent); border-color: var(--accent); color: #fff; }

    .platform-content, .backend-content { display: none; }
    .platform-content.active, .backend-content.active { display: block; }

    .hint-box {
      background: var(--bg-subtle); border: 1px solid var(--border);
      border-radius: var(--radius-md); padding: 15px 18px; margin: 12px 0;
      font-size: 0.86rem; color: var(--text-secondary); line-height: 1.75;
    }
    .hint-box strong { color: var(--text-strong); }

    .config-input { display: flex; gap: 10px; align-items: center; flex-wrap: wrap; margin: 10px 0; }
    .config-input label { color: var(--text-secondary); font-size: 0.84rem; min-width: 130px; font-weight: 500; }
    .config-input input {
      padding: 9px 12px; background: var(--bg-input); border: 1px solid var(--border-strong);
      border-radius: 8px; color: var(--text-primary); font-size: 0.86rem;
      font-family: var(--font-mono); flex: 1; min-width: 220px;
    }
    .config-input input:focus { border-color: var(--accent); box-shadow: 0 0 0 3px var(--accent-soft); }

    .wizard-nav {
      display: flex; justify-content: space-between; align-items: center; gap: 12px;
      margin-top: 20px; padding-top: 18px; border-top: 1px solid var(--border);
    }

    .status-indicator {
      display: flex; align-items: center; gap: 12px; padding: 15px 18px;
      background: var(--bg-subtle); border: 1px solid var(--border);
      border-radius: var(--radius-md); margin: 14px 0; font-size: 0.92rem;
    }
    .status-indicator .pulse {
      width: 11px; height: 11px; border-radius: 50%; background: var(--text-muted); flex-shrink: 0;
    }
    .status-indicator .pulse.searching { background: var(--warning); animation: pulse 1.4s ease-in-out infinite; }
    .status-indicator .pulse.connected { background: var(--success); box-shadow: 0 0 10px rgba(52, 211, 153, 0.55); }
    @keyframes pulse { 0%, 100% { opacity: 1; } 50% { opacity: 0.35; } }

    .test-result {
      background: var(--bg-code); border: 1px solid var(--code-border); border-radius: var(--radius-md);
      padding: 15px; margin-top: 14px; font-family: var(--font-mono); font-size: 0.84rem;
      color: var(--code-fg); max-height: 280px; overflow-y: auto; white-space: pre-wrap;
    }
    .skip-link {
      display: none; margin-top: 8px; color: var(--accent-bright); cursor: pointer;
      font-size: 0.84rem; background: none; border: none; font-family: inherit;
    }
    .skip-link:hover { text-decoration: underline; }
    .check-mark { color: var(--success); font-weight: 700; margin-right: 4px; }

    @media (max-width: 680px) {
      .wizard-card { padding: 20px; }
      .config-input { flex-direction: column; align-items: stretch; }
      .config-input label, .config-input input { min-width: 0; width: 100%; }
      .wizard-nav { flex-direction: column-reverse; }
      .wizard-nav .btn { width: 100%; }
    }
    ";

    let wizard_js = r#"
(function() {
  const STEPS = 8;
  let currentStep = 1;
  let detectedPlatform = 'linux';
  let selectedBackend = 'llamacpp';
  let workerPollInterval = null;
  let workerPollSession = 0;
  let troubleshootTimer = null;
  let initialWorkerIds = new Set();
  let detectedModels = [];

  const $ = s => document.querySelector(s);
  const $$ = s => document.querySelectorAll(s);

  const ua = navigator.userAgent.toLowerCase();
  if (ua.includes('mac')) detectedPlatform = 'macos';
  else if (ua.includes('win')) detectedPlatform = 'windows';

  const cloudCfg = window.__mrCloudConfig || null;

  function getAdminToken() { return cloudCfg ? '' : (localStorage.getItem('mr_admin_token') || ''); }
  function getServerUrl() {
    if (cloudCfg && cloudCfg.serverUrl) return cloudCfg.serverUrl;
    return localStorage.getItem('mr_server_url') || 'https://api.modelrelay.io';
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
  function escHtml(s) { const d = document.createElement('div'); d.textContent = s; return d.innerHTML; }

  function goToStep(n) {
    if (n < 1 || n > STEPS) return;
    currentStep = n;
    $$('.wizard-step').forEach((el, i) => el.classList.toggle('active', i + 1 === n));
    $$('.step-indicator').forEach((el, i) => {
      el.classList.remove('active', 'done');
      if (i + 1 === n) el.classList.add('active');
      else if (i + 1 < n) el.classList.add('done');
    });
    if (n === 6) startWorkerPoll();
    else stopWorkerPoll();
  }

  window.__wizNext = () => goToStep(currentStep + 1);
  window.__wizPrev = () => goToStep(currentStep - 1);
  window.__wizGoTo = goToStep;

  window.__setPlatform = function(p) {
    detectedPlatform = p;
    $$('.platform-tabs:not(.backend-tabs):not(.persist-tabs) .tab').forEach(t => t.classList.toggle('active', t.dataset.platform === p));
    $$('.platform-content').forEach(el => el.classList.toggle('active', el.dataset.platform === p));
    updateDownloadLinks();
    updateConfigSnippet();
    window.__setPersistPlatform(p);
  };

  window.__setBackend = function(b) {
    selectedBackend = b;
    $$('.backend-tabs .tab').forEach(t => t.classList.toggle('active', t.dataset.backend === b));
    $$('.backend-content').forEach(el => el.classList.toggle('active', el.dataset.backend === b));
    const portHint = $('#backend-port-hint');
    if (portHint) portHint.textContent = getBackendPort();
    updateConfigSnippet();
  };

  window.__setPersistPlatform = function(p) {
    $$('.persist-content').forEach(el => el.style.display = el.dataset.platform === p ? 'block' : 'none');
    $$('.persist-tabs .tab').forEach(t => t.classList.toggle('active', t.dataset.platform === p));
  };

  function updateDownloadLinks() {
    const base = 'https://github.com/ericflo/modelrelay/releases/latest/download';
    const binMap = {
      macos: 'modelrelay-worker-darwin-arm64',
      windows: 'modelrelay-worker-windows-amd64.exe',
      linux: 'modelrelay-worker-linux-amd64',
    };
    const bin = binMap[detectedPlatform] || binMap.linux;
    const el = $('#download-cmd');
    if (el) {
      el.textContent = detectedPlatform === 'windows'
        ? 'curl.exe -L -o modelrelay-worker.exe ' + base + '/' + bin
        : 'curl -L -o modelrelay-worker ' + base + '/' + bin + ' && chmod +x modelrelay-worker';
    }
  }

  function getBackendPort() {
    if (selectedBackend === 'lmstudio') return '1234';
    if (selectedBackend === 'ollama') return '11434';
    return '8000';
  }

  function updateConfigSnippet() {
    const serverUrl = ($('#cfg-server-url') && $('#cfg-server-url').value.trim()) || getServerUrl();
    const secret = ($('#cfg-worker-secret') && $('#cfg-worker-secret').value.trim()) || 'mr_live_your_api_key';
    const workerName = ($('#cfg-worker-name') && $('#cfg-worker-name').value.trim()) || 'my-gpu-box';
    const port = getBackendPort();

    const tomlEl = $('#config-toml');
    if (tomlEl) {
      tomlEl.textContent =
        'proxy_url = "' + serverUrl + '"\n' +
        'worker_secret = "' + secret + '"\n' +
        'worker_name = "' + workerName + '"\n' +
        'backend_url = "http://127.0.0.1:' + port + '"\n' +
        'models = ["*"]';
    }
    const envEl = $('#config-env');
    if (envEl) {
      envEl.textContent =
        'export PROXY_URL="' + serverUrl + '"\n' +
        'export WORKER_SECRET="' + secret + '"\n' +
        'export WORKER_NAME="' + workerName + '"\n' +
        'export BACKEND_URL="http://127.0.0.1:' + port + '"\n' +
        'export MODELS="*"';
    }
    const curlEl = $('#curl-test');
    if (curlEl) {
      const apiKeyInput = $('#test-api-key');
      const apiKey = (cloudCfg && cloudCfg.apiKey) || (apiKeyInput ? apiKeyInput.value.trim() : '') || 'mr_live_your_api_key';
      const testModel = ($('#test-model') && $('#test-model').value.trim()) || 'your-model';
      curlEl.textContent =
        'curl ' + serverUrl + '/v1/chat/completions \\\n' +
        '  -H "Content-Type: application/json" \\\n' +
        '  -H "Authorization: Bearer ' + apiKey + '" \\\n' +
        '  -d \'{"model":"' + testModel + '","messages":[{"role":"user","content":"Hello!"}],"max_tokens":100}\'';
    }
  }

  async function snapshotWorkers() {
    try {
      const opts = cloudCfg ? { credentials: 'same-origin' } : { headers: authHeaders() };
      const r = await fetch(getWorkersPollUrl(), opts);
      if (!r.ok) return;
      const d = await r.json();
      (d.workers || []).forEach(w => initialWorkerIds.add(w.worker_id));
    } catch (_) {}
  }

  async function startWorkerPoll() {
    stopWorkerPoll();
    const pollSession = ++workerPollSession;
    const pulse = $('#worker-pulse');
    const statusText = $('#worker-status-text');
    const troubleshoot = $('#troubleshoot-hints');
    const skipBtn = $('#skip-detect');
    if (pulse) pulse.className = 'pulse searching';
    if (statusText) statusText.textContent = 'Waiting for worker to connect…';
    if (troubleshoot) troubleshoot.style.display = 'none';
    if (skipBtn) skipBtn.style.display = 'none';

    const checkWorkers = async () => {
      try {
        const opts = cloudCfg ? { credentials: 'same-origin' } : { headers: authHeaders() };
        const r = await fetch(getWorkersPollUrl(), opts);
        if (!r.ok || pollSession !== workerPollSession) return false;
        const d = await r.json();
        const workers = d.workers || [];
        const configuredName = ($('#cfg-worker-name') && $('#cfg-worker-name').value.trim()) || '';
        const newWorker = workers.find(w => !initialWorkerIds.has(w.worker_id))
          || (configuredName && workers.find(w => w.worker_name === configuredName))
          || (workers.length > 0 && initialWorkerIds.size === 0 ? workers[0] : null);
        if (newWorker) {
          stopWorkerPoll();
          if (pulse) pulse.className = 'pulse connected';
          if (statusText) {
            const name = newWorker.worker_name || newWorker.worker_id;
            detectedModels = newWorker.models || [];
            const modelsDisplay = detectedModels.join(', ') === '*' ? 'all models' : detectedModels.join(', ');
            statusText.innerHTML = '<span class="check-mark">&#10003;</span> Worker <strong>' + escHtml(name) + '</strong> connected' +
              (modelsDisplay ? ' <span style="color:var(--text-secondary);">(' + escHtml(modelsDisplay) + ')</span>' : '');
          }
          const testModel = $('#test-model');
          if (testModel && detectedModels.length > 0 && !testModel.value) {
            testModel.value = detectedModels[0];
            updateConfigSnippet();
          }
          return true;
        }
      } catch (_) {}
      return false;
    };

    await snapshotWorkers();
    if (pollSession !== workerPollSession) return;
    if (await checkWorkers()) return;

    let elapsed = 0;
    troubleshootTimer = setInterval(() => {
      elapsed += 1;
      if (elapsed >= 15 && troubleshoot) troubleshoot.style.display = 'block';
      if (elapsed >= 30 && skipBtn) skipBtn.style.display = 'inline-block';
    }, 1000);
    workerPollInterval = setInterval(checkWorkers, 3000);
  }

  function stopWorkerPoll() {
    workerPollSession += 1;
    if (workerPollInterval) { clearInterval(workerPollInterval); workerPollInterval = null; }
    if (troubleshootTimer) { clearInterval(troubleshootTimer); troubleshootTimer = null; }
  }

  window.__testInference = async function() {
    const resultEl = $('#test-result');
    const btnEl = $('#test-btn');
    if (!resultEl || !btnEl) return;
    btnEl.disabled = true;
    btnEl.textContent = 'Sending…';
    resultEl.textContent = 'Sending request…';
    resultEl.style.display = 'block';

    const serverUrl = ($('#cfg-server-url') && $('#cfg-server-url').value.trim()) || getServerUrl();
    const model = ($('#test-model') && $('#test-model').value.trim()) || 'default';
    const apiKeyInput = $('#test-api-key');
    const apiKey = (cloudCfg && cloudCfg.apiKey) || (apiKeyInput ? apiKeyInput.value.trim() : '') || '';

    try {
      const headers = { 'Content-Type': 'application/json' };
      if (apiKey) headers['Authorization'] = 'Bearer ' + apiKey;
      const r = await fetch(serverUrl + '/v1/chat/completions', {
        method: 'POST',
        headers,
        body: JSON.stringify({ model, messages: [{ role: 'user', content: 'Hello! Reply in one short sentence.' }], max_tokens: 100 }),
      });
      const text = await r.text();
      if (r.ok) {
        try {
          const d = JSON.parse(text);
          const reply = d.choices?.[0]?.message?.content || text;
          resultEl.innerHTML = '<span class="check-mark">&#10003;</span> <strong>Success</strong>\n\nModel: ' + escHtml(d.model || model) + '\nResponse: ' + escHtml(reply);
        } catch (_) {
          resultEl.textContent = text;
        }
      } else {
        resultEl.textContent = 'HTTP ' + r.status + ':\n' + text;
      }
    } catch (e) {
      resultEl.textContent = 'Connection failed: ' + e.message;
    }
    btnEl.disabled = false;
    btnEl.textContent = 'Send Test Request';
  };

  goToStep(1);
  window.__setPlatform(detectedPlatform);
  window.__setBackend('llamacpp');
  window.__setPersistPlatform(detectedPlatform);

  if (cloudCfg) {
    if ($('#cfg-server-url') && cloudCfg.serverUrl) $('#cfg-server-url').value = cloudCfg.serverUrl;
    if ($('#cfg-worker-secret') && cloudCfg.workerSecret) $('#cfg-worker-secret').value = cloudCfg.workerSecret;
    if ($('#test-api-key') && cloudCfg.apiKey) $('#test-api-key').value = cloudCfg.apiKey;
  } else {
    if ($('#cfg-server-url') && !$('#cfg-server-url').value) $('#cfg-server-url').value = getServerUrl();
  }
  updateConfigSnippet();

  document.addEventListener('input', e => {
    if (['cfg-server-url', 'cfg-worker-secret', 'cfg-worker-name', 'test-api-key', 'test-model'].includes(e.target.id)) {
      updateConfigSnippet();
    }
  });
})();
    "#;

    let step_labels = [
        "1. Platform",
        "2. Backend",
        "3. Model",
        "4. Worker",
        "5. Configure",
        "6. Connect",
        "7. Test",
        "8. Persist",
    ];

    let mut progress_html = String::from(r#"<div class="wizard-progress">"#);
    for (i, label) in step_labels.iter().enumerate() {
        let cls = if i == 0 { " active" } else { "" };
        let step_num = i + 1;
        let _ = write!(
            progress_html,
            r#"<div class="step-indicator{cls}" onclick="window.__wizGoTo({step_num})">{label}</div>"#
        );
    }
    progress_html.push_str("</div>");

    let steps_html = r##"
    <!-- Step 1: Platform -->
    <div class="wizard-step active" data-step="1">
      <div class="wizard-card">
        <h2><span class="step-num">1</span> Choose your GPU machine&rsquo;s OS</h2>
        <p>Select the operating system where your local inference backend runs:</p>
        <div class="platform-tabs">
          <div class="tab" data-platform="macos" onclick="window.__setPlatform('macos')">macOS</div>
          <div class="tab" data-platform="linux" onclick="window.__setPlatform('linux')">Linux</div>
          <div class="tab" data-platform="windows" onclick="window.__setPlatform('windows')">Windows</div>
        </div>
        <div class="platform-content" data-platform="macos">
          <div class="hint-box">
            <strong>Apple Silicon (M1&ndash;M4)</strong> &mdash; models run on unified memory with Metal out of the box (16 GB+ recommended).<br>
            <strong>Intel Macs</strong> &mdash; CPU inference works for smaller quantized models.
          </div>
        </div>
        <div class="platform-content" data-platform="linux">
          <div class="hint-box">
            <strong>NVIDIA GPU</strong> &mdash; confirm visibility with <code>nvidia-smi</code>.<br>
            <strong>AMD GPU (ROCm)</strong> &mdash; supported by llama.cpp, vLLM, and Ollama; confirm with <code>rocm-smi</code>.
          </div>
        </div>
        <div class="platform-content" data-platform="windows">
          <div class="hint-box">
            <strong>NVIDIA GPU</strong> &mdash; 8 GB+ VRAM recommended with up-to-date NVIDIA drivers.<br>
            <strong>AMD / CPU</strong> &mdash; supported via llama.cpp, LM Studio, or Ollama.
          </div>
        </div>
      </div>
      <div class="wizard-nav">
        <div></div>
        <button type="button" class="btn" onclick="window.__wizNext()">Next: Choose Backend &rarr;</button>
      </div>
    </div>

    <!-- Step 2: Backend -->
    <div class="wizard-step" data-step="2">
      <div class="wizard-card">
        <h2><span class="step-num">2</span> Local inference backend</h2>
        <p>ModelRelay forwards to any local OpenAI-compatible server. Pick the one you run:</p>
        <div class="platform-tabs backend-tabs">
          <div class="tab active" data-backend="llamacpp" onclick="window.__setBackend('llamacpp')">llama.cpp</div>
          <div class="tab" data-backend="lmstudio" onclick="window.__setBackend('lmstudio')">LM Studio</div>
          <div class="tab" data-backend="ollama" onclick="window.__setBackend('ollama')">Ollama</div>
          <div class="tab" data-backend="vllm" onclick="window.__setBackend('vllm')">vLLM</div>
        </div>

        <div class="backend-content active" data-backend="llamacpp">
          <p>Lightweight, fast GGUF inference on NVIDIA, AMD, and Apple Silicon. Serves on <code>http://127.0.0.1:8000</code>.</p>
          <div class="code-block">
            <button type="button" class="copy-btn" data-copy-target="install-llamacpp">Copy</button>
            <code id="install-llamacpp">git clone https://github.com/ggml-org/llama.cpp && cd llama.cpp
cmake -B build && cmake --build build --config Release -t llama-server</code>
          </div>
        </div>

        <div class="backend-content" data-backend="lmstudio">
          <p>Desktop GUI with a built-in OpenAI-compatible server on <code>http://127.0.0.1:1234</code>.</p>
          <p><a href="https://lmstudio.ai" target="_blank" rel="noopener" class="btn btn-secondary">Download LM Studio &rarr;</a></p>
        </div>

        <div class="backend-content" data-backend="ollama">
          <p>Simple CLI model manager serving an OpenAI-compatible API on <code>http://127.0.0.1:11434</code>.</p>
          <div class="code-block">
            <button type="button" class="copy-btn" data-copy-target="install-ollama">Copy</button>
            <code id="install-ollama">curl -fsSL https://ollama.com/install.sh | sh</code>
          </div>
        </div>

        <div class="backend-content" data-backend="vllm">
          <p>High-throughput continuous batching for NVIDIA/ROCm GPUs on <code>http://127.0.0.1:8000</code>.</p>
          <div class="code-block">
            <button type="button" class="copy-btn" data-copy-target="install-vllm">Copy</button>
            <code id="install-vllm">pip install vllm</code>
          </div>
        </div>

        <p style="font-size:0.86rem;">Already serving locally? <a href="#" onclick="event.preventDefault();window.__wizGoTo(4);">Skip to Download Worker &rarr;</a></p>
      </div>
      <div class="wizard-nav">
        <button type="button" class="btn btn-secondary" onclick="window.__wizPrev()">&larr; Back</button>
        <button type="button" class="btn" onclick="window.__wizNext()">Next: Start Model &rarr;</button>
      </div>
    </div>

    <!-- Step 3: Model -->
    <div class="wizard-step" data-step="3">
      <div class="wizard-card">
        <h2><span class="step-num">3</span> Start serving a model locally</h2>

        <div class="backend-content active" data-backend="llamacpp">
          <div class="code-block">
            <button type="button" class="copy-btn" data-copy-target="model-llamacpp">Copy</button>
            <code id="model-llamacpp">./build/bin/llama-server -m your-model.gguf --host 127.0.0.1 --port 8000 --jinja</code>
          </div>
        </div>
        <div class="backend-content" data-backend="lmstudio">
          <div class="hint-box">
            1. Download a model in the <strong>Discover</strong> tab.<br>
            2. Switch to <strong>Developer</strong>, load the model, and click <strong>Start Server</strong> on port <code>1234</code>.
          </div>
        </div>
        <div class="backend-content" data-backend="ollama">
          <div class="code-block">
            <button type="button" class="copy-btn" data-copy-target="model-ollama">Copy</button>
            <code id="model-ollama">ollama pull qwen3:32b
ollama serve</code>
          </div>
        </div>
        <div class="backend-content" data-backend="vllm">
          <div class="code-block">
            <button type="button" class="copy-btn" data-copy-target="model-vllm">Copy</button>
            <code id="model-vllm">vllm serve Qwen/Qwen3-32B --host 127.0.0.1 --port 8000</code>
          </div>
        </div>

        <div class="hint-box">
          <strong>Verify locally:</strong> <code>curl http://127.0.0.1:<span id="backend-port-hint">8000</span>/v1/models</code> should return a JSON list of loaded models.
        </div>
      </div>
      <div class="wizard-nav">
        <button type="button" class="btn btn-secondary" onclick="window.__wizPrev()">&larr; Back</button>
        <button type="button" class="btn" onclick="window.__wizNext()">Next: Download Worker &rarr;</button>
      </div>
    </div>

    <!-- Step 4: Download Worker -->
    <div class="wizard-step" data-step="4">
      <div class="wizard-card">
        <h2><span class="step-num">4</span> Download <code>modelrelay-worker</code></h2>
        <p>The worker daemon runs beside your local model server and opens an outbound WebSocket to the relay &mdash; zero inbound ports required.</p>
        <div class="code-block">
          <button type="button" class="copy-btn" data-copy-target="download-cmd">Copy</button>
          <code id="download-cmd">curl -L -o modelrelay-worker https://github.com/ericflo/modelrelay/releases/latest/download/modelrelay-worker-linux-amd64 &amp;&amp; chmod +x modelrelay-worker</code>
        </div>
        <p style="font-size:0.86rem;">Need arm64, Docker, or the native tray app? See <a href="/downloads">all downloads &rarr;</a></p>
      </div>
      <div class="wizard-nav">
        <button type="button" class="btn btn-secondary" onclick="window.__wizPrev()">&larr; Back</button>
        <button type="button" class="btn" onclick="window.__wizNext()">Next: Configure &rarr;</button>
      </div>
    </div>

    <!-- Step 5: Configure Worker -->
    <div class="wizard-step" data-step="5">
      <div class="wizard-card">
        <h2><span class="step-num">5</span> Create <code>config.toml</code></h2>
        <div class="config-input">
          <label for="cfg-server-url">Relay URL</label>
          <input id="cfg-server-url" type="text" placeholder="https://api.modelrelay.io">
        </div>
        <div class="config-input">
          <label for="cfg-worker-secret">API Key / Secret</label>
          <input id="cfg-worker-secret" type="text" placeholder="mr_live_...">
        </div>
        <div class="config-input">
          <label for="cfg-worker-name">Worker Name</label>
          <input id="cfg-worker-name" type="text" placeholder="my-gpu-box">
        </div>
        <div class="code-block">
          <button type="button" class="copy-btn" data-copy-target="config-toml">Copy</button>
          <code id="config-toml"></code>
        </div>
        <div class="hint-box">
          <strong><code>models = ["*"]</code></strong> polls your local backend&rsquo;s <code>/v1/models</code> automatically so any model you load or swap appears immediately on <code>api.modelrelay.io/v1/models</code> and in Pi.
        </div>
        <details style="margin-top:10px;">
          <summary style="color:var(--accent-bright);cursor:pointer;font-size:0.88rem;font-weight:600;">Prefer environment variables?</summary>
          <div class="code-block" style="margin-top:8px;">
            <button type="button" class="copy-btn" data-copy-target="config-env">Copy</button>
            <code id="config-env"></code>
          </div>
        </details>
      </div>
      <div class="wizard-nav">
        <button type="button" class="btn btn-secondary" onclick="window.__wizPrev()">&larr; Back</button>
        <button type="button" class="btn" onclick="window.__wizNext()">Next: Start &amp; Verify &rarr;</button>
      </div>
    </div>

    <!-- Step 6: Connect -->
    <div class="wizard-step" data-step="6">
      <div class="wizard-card">
        <h2><span class="step-num">6</span> Start the worker</h2>
        <div class="code-block">
          <button type="button" class="copy-btn" data-copy-target="run-cmd">Copy</button>
          <code id="run-cmd">./modelrelay-worker --config config.toml</code>
        </div>
        <div class="status-indicator" id="worker-status">
          <div class="pulse" id="worker-pulse"></div>
          <span id="worker-status-text">Waiting for worker to connect&hellip;</span>
        </div>
        <div id="troubleshoot-hints" style="display:none;" class="hint-box">
          <strong>Not appearing yet?</strong><br>
          &bull; Check the worker terminal logs for auth or backend connection messages.<br>
          &bull; Verify <code>proxy_url</code> and <code>worker_secret</code> match your account key.<br>
          &bull; Note: live worker detection polls your admin API; non-admin accounts can verify with <code>curl /v1/models</code> in the next step.
        </div>
        <button type="button" id="skip-detect" class="skip-link" onclick="window.__wizNext()">Continue to test inference &rarr;</button>
      </div>
      <div class="wizard-nav">
        <button type="button" class="btn btn-secondary" onclick="window.__wizPrev()">&larr; Back</button>
        <button type="button" class="btn" id="step6-next" onclick="window.__wizNext()">Next: Test Inference &rarr;</button>
      </div>
    </div>

    <!-- Step 7: Test -->
    <div class="wizard-step" data-step="7">
      <div class="wizard-card">
        <h2><span class="step-num">7</span> Test end-to-end inference</h2>
        <div class="config-input">
          <label for="test-model">Model ID</label>
          <input id="test-model" type="text" placeholder="e.g. qwen3.5-27b">
        </div>
        <div class="config-input">
          <label for="test-api-key">API Key</label>
          <input id="test-api-key" type="text" placeholder="mr_live_...">
        </div>
        <p style="margin:14px 0;"><button type="button" class="btn" id="test-btn" onclick="window.__testInference()">Send Test Request</button></p>
        <div id="test-result" class="test-result" style="display:none;"></div>
        <div class="code-block" style="margin-top:16px;">
          <button type="button" class="copy-btn" data-copy-target="curl-test">Copy</button>
          <code id="curl-test"></code>
        </div>
      </div>
      <div class="wizard-nav">
        <button type="button" class="btn btn-secondary" onclick="window.__wizPrev()">&larr; Back</button>
        <button type="button" class="btn" onclick="window.__wizNext()">Next: Run on Boot &rarr;</button>
      </div>
    </div>

    <!-- Step 8: Persist -->
    <div class="wizard-step" data-step="8">
      <div class="wizard-card">
        <h2><span class="step-num">8</span> Keep it running on boot</h2>
        <div class="platform-tabs persist-tabs">
          <div class="tab" data-platform="linux" onclick="window.__setPersistPlatform('linux')">Linux (systemd)</div>
          <div class="tab" data-platform="macos" onclick="window.__setPersistPlatform('macos')">macOS (launchd)</div>
          <div class="tab" data-platform="windows" onclick="window.__setPersistPlatform('windows')">Windows Service</div>
        </div>

        <div class="persist-content" data-platform="linux">
          <div class="code-block">
            <button type="button" class="copy-btn" data-copy-target="persist-linux">Copy</button>
            <code id="persist-linux">sudo install -m 755 modelrelay-worker /usr/local/bin/
sudo useradd --system --no-create-home modelrelay
sudo mkdir -p /etc/modelrelay
curl -fsSL https://raw.githubusercontent.com/ericflo/modelrelay/main/extras/modelrelay-worker%40.service \
  | sudo tee /etc/systemd/system/modelrelay-worker@.service &gt; /dev/null
sudo tee /etc/modelrelay/worker-gpu0.env &gt; /dev/null &lt;&lt;'EOF'
PROXY_URL=https://api.modelrelay.io
WORKER_SECRET=mr_live_your_api_key
BACKEND_URL=http://127.0.0.1:8000
MODELS=*
EOF
sudo systemctl daemon-reload &amp;&amp; sudo systemctl enable --now modelrelay-worker@gpu0</code>
          </div>
        </div>

        <div class="persist-content" data-platform="macos" style="display:none;">
          <p>For macOS desktops, the <a href="/downloads">ModelRelay Desktop app</a> includes built-in Launch at Login. For headless Macs, install a LaunchDaemon:</p>
          <div class="code-block">
            <button type="button" class="copy-btn" data-copy-target="persist-mac">Copy</button>
            <code id="persist-mac">sudo cp modelrelay-worker /usr/local/bin/
sudo mkdir -p /etc/modelrelay &amp;&amp; sudo cp config.toml /etc/modelrelay/config.toml
sudo launchctl load /Library/LaunchDaemons/io.modelrelay.worker.plist</code>
          </div>
        </div>

        <div class="persist-content" data-platform="windows" style="display:none;">
          <p>Run PowerShell as Administrator (or use the <a href="/downloads">Desktop tray app</a>):</p>
          <div class="code-block">
            <button type="button" class="copy-btn" data-copy-target="persist-win">Copy</button>
            <code id="persist-win">mkdir C:\ModelRelay
copy modelrelay-worker.exe C:\ModelRelay\
sc.exe create ModelRelayWorker binPath= "C:\ModelRelay\modelrelay-worker.exe" start= auto
Start-Service ModelRelayWorker</code>
          </div>
        </div>
      </div>

      <div class="wizard-card" style="text-align:center;border-color:rgba(52,211,153,0.3);">
        <h2 style="justify-content:center;color:var(--success);">Worker Ready</h2>
        <p style="margin-bottom:18px;">Now connect Pi Coding Agent as a provider so your new models appear automatically in <code>/model</code>.</p>
        <div style="display:flex;gap:10px;justify-content:center;flex-wrap:wrap;">
          <a href="/integrate#pi" class="btn">Set Up Pi Coding Agent &rarr;</a>
          <a href="/dashboard" class="btn btn-secondary">Open Dashboard</a>
        </div>
      </div>
      <div class="wizard-nav">
        <button type="button" class="btn btn-secondary" onclick="window.__wizPrev()">&larr; Back</button>
        <div></div>
      </div>
    </div>
    "##;

    let logged_in = cloud_config.is_some();
    let extra_css = format!("<style>{wizard_css}</style>");

    let body = format!(
        r#"<div class="wiz-head">
          <h1>Connect a GPU Worker</h1>
          <p>Connect any machine running llama.cpp, Ollama, vLLM, or LM Studio to your ModelRelay endpoint.</p>
        </div>
        <div class="desktop-shortcut">
          <div>
            <h2>Want zero terminal setup?</h2>
            <p>Install the ModelRelay Desktop tray app for macOS, Linux, or Windows &mdash; visual status, live model list, auto-reconnect, and signed auto-updates.</p>
          </div>
          <a href="/downloads" class="btn">Download Desktop App &rarr;</a>
        </div>
        {progress_html}
        {steps_html}"#
    );

    let cloud_cfg_script = cloud_config_script(cloud_config);
    let extra_body_end = format!("{cloud_cfg_script}<script>{wizard_js}</script>");

    page_shell_custom(
        "Worker Setup",
        "/setup",
        &body,
        logged_in,
        &extra_css,
        &extra_body_end,
    )
}
