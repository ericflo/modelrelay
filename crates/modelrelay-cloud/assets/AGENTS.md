# ModelRelay — Agent Setup & Integration Guide (`AGENTS.md`)

> **Canonical URL:** `https://modelrelay.io/AGENTS.md`  
> **Relay Endpoint:** `https://api.modelrelay.io`  
> **OAuth / Web Console:** `https://modelrelay.io` (`/integrate` for clients · `/setup` for workers · `/dashboard`)

This guide is for coding agents (**Claude Code**, **OpenAI Codex CLI**, **Pi Coding Agent**, etc.) helping a user set up either of two separate machines against `https://api.modelrelay.io`:

- **Workflow A — Serve an LLM (GPU machine):** Detect the running local LLM server (`llama-server`, Ollama, vLLM, LM Studio, SGLang, etc.), install `modelrelay-worker` pointing at `https://api.modelrelay.io`, authenticate **without pasting API keys into chat**, offer to configure permanent startup persistence for this OS/platform, and verify live `/v1/models` + streaming completions.
- **Workflow B — Connect a Client (laptop / workstation):** Configure **Pi** against `https://api.modelrelay.io` (if Pi isn't installed, ask whether to install Pi or which client to configure — **do not modify Claude Code or Codex CLI unless the user explicitly asks**) alongside existing commercial providers, **without asking for a URL or pasting an API key into chat**.

---

## 0. Security & Operating Rules (Read First!)

1. **NEVER ask the user to paste an API key into the LLM chat.** Always keep secrets out of the conversation transcript using one of the zero-paste methods in **Section 1** below:
   - **Local loopback OAuth** (spin up a temporary `127.0.0.1` callback server so the key goes straight from `https://modelrelay.io/oauth/token` to a `chmod 600` file on disk — **never print the raw key to stdout**),
   - **Pi's built-in `/login modelrelay`** (opens browser OAuth directly inside Pi), or
   - **Pre-filled copy-paste pages** at `https://modelrelay.io/integrate` (clients) or `https://modelrelay.io/setup` (workers), where the logged-in user can copy exact commands with their key pre-filled and run them directly in their own terminal outside the LLM.
2. **Assume `https://api.modelrelay.io` as the relay URL.** Do not ask the user for a relay URL or offer to set up a self-hosted `modelrelay-server` unless they explicitly ask for self-hosting (in which case see `https://modelrelay.io/docs/self-hosting`).
3. **Never overwrite existing commercial providers.** Merge into `~/.pi/agent/auth.json`, `~/.codex/config.toml`, or a separate `claude-local` shell helper so existing Anthropic/OpenAI/Google credentials stay untouched.
4. **Lock down every file that holds a key with `chmod 600`** and never print, log, or commit the raw `mr_live_...` secret.

---

## 1. Zero-Paste Authentication (Loopback OAuth)

ModelRelay supports RFC 7636 PKCE OAuth 2.0 with loopback `http://127.0.0.1:<port>/callback` redirects:
- Authorize: `GET https://modelrelay.io/oauth/authorize?callback_url=...&code_challenge=...&code_challenge_method=S256&state=...&key_label=...`
- Exchange: `POST https://modelrelay.io/oauth/token` with `{ code, code_verifier, code_challenge_method: "S256", callback_url }` → returns `{ "key": "mr_live_...", "api_base_url": "https://api.modelrelay.io/v1" }`

**Note:** On `https://api.modelrelay.io`, the same `mr_live_...` account key authenticates **both** `modelrelay-worker` (`worker_secret`) and API clients (`Authorization: Bearer ...`).

### Succinct Loopback OAuth Helper (Writes Key Straight to Disk, Never Prints Secret)

Run this zero-dependency Node.js command to save the user's key directly to `~/.config/modelrelay/api_key` (`chmod 600`) without exposing it to the LLM:

```bash
node -e '
const http = require("node:http"), crypto = require("node:crypto"), fs = require("node:fs"), path = require("node:path");
const verifier = crypto.randomBytes(32).toString("base64url");
const challenge = crypto.createHash("sha256").update(verifier).digest("base64url");
const state = crypto.randomBytes(16).toString("base64url");
const outFile = path.join(process.env.HOME, ".config", "modelrelay", "api_key");
const srv = http.createServer(async (req, res) => {
  const u = new URL(req.url, "http://127.0.0.1");
  if (u.pathname !== "/callback") return res.writeHead(404).end();
  if (u.searchParams.get("state") !== state || !u.searchParams.get("code")) {
    res.writeHead(400).end("OAuth failed or cancelled.");
    srv.close(); process.exit(1);
  }
  const r = await fetch("https://modelrelay.io/oauth/token", {
    method: "POST",
    headers: { "Content-Type": "application/json" },
    body: JSON.stringify({ code: u.searchParams.get("code"), code_verifier: verifier, code_challenge_method: "S256", callback_url: cb }),
  });
  const { key } = await r.json();
  if (!key) { res.writeHead(500).end("Token exchange failed."); srv.close(); process.exit(1); }
  fs.mkdirSync(path.dirname(outFile), { recursive: true, mode: 0o700 });
  fs.writeFileSync(outFile, key.trim() + "\n", { mode: 0o600 });
  fs.chmodSync(outFile, 0o600);
  res.writeHead(200, { "Content-Type": "text/html" }).end("<h2>Connected! You can close this tab.</h2>");
  console.log("Saved ModelRelay key (" + key.slice(0, 10) + "..." + key.slice(-4) + ") to " + outFile);
  srv.close();
});
let cb = "";
srv.listen(0, "127.0.0.1", () => {
  cb = `http://127.0.0.1:${srv.address().port}/callback`;
  const authUrl = "https://modelrelay.io/oauth/authorize?" + new URLSearchParams({
    callback_url: cb, code_challenge: challenge, code_challenge_method: "S256", state, key_label: "CLI Setup"
  });
  console.log("Open this URL in your browser to approve access:\n" + authUrl);
});
'
```

- **When a local browser is available:** Print the URL for the user (or open it via `open` / `xdg-open`) and wait for approval. Once saved to `~/.config/modelrelay/api_key`, reference it inside local scripts/commands without ever echoing its contents.
- **When on a headless/SSH machine (no local browser):** Skip loopback OAuth and send the user to **`https://modelrelay.io/setup`** (for GPU workers) or **`https://modelrelay.io/integrate`** (for clients) — when signed in, both pages pre-fill their live key into exact one-click copy-paste commands they can run directly in their own terminal.

---

## Workflow A — Serve an LLM (GPU Machine)

### A1. Detect the Local LLM Server

```bash
for port in 8000 8080 11434 1234 30000; do
  if out=$(curl -fsS --max-time 2 "http://127.0.0.1:${port}/v1/models" 2>/dev/null); then
    echo "=== Found backend on http://127.0.0.1:${port} ==="
    echo "$out" | head -c 1000
    echo ""
  fi
done
```

Common defaults: **llama.cpp (`llama-server`)** `:8000` / `:8080` · **vLLM** `:8000` · **Ollama** `:11434` · **LM Studio** `:1234` · **SGLang** `:30000`.
*(Note: `modelrelay-worker` handles both `http://127.0.0.1:<port>` and `.../v1` seamlessly, including root metadata paths `/props` and `/api/*`.)*

If no local server is running yet, ask the user which runtime/model they want started before proceeding.

### A2. Install `modelrelay-worker` (or Desktop App)

- **Desktop GUI (macOS / Linux / Windows):** `https://modelrelay.io/downloads` (tray worker with Launch at Login).
- **Standalone CLI binary** from `https://github.com/ericflo/modelrelay/releases/latest/download/`:
  - Linux x86_64: `modelrelay-worker-linux-amd64`
  - Linux arm64: `modelrelay-worker-linux-arm64`
  - macOS Apple Silicon / Intel: `modelrelay-worker-darwin-arm64` / `modelrelay-worker-darwin-amd64`
  - Windows x86_64: `modelrelay-worker-windows-amd64.exe`

```bash
# Example: Linux x86_64
sudo curl -fsSL https://github.com/ericflo/modelrelay/releases/latest/download/modelrelay-worker-linux-amd64 \
  -o /usr/local/bin/modelrelay-worker
sudo chmod 0755 /usr/local/bin/modelrelay-worker
```

### A3. Authenticate & Write `~/.config/modelrelay/worker.toml` (`chmod 600`)

Run the **Section 1 loopback OAuth helper** (or if headless, direct the user to `https://modelrelay.io/setup` to copy their pre-filled `worker.toml` command), then write `~/.config/modelrelay/worker.toml` reading the key from `~/.config/modelrelay/api_key` without printing it:

```bash
mkdir -p ~/.config/modelrelay
(
  umask 077
  KEY="$(tr -d '\r\n' < ~/.config/modelrelay/api_key)"
  cat > ~/.config/modelrelay/worker.toml <<EOF
proxy_url = "https://api.modelrelay.io"
worker_secret = "${KEY}"
worker_name = "$(hostname)"
provider = "local"
backend_url = "http://127.0.0.1:<DETECTED_PORT>"
models = ["*"]
max_concurrency = 4
models_poll_interval_secs = 30
log_level = "info"
EOF
)
chmod 600 ~/.config/modelrelay/worker.toml
```

> In TOML, the concurrency field is `max_concurrency` (`models = ["*"]` auto-discovers live `/v1/models` every 30s and preserves full model metadata).

### A4. Offer Permanent Startup Persistence for This Platform

Always **offer to configure `modelrelay-worker` to start permanently on boot** for whichever OS this GPU machine is running:

- **Linux (`systemd` user service — no root needed):**
```bash
mkdir -p ~/.config/systemd/user
cat > ~/.config/systemd/user/modelrelay-worker.service <<'EOF'
[Unit]
Description=ModelRelay GPU Worker
After=network-online.target
Wants=network-online.target

[Service]
Type=simple
ExecStart=/usr/local/bin/modelrelay-worker --config %h/.config/modelrelay/worker.toml
Restart=always
RestartSec=3
KillSignal=SIGTERM
TimeoutStopSec=20

[Install]
WantedBy=default.target
EOF
systemctl --user daemon-reload
systemctl --user enable --now modelrelay-worker
systemctl --user status modelrelay-worker --no-pager
```
*(For headless Linux servers, also offer `loginctl enable-linger "$USER"` so it runs before interactive login.)*

- **macOS (`launchd` LaunchAgent):**
Save to `~/Library/LaunchAgents/io.modelrelay.worker.plist` and run `launchctl bootstrap gui/$(id -u) ~/Library/LaunchAgents/io.modelrelay.worker.plist`:
```xml
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>Label</key><string>io.modelrelay.worker</string>
  <key>ProgramArguments</key>
  <array>
    <string>/usr/local/bin/modelrelay-worker</string>
    <string>--config</string>
    <string>/Users/YOU/.config/modelrelay/worker.toml</string>
  </array>
  <key>RunAtLoad</key><true/>
  <key>KeepAlive</key><true/>
  <key>StandardOutPath</key><string>/tmp/modelrelay-worker.log</string>
  <key>StandardErrorPath</key><string>/tmp/modelrelay-worker.err</string>
</dict>
</plist>
```

- **Windows (PowerShell Scheduled Task on startup, or Desktop Launch at Login):**
```powershell
$action = New-ScheduledTaskAction -Execute "$env:LOCALAPPDATA\modelrelay\modelrelay-worker.exe" `
  -Argument "--config `"$env:USERPROFILE\.config\modelrelay\worker.toml`""
$trigger = New-ScheduledTaskTrigger -AtStartup
$settings = New-ScheduledTaskSettingsSet -AllowStartIfOnBatteries -DontStopIfGoingOnBatteries -RestartCount 3 -RestartInterval (New-TimeSpan -Minutes 1)
Register-ScheduledTask -TaskName "ModelRelayWorker" -Action $action -Trigger $trigger -Settings $settings -RunLevel Highest -Force
Start-ScheduledTask -TaskName "ModelRelayWorker"
```
*(Or install ModelRelay Desktop from `https://modelrelay.io/download/desktop/windows` and enable **Launch at Login**.)*

- **Docker (`--restart unless-stopped`):**
```bash
docker run -d --name modelrelay-worker --restart unless-stopped \
  --network host \
  -v ~/.config/modelrelay/worker.toml:/etc/modelrelay/worker.toml:ro \
  ghcr.io/ericflo/modelrelay/modelrelay-worker:latest \
  --config /etc/modelrelay/worker.toml
```
*(On macOS/Windows Docker Desktop, omit `--network host` and set `backend_url = "http://host.docker.internal:<PORT>"`.)*

### A5. Verify End-to-End (Without Echoing the Key)

```bash
curl -fsS https://api.modelrelay.io/health
curl -fsS https://api.modelrelay.io/v1/models \
  -H "Authorization: Bearer $(tr -d '\r\n' < ~/.config/modelrelay/api_key)"
curl -N -fsS https://api.modelrelay.io/v1/chat/completions \
  -H "Content-Type: application/json" \
  -H "Authorization: Bearer $(tr -d '\r\n' < ~/.config/modelrelay/api_key)" \
  -d '{"model":"<MODEL_ID>","stream":true,"max_tokens":64,"messages":[{"role":"user","content":"Reply with OK."}]}'
```

---

## Workflow B — Connect a Client (Laptop / Workstation)

Check whether **Pi** is installed (`command -v pi`). If Pi is installed, configure Pi against `https://api.modelrelay.io` (Section B1). If Pi is **not** installed, ask the user whether they want to install Pi or which client they want configured instead. **Do not modify Claude Code or Codex CLI configs unless the user explicitly asks you to.** Always keep existing commercial providers intact and **never ask the user to paste an API key into chat**.

### B1. Pi Coding Agent (`@earendil-works/pi-coding-agent`) — Recommended

Install Pi if needed (`curl -fsSL https://pi.dev/install.sh | sh`), then install the hosted dynamic provider extension (**zero `models.json` config required** — auto-discovers live models, runtime context limits, vision, and thinking formats across **llama.cpp**, **vLLM**, **LM Studio**, **Ollama**, and custom OpenAI servers):

```bash
curl -fsSL https://modelrelay.io/pi/modelrelay.ts \
  --create-dirs -o ~/.pi/agent/extensions/modelrelay.ts
```

Authenticate Pi with **zero key pasting**:
- **Best (built-in OAuth):** Tell the user to run `/login modelrelay` inside `pi` — the extension opens `https://modelrelay.io/oauth/authorize` in their browser and stores the credential in `~/.pi/agent/auth.json` automatically!
- **Or via Section 1 loopback OAuth helper** (if authenticating multiple clients in one step): merge `~/.config/modelrelay/api_key` into `~/.pi/agent/auth.json` without touching existing providers:
```bash
node -e '
const fs = require("node:fs"), path = require("node:path");
const key = fs.readFileSync(path.join(process.env.HOME, ".config/modelrelay/api_key"), "utf8").trim();
const p = path.join(process.env.HOME, ".pi/agent/auth.json");
fs.mkdirSync(path.dirname(p), { recursive: true, mode: 0o700 });
const cur = fs.existsSync(p) ? JSON.parse(fs.readFileSync(p, "utf8")) : {};
cur.modelrelay = { type: "api_key", key };
fs.writeFileSync(p, JSON.stringify(cur, null, 2) + "\n", { mode: 0o600 });
fs.chmodSync(p, 0o600);
console.log("Merged modelrelay into " + p);
'
```
- **Or manual pre-filled copy-paste:** Point the user to `https://modelrelay.io/integrate#pi`.

Verify Pi discovery:
```bash
pi update --models
pi --list-models | grep -i modelrelay
```
Inside `pi`, run `/model` to select any relayed model and `/thinking` (`Shift+Tab`) to adjust effort.

### B2. OpenAI Codex CLI (Only If Explicitly Requested)

1. Run the **Section 1 loopback OAuth helper** (if `~/.config/modelrelay/api_key` does not already exist) — or direct the user to `https://modelrelay.io/integrate#clients` for a pre-filled 1-click copy-paste command.
2. Append a separate `modelrelay` provider to `~/.codex/config.toml` **without** changing their default `model_provider`:
```toml
[model_providers.modelrelay]
name = "ModelRelay"
base_url = "https://api.modelrelay.io/v1"
env_key = "MODELRELAY_API_KEY"
wire_api = "chat"
```
3. Load `MODELRELAY_API_KEY` from `~/.config/modelrelay/api_key` in the user's shell rc (`export MODELRELAY_API_KEY="$(cat ~/.config/modelrelay/api_key 2>/dev/null)"`), keeping the secret out of `~/.zshrc`/`~/.bashrc` and out of chat!
4. Run on demand while keeping plain `codex` on commercial OpenAI:
```bash
codex -c model_provider=modelrelay -m <MODEL_ID>
```

### B3. Claude Code (Only If Explicitly Requested)

Claude Code uses the Anthropic Messages API (`POST https://api.modelrelay.io/v1/messages`, forwarded transparently when the local backend supports `/v1/messages`, e.g. `llama-server`).

1. Ensure `~/.config/modelrelay/api_key` exists via the **Section 1 loopback OAuth helper** (or direct the user to `https://modelrelay.io/integrate#clients`).
2. Add a non-destructive `claude-local` helper function to `~/.zshrc` or `~/.bashrc` (keeps normal `claude` on commercial Anthropic and never embeds the raw key in shell rc files):
```bash
claude-local() {
  ANTHROPIC_BASE_URL="https://api.modelrelay.io" \
  ANTHROPIC_AUTH_TOKEN="$(tr -d '\r\n' < ~/.config/modelrelay/api_key)" \
  ANTHROPIC_MODEL="${1:-<MODEL_ID>}" \
  claude "${@:2}"
}
```

### B4. OpenAI / Anthropic SDKs & Browser Apps

- OpenAI SDKs: `base_url="https://api.modelrelay.io/v1"`, `Authorization: Bearer <key>`
- Browser apps (PKCE OAuth 2.0): authorize via `GET https://modelrelay.io/oauth/authorize` (`callback_url` must be `https://` or loopback `http://localhost|127.0.0.1|[::1]`), exchange via CORS-enabled `POST https://modelrelay.io/api/v1/auth/keys` or `POST https://modelrelay.io/oauth/token`.

---

## Verification Checklist & Troubleshooting

- [ ] No raw `mr_live_...` secret was asked for in chat or printed to stdout; key files are `chmod 600`.
- [ ] Existing commercial provider configs (`auth.json`, `config.toml`, `claude`) were preserved untouched.
- [ ] `GET https://api.modelrelay.io/v1/models` returns the expected live models.
- [ ] A streaming completion (`"stream": true`) responds cleanly end-to-end.
- [ ] (GPU machine) Offered permanent startup service (`systemd` / `launchd` / Windows Scheduled Task / Docker).

| Symptom | Cause & Fix |
|---|---|
| `401 Unauthorized` | Missing or revoked key — re-run `/login modelrelay` or local OAuth helper. |
| `503 Service Unavailable` | No connected worker advertises that model ID, or worker is using a different account key. Check `GET https://api.modelrelay.io/v1/models`. |
| `502 Bad Gateway` | Local LLM backend isn't reachable on `backend_url`. Verify `curl http://127.0.0.1:<port>/v1/models` on the GPU box. |
| Worker WS closes with `1008` | Invalid/revoked `worker_secret` in `worker.toml`. Re-authenticate via OAuth or `https://modelrelay.io/setup`. |

*(Looking to self-host the `modelrelay-server` binary itself? See `https://modelrelay.io/docs/self-hosting`.)*
