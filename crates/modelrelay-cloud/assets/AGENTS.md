# ModelRelay — Agent Setup & Integration Guide (`AGENTS.md`)

> **Canonical URL:** `https://modelrelay.io/AGENTS.md`  
> **Source & Releases:** `https://github.com/ericflo/modelrelay` (MIT License)  
> **Hosted Relay Endpoint:** `https://api.modelrelay.io` (Dashboard: `https://modelrelay.io/dashboard`)

This document is written for coding agents (**Claude Code**, **OpenAI Codex CLI**, **Pi Coding Agent**, Cursor, etc.) helping a user set up **ModelRelay** end-to-end:

1. Putting a relay in front of one or more local GPU servers running an LLM (`llama-server`, Ollama, vLLM, LM Studio, SGLang, etc.) — either **self-hosted** (`modelrelay-server`) or via **hosted** `https://api.modelrelay.io`.
2. Installing and persisting `modelrelay-worker` on the GPU machine(s).
3. Configuring **Pi**, **Codex CLI**, **Claude Code**, OpenAI/Anthropic SDKs, browser OAuth apps, or `curl` to talk to models through ModelRelay — **alongside** any commercial provider already configured.

---

## 0. Agent Operating Rules

Follow these rules whenever bootstrapping ModelRelay for a user:

1. **Never overwrite existing commercial providers.** Merge ModelRelay into `~/.pi/agent/auth.json`, `~/.codex/config.toml`, or shell aliases (`claude-local`) so the user's existing Anthropic/OpenAI/Google credentials stay intact.
2. **Never print raw secrets to logs, chat transcripts, or git commits.** Generate secrets locally (`openssl rand -hex 32`), write them directly to `chmod 600` config/auth files, and mask them in summaries.
3. **Prefer loopback (`127.0.0.1`) for local inference backends and behind-proxy relays.** Workers connect *outbound* over WSS, so GPU machines never need inbound firewall ports.
4. **Ask before assuming missing infrastructure details.** If you don't know whether the user wants to self-host `modelrelay-server` (and on which host/domain) or use an existing relay/hosted key, ask concisely first.
5. **Verify every layer with live HTTP checks** before reporting success (Step 6 checklist).

---

## Architecture in One Paragraph

```text
[Clients: Pi / Codex / Claude Code / curl / Browser]
        │  HTTPS  /v1/models, /v1/chat/completions, /v1/responses, /v1/messages
        ▼
[modelrelay-server]  (self-hosted VPS/Docker or hosted https://api.modelrelay.io)
        ▲  Outbound WSS  /v1/worker/connect  (no inbound ports on GPU boxes!)
        │
[modelrelay-worker]  (or ModelRelay Desktop tray app on macOS/Linux/Windows)
        │  Loopback HTTP  http://127.0.0.1:<port>
        ▼
[Local LLM Server]   (llama-server, Ollama, vLLM, LM Studio, SGLang, etc.)
```

- Workers register supported models (or `--models "*"` to poll the local backend's `/v1/models` every 30s and publish changes dynamically).
- `modelrelay-server` routes by `"model"` in the JSON body (or `X-Model` header), queues/load-balances across workers, streams SSE unbuffered, and propagates client cancellation immediately to abort GPU generation.
- `modelrelay-server` strips browser hop headers (`Origin`, `Referer`, `Sec-Fetch-*`, `Cookie`) before forwarding to workers so third-party web apps work cleanly against localhost backends without per-origin allowlisting.

---

## Step 1 — Inspect the Local LLM Server (on the GPU Machine)

Before installing a worker, detect what local model server is already running and determine its **base URL**, **model IDs**, and **real context limit**:

```bash
# Probe common local OpenAI-compatible ports
for port in 8000 8080 11434 1234 30000; do
  if out=$(curl -fsS --max-time 2 "http://127.0.0.1:${port}/v1/models" 2>/dev/null); then
    echo "=== Found backend on http://127.0.0.1:${port} ==="
    echo "$out" | head -c 1000
    echo ""
  fi
done
```

Common defaults:
- **llama.cpp (`llama-server`)**: `http://127.0.0.1:8000` or `:8080` (check `-c` / `--ctx-size` or `meta.n_ctx` in `/v1/models`)
- **vLLM**: `http://127.0.0.1:8000` (check `--max-model-len`)
- **Ollama**: `http://127.0.0.1:11434` (exposes `/v1/models` and `/v1/chat/completions`)
- **LM Studio**: `http://127.0.0.1:1234`
- **SGLang**: `http://127.0.0.1:30000`

> Note: `modelrelay-worker` automatically deduplicates `/v1` if `backend_url` ends with `/v1`, so both `http://127.0.0.1:8000` and `http://127.0.0.1:8000/v1` work.

If no local LLM server is running yet, ask the user which runtime/model they want started on the GPU host before continuing.

---

## Step 2 — Choose & Set Up the Central Relay

### Option 2A — Self-Host `modelrelay-server` (MIT, Free, Full Control)

Run `modelrelay-server` on any small VPS, home router/NAS with a domain, Tailscale host, or container host reachable by both your GPU worker(s) and your client laptop(s).

#### Download Binary or Pull Docker Image

Pre-built binaries from GitHub Releases (`https://github.com/ericflo/modelrelay/releases/latest/download/...`):
- Linux x86_64: `modelrelay-server-linux-amd64`
- Linux arm64: `modelrelay-server-linux-arm64`
- macOS Apple Silicon: `modelrelay-server-darwin-arm64`
- macOS Intel: `modelrelay-server-darwin-amd64`
- Windows x86_64 / arm64: `modelrelay-server-windows-amd64.exe` / `modelrelay-server-windows-arm64.exe`

Docker image: `ghcr.io/ericflo/modelrelay/modelrelay-server:latest`

#### Generate Secrets

```bash
WORKER_SECRET="$(openssl rand -hex 32)"
ADMIN_TOKEN="$(openssl rand -hex 32)"
```

#### Environment Variables & Flags (`modelrelay-server`)

| Env Var | CLI Flag | Default | Purpose |
|---|---|---|---|
| `LISTEN_ADDR` | `--listen` | `127.0.0.1:8080` | Bind address (`127.0.0.1:8080` behind TLS proxy; `0.0.0.0:8080` in Docker). |
| `WORKER_SECRET` | `--worker-secret` | *optional if `DATABASE_URL` set* | Shared secret authenticating worker WebSocket handshakes (`x-worker-secret`). |
| `MODELRELAY_ADMIN_TOKEN` | `--admin-token` | *none* | Bearer token enabling `/admin/workers`, `/admin/stats`, and `/admin/keys`. |
| `MODELRELAY_REQUIRE_API_KEYS` | `--require-api-keys` | `false` | Set `true` to require Bearer / `x-api-key` auth on all `/v1/*` requests. |
| `DATABASE_URL` | `--database-url` | *none (in-memory)* | Optional PostgreSQL URL for persistent API keys across restarts. |
| `MODELRELAY_BOOTSTRAP_API_KEY` | `--bootstrap-api-key` | *none* | Optional existing native key to hash-import into PostgreSQL on startup. |
| `TRUST_PROXY_HEADERS` | `--trust-proxy-headers` | `false` | Set `true` behind a trusted TLS proxy (Caddy/nginx) for accurate rate-limit IPs. |
| `MAX_QUEUE_LEN` | `--max-queue-len` | `100` | Max queued requests (`0` = unlimited) before HTTP `503`. |
| `QUEUE_TIMEOUT_SECS` | `--queue-timeout` | `30` | Max seconds waiting in queue (`0` = no timeout) before HTTP `504`. |
| `REQUEST_TIMEOUT_SECS` | `--request-timeout` | `300` | Unary timeout / streaming chunk idle timeout in seconds. |
| `MAX_BODY_MB` | `--max-body-mb` | `64` | Max HTTP body size in MiB (supports multimodal/vision payloads). |
| `CORS_ALLOWED_ORIGINS` | `--cors-allowed-origins` | *public `*`, admin none* | Comma-separated origins or `*`. Leave unset for standard browser access on `/v1/*`. |
| `PROVIDER_NAME` | `--provider` | `local` | Default provider namespace (match `--provider` on workers). |
| `LOG_LEVEL` | `--log-level` | `info` | `trace`, `debug`, `info`, `warn`, or `error`. |

> **Persistence tip:** Without `DATABASE_URL`, client API keys created via `POST /admin/keys` are held in memory and reset when `modelrelay-server` restarts. For a zero-database single-user VPS setup, either run behind private Tailscale/VPN with `MODELRELAY_REQUIRE_API_KEYS=false`, or attach PostgreSQL (`DATABASE_URL`), or use a startup script / reverse-proxy bearer check. With PostgreSQL enabled, API keys persist permanently and can also authenticate workers directly.

#### Run via Docker

```bash
docker run -d --name modelrelay-server --restart unless-stopped \
  -p 127.0.0.1:8080:8080 \
  -e LISTEN_ADDR=0.0.0.0:8080 \
  -e WORKER_SECRET="$WORKER_SECRET" \
  -e MODELRELAY_ADMIN_TOKEN="$ADMIN_TOKEN" \
  -e MODELRELAY_REQUIRE_API_KEYS=true \
  -e TRUST_PROXY_HEADERS=true \
  ghcr.io/ericflo/modelrelay/modelrelay-server:latest
```

#### Or Run as a Linux `systemd` Service

```bash
sudo curl -fsSL https://github.com/ericflo/modelrelay/releases/latest/download/modelrelay-server-linux-amd64 \
  -o /usr/local/bin/modelrelay-server
sudo chmod 0755 /usr/local/bin/modelrelay-server
sudo install -d -m 0700 /etc/modelrelay
```

Write `/etc/modelrelay/server.env` (`chmod 600`):
```dotenv
LISTEN_ADDR=127.0.0.1:8080
WORKER_SECRET=<generated-worker-secret>
MODELRELAY_ADMIN_TOKEN=<generated-admin-token>
MODELRELAY_REQUIRE_API_KEYS=true
TRUST_PROXY_HEADERS=true
REQUEST_TIMEOUT_SECS=300
LOG_LEVEL=info
```

Write `/etc/systemd/system/modelrelay-server.service`:
```ini
[Unit]
Description=ModelRelay Proxy Server
After=network-online.target
Wants=network-online.target

[Service]
Type=simple
EnvironmentFile=/etc/modelrelay/server.env
ExecStart=/usr/local/bin/modelrelay-server
Restart=always
RestartSec=2
KillSignal=SIGTERM
TimeoutStopSec=20

[Install]
WantedBy=multi-user.target
```

```bash
sudo systemctl daemon-reload
sudo systemctl enable --now modelrelay-server
curl -fsS http://127.0.0.1:8080/health
```

#### Put TLS in Front (Caddy or nginx)

**Caddy** (recommended — automatic HTTPS, WebSockets, and unbuffered SSE out of the box):
```caddyfile
relay.example.com {
    reverse_proxy 127.0.0.1:8080
}
```

**nginx** (must forward WebSocket `Upgrade` on `/v1/worker/connect` and disable buffering on `/v1/`):
```nginx
server {
    listen 443 ssl http2;
    server_name relay.example.com;
    # ssl_certificate / ssl_certificate_key ...
    client_max_body_size 64m;

    location /v1/worker/connect {
        proxy_pass http://127.0.0.1:8080;
        proxy_http_version 1.1;
        proxy_set_header Upgrade $http_upgrade;
        proxy_set_header Connection "upgrade";
        proxy_set_header Host $host;
        proxy_set_header X-Forwarded-For $proxy_add_x_forwarded_for;
        proxy_read_timeout 86400s;
        proxy_send_timeout 86400s;
    }

    location /v1/ {
        proxy_pass http://127.0.0.1:8080;
        proxy_http_version 1.1;
        proxy_set_header Host $host;
        proxy_set_header X-Forwarded-For $proxy_add_x_forwarded_for;
        proxy_buffering off;
        proxy_cache off;
        proxy_read_timeout 300s;
    }

    location /health {
        proxy_pass http://127.0.0.1:8080;
    }
}
```

#### Provision a Client API Key (Self-Hosted)

```bash
curl -fsS -X POST http://127.0.0.1:8080/admin/keys \
  -H "Authorization: Bearer $ADMIN_TOKEN" \
  -H "Content-Type: application/json" \
  -d '{"name": "my-laptop"}'
# Returns JSON containing `secret` (save this as the client's MODELRELAY_API_KEY)
```

Admin endpoints (require `Authorization: Bearer $MODELRELAY_ADMIN_TOKEN`):
- `GET /admin/workers` — list connected workers, models, and live load
- `GET /admin/stats` — queue depth and active worker count
- `GET /admin/keys` — list API key metadata
- `POST /admin/keys` — create an API key (`{"name":"..."}`)
- `DELETE /admin/keys/{id}` — revoke an API key

---

### Option 2B — Hosted Relay (`https://api.modelrelay.io`)

If the user already has a hosted account on `https://modelrelay.io`:
- Relay URL: `https://api.modelrelay.io`
- Get the API key (`mr_live_...`) from `https://modelrelay.io/dashboard`
- **Important:** On hosted `api.modelrelay.io`, pass the user's `mr_live_...` API key as **both** the worker's `WORKER_SECRET` and the client's API key. Tenant-scoped workers route strictly to requests authenticated with the matching API key.

---

## Step 3 — Install & Run `modelrelay-worker` on the GPU Server

### Option 3A — Native Desktop Tray App (macOS / Linux / Windows Desktop)

Download from `https://modelrelay.io/downloads` (or `/download/desktop/macos`, `/download/desktop/linux`, `/download/desktop/windows`). Enter the Relay URL, Worker Secret / API key, Backend URL, and `models = "*"` in Settings.

### Option 3B — Headless `modelrelay-worker` Binary (Recommended for Servers)

Download the release binary matching the GPU host architecture:
- Linux x86_64: `https://github.com/ericflo/modelrelay/releases/latest/download/modelrelay-worker-linux-amd64`
- Linux arm64: `https://github.com/ericflo/modelrelay/releases/latest/download/modelrelay-worker-linux-arm64`
- macOS Apple Silicon: `https://github.com/ericflo/modelrelay/releases/latest/download/modelrelay-worker-darwin-arm64`
- macOS Intel: `https://github.com/ericflo/modelrelay/releases/latest/download/modelrelay-worker-darwin-amd64`
- Windows x86_64: `https://github.com/ericflo/modelrelay/releases/latest/download/modelrelay-worker-windows-amd64.exe`

```bash
# Example: Linux x86_64
sudo curl -fsSL https://github.com/ericflo/modelrelay/releases/latest/download/modelrelay-worker-linux-amd64 \
  -o /usr/local/bin/modelrelay-worker
sudo chmod 0755 /usr/local/bin/modelrelay-worker
```

#### Worker TOML Config Format (`--config <path>`)

Precedence: **CLI flags > environment variables > config file > defaults**.

Create `~/.config/modelrelay/worker.toml` (or `/etc/modelrelay/worker.toml`) with `chmod 600`:

```toml
proxy_url = "https://relay.example.com"     # or "https://api.modelrelay.io"
worker_secret = "<worker-secret-or-mr_live-key>"
worker_name = "gpu-box-4090"
provider = "local"                          # must match server PROVIDER_NAME (default: "local")
backend_url = "http://127.0.0.1:8000"       # local llama-server / Ollama / vLLM / LM Studio
models = ["*"]                              # "*" auto-discovers live /v1/models every 30s
max_concurrency = 4
models_poll_interval_secs = 30
log_level = "info"
```

> **Important field name:** In TOML, the concurrency field is `max_concurrency` (CLI: `--max-concurrency`, env: `MAX_CONCURRENCY`).

#### Worker Environment Variables & Flags

| Env Var | CLI Flag | Default | Notes |
|---|---|---|---|
| `PROXY_URL` | `--proxy-url` | `http://127.0.0.1:8080` | Relay base URL (`https://...` converts to `wss://.../v1/worker/connect`). |
| `WORKER_SECRET` | `--worker-secret` | *required* | Server `WORKER_SECRET` (self-hosted) or `mr_live_...` API key (hosted). |
| `BACKEND_URL` | `--backend-url` | `http://127.0.0.1:8000` | Local inference backend URL. |
| `MODELS` | `--models` | `default` | Comma-separated model IDs, or `*` for automatic `/v1/models` discovery. |
| `WORKER_NAME` | `--worker-name` | `worker` | Label shown in logs and dashboards. |
| `PROVIDER_NAME` | `--provider` | `local` | Provider group name on the relay. |
| `MAX_CONCURRENCY` | `--max-concurrency` | `1` | Max simultaneous requests sent to the local backend. |
| `MODELS_POLL_INTERVAL_SECS` | `--models-poll-interval` | `30` | Re-poll interval for `*` discovery (`0` = disabled). |
| `ENDPOINT_PREFIXES` | `--endpoint-prefixes` | *(all)* | Optional comma-separated allowed paths, e.g. `/v1/chat,/v1/messages`. |
| `LOG_LEVEL` | `--log-level` | `info` | Tracing filter (overridden by `RUST_LOG`). |

#### Persist on Linux (`systemd` user service — no root needed)

```bash
mkdir -p ~/.config/modelrelay ~/.config/systemd/user
chmod 600 ~/.config/modelrelay/worker.toml

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

*(For headless servers where the user isn't logged in, run `loginctl enable-linger "$USER"` or install as a system unit under `/etc/systemd/system/`.)*

#### Persist on macOS (`launchd`)

Save to `~/Library/LaunchAgents/io.modelrelay.worker.plist` and load with `launchctl bootstrap gui/$(id -u) ~/Library/LaunchAgents/io.modelrelay.worker.plist`:

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

#### Or Run via Docker on the GPU Machine

```bash
# Linux (reaches loopback LLM directly via --network host)
docker run -d --name modelrelay-worker --restart unless-stopped \
  --network host \
  -e PROXY_URL="https://relay.example.com" \
  -e WORKER_SECRET="$WORKER_SECRET" \
  -e WORKER_NAME="$(hostname)" \
  -e BACKEND_URL="http://127.0.0.1:8000" \
  -e MODELS="*" \
  -e MAX_CONCURRENCY=4 \
  ghcr.io/ericflo/modelrelay/modelrelay-worker:latest
```
*(On macOS/Windows Docker Desktop, omit `--network host` and set `BACKEND_URL=http://host.docker.internal:8000`.)*

---

## Step 4 — Verify the Relay End-to-End with `curl`

Set `RELAY_URL` (e.g. `https://relay.example.com` or `https://api.modelrelay.io`) and `MODELRELAY_API_KEY`:

```bash
# 1. Relay health
curl -fsS "${RELAY_URL}/health"

# 2. List live models advertised by your worker(s)
curl -fsS "${RELAY_URL}/v1/models" \
  -H "Authorization: Bearer ${MODELRELAY_API_KEY}"

# 3. Streaming Chat Completion (replace MODEL_ID with an ID returned above)
curl -N -fsS "${RELAY_URL}/v1/chat/completions" \
  -H "Content-Type: application/json" \
  -H "Authorization: Bearer ${MODELRELAY_API_KEY}" \
  -d '{"model":"MODEL_ID","stream":true,"max_tokens":64,"messages":[{"role":"user","content":"Reply with OK."}]}'
```

Also supported (whenever the underlying local backend implements the route):
- `POST ${RELAY_URL}/v1/responses` (`Authorization: Bearer ...`)
- `POST ${RELAY_URL}/v1/messages` (`x-api-key: ...` or `Authorization: Bearer ...`, plus `anthropic-version: 2023-06-01`)
- Routing headers: `X-Model` (fallback if `model` omitted from body) and `X-Provider` (or `"provider": "name"` in body) for targeting a specific worker group.

---

## Step 5 — Configure Coding Agents & Clients (Alongside Commercial Providers)

### 5A. Pi Coding Agent (`@earendil-works/pi-coding-agent`) — Recommended

Install Pi if not already present:
```bash
curl -fsSL https://pi.dev/install.sh | sh
# or: npm install -g --ignore-scripts @earendil-works/pi-coding-agent
```

#### 1. Create `~/.pi/agent/extensions/modelrelay.ts`

Register ModelRelay as a **dynamic provider** (NOT a static model list). Set `DEFAULT_BASE_URL` to `"https://api.modelrelay.io/v1"` or your self-hosted `"https://relay.example.com/v1"`, and set `DEFAULT_CONTEXT_WINDOW` to match your GPU backend's actual `-c` / `--max-model-len`:

```ts
import type { ExtensionAPI } from "@earendil-works/pi-coding-agent";

const DEFAULT_BASE_URL =
  process.env.MODELRELAY_BASE_URL || "https://api.modelrelay.io/v1";
const DEFAULT_CONTEXT_WINDOW =
  Number(process.env.MODELRELAY_CONTEXT_WINDOW) || 131072;
const DEFAULT_MAX_TOKENS =
  Number(process.env.MODELRELAY_MAX_TOKENS) || 32768;

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
}
```

#### 2. Merge Token into `~/.pi/agent/auth.json` (Preserving Existing Providers!)

Do **not** replace `~/.pi/agent/auth.json` if it already exists — merge the `"modelrelay"` entry into the existing JSON object and lock down permissions:

```bash
mkdir -p ~/.pi/agent
node -e '
const fs = require("fs");
const p = process.env.HOME + "/.pi/agent/auth.json";
const cur = fs.existsSync(p) ? JSON.parse(fs.readFileSync(p, "utf8")) : {};
cur.modelrelay = { type: "api_key", key: process.env.MODELRELAY_API_KEY };
fs.writeFileSync(p, JSON.stringify(cur, null, 2) + "\n", { mode: 0o600 });
fs.chmodSync(p, 0o600);
'
```
*(Alternatively, the user can run `/login modelrelay` inside Pi, or use `"key": "!security find-generic-password -ws 'modelrelay'"` for keychain resolution.)*

#### 3. Optional Per-Model Overrides (`~/.pi/agent/models.json`)

For heterogeneous GPU fleets (e.g., Qwen3 chat templates, DeepSeek R1/V3, GLM, or different `-c` sizes per model), layer `modelOverrides` cleanly without putting keys in `models.json`:

```json
{
  "$schema": "https://pi.dev/schemas/models.schema.json",
  "providers": {
    "modelrelay": {
      "baseUrl": "https://relay.example.com/v1",
      "modelOverrides": {
        "qwen3.5-27b": {
          "reasoning": true,
          "contextWindow": 131072,
          "maxTokens": 32768,
          "compat": {
            "thinkingFormat": "qwen-chat-template",
            "thinkingTokenBudgetField": "thinking_budget_tokens"
          }
        }
      }
    }
  }
}
```

#### 4. Verify Pi Discovery

```bash
pi update --models
pi --list-models | grep -i modelrelay
```
Inside `pi`, run `/model` to switch between ModelRelay and existing commercial providers, and `/thinking` (or `Shift+Tab`) to adjust thinking effort (`off`, `low`, `medium`, `high`).

---

### 5B. OpenAI Codex CLI (Alongside Commercial OpenAI)

Add a separate `modelrelay` provider to `~/.codex/config.toml` **without** changing the user's default `model_provider` unless they ask you to:

```toml
[model_providers.modelrelay]
name = "ModelRelay"
base_url = "https://relay.example.com/v1"   # or "https://api.modelrelay.io/v1"
env_key = "MODELRELAY_API_KEY"
wire_api = "chat"                           # use "responses" if your backend implements /v1/responses
```

Export `MODELRELAY_API_KEY` in the user's shell profile (or pass inline), then invoke Codex against ModelRelay on demand while keeping plain `codex` on commercial OpenAI:

```bash
codex -c model_provider=modelrelay -m <model-id>
```

---

### 5C. Claude Code (Alongside Commercial Anthropic)

Claude Code speaks the **Anthropic Messages API** (`POST /v1/messages`). ModelRelay forwards `/v1/messages` transparently — make sure the local backend supports Anthropic Messages (for example, recent `llama-server` builds support `/v1/messages` natively).

To keep the user's normal `claude` command on commercial Anthropic, add a non-destructive shell helper function (in `~/.zshrc` or `~/.bashrc`) that points a separate command `claude-local` at ModelRelay:

```bash
# Add to ~/.zshrc or ~/.bashrc (keeps `claude` using commercial Anthropic)
claude-local() {
  ANTHROPIC_BASE_URL="https://relay.example.com" \
  ANTHROPIC_AUTH_TOKEN="${MODELRELAY_API_KEY}" \
  ANTHROPIC_MODEL="${1:-MODEL_ID}" \
  claude "${@:2}"
}
```
*(Verify first with `curl -fsS "${RELAY_URL}/v1/messages" -H "Content-Type: application/json" -H "x-api-key: ${MODELRELAY_API_KEY}" -H "anthropic-version: 2023-06-01" -d '{"model":"MODEL_ID","max_tokens":32,"messages":[{"role":"user","content":"Hi"}]}'` that the local backend handles `/v1/messages`.)*

---

### 5D. OpenAI & Anthropic SDKs / Generic Clients

```bash
# OpenAI-compatible clients & scripts
export OPENAI_BASE_URL="https://relay.example.com/v1"
export OPENAI_API_KEY="${MODELRELAY_API_KEY}"
```

```python
from openai import OpenAI

client = OpenAI(
    base_url="https://relay.example.com/v1",  # or "https://api.modelrelay.io/v1"
    api_key=os.environ["MODELRELAY_API_KEY"],
)
```

---

### 5E. Browser & Web Apps (OAuth 2.0 PKCE on `modelrelay.io`)

Third-party browser apps (such as `https://chat.neuralcolumn.com`, `folkreel.neuralcolumn.com`, `trellis.neuralcolumn.com`, `explainer-studio.neuralcolumn.com`, `riff.neuralcolumn.com`) can connect hosted ModelRelay accounts without users copy-pasting API keys:

1. Generate a 32-byte random `code_verifier`, compute `code_challenge = BASE64URL(SHA256(code_verifier))`, and redirect to:
   ```text
   https://modelrelay.io/oauth/authorize?callback_url=https://your.app/&code_challenge=...&code_challenge_method=S256&state=...&key_label=Your+App
   ```
   (`redirect_uri` and `client_name` aliases are also accepted; `callback_url` must be `https://` or loopback `http://localhost|127.0.0.1|[::1]`.)
2. On approval, ModelRelay redirects to `callback_url?code=mrc_...&state=...` (single-use, 5-minute TTL; `?error=access_denied` on cancel).
3. Exchange directly from the browser via CORS-enabled `POST https://modelrelay.io/api/v1/auth/keys` (JSON) or `POST https://modelrelay.io/oauth/token` (JSON or form):
   ```json
   { "code": "mrc_...", "code_verifier": "...", "code_challenge_method": "S256" }
   ```
   Response:
   ```json
   { "key": "mr_live_...", "access_token": "mr_live_...", "token_type": "Bearer", "api_base_url": "https://api.modelrelay.io/v1", "label": "Your App" }
   ```
4. Discover live models via `GET https://api.modelrelay.io/v1/models` (`Authorization: Bearer <key>`) and stream completions via `POST https://api.modelrelay.io/v1/chat/completions`.

---

## Step 6 — Final Verification Checklist & Troubleshooting

Before reporting completion to the user, verify every applicable item:

- [ ] Local LLM backend responds on `http://127.0.0.1:<port>/v1/models` with expected model IDs.
- [ ] Relay responds `200 OK` on `GET ${RELAY_URL}/health`.
- [ ] `modelrelay-worker` is running persistently (`systemd`/`launchd`/Docker) and connected (`RegisterAck` in logs).
- [ ] `GET ${RELAY_URL}/v1/models` with `Authorization: Bearer <key>` returns the GPU worker's live model IDs.
- [ ] Streaming `POST ${RELAY_URL}/v1/chat/completions` (`"stream": true`) returns tokens end-to-end.
- [ ] Configured client(s) (Pi / Codex / Claude Code) list or invoke the ModelRelay model cleanly.
- [ ] Existing commercial provider keys/configs were **not** overwritten, and secret files are `chmod 600`.

### Common Failure Modes

| Symptom | Cause & Fix |
|---|---|
| `401 Unauthorized` | Wrong/revoked API key, or self-hosted server restarted without `DATABASE_URL` (clearing in-memory keys). Recreate key or attach PostgreSQL. |
| `503 Service Unavailable` | No connected worker advertises the requested `model` ID, or `PROVIDER_NAME` mismatch between server and worker, or tenant key mismatch on hosted `api.modelrelay.io`. Check `GET /v1/models`. |
| `502 Bad Gateway` | Worker cannot reach `BACKEND_URL` locally, or backend crashed/returned malformed response. Test `curl http://127.0.0.1:<port>/v1/models` on the GPU host. |
| Streaming arrives all at once | Reverse proxy is buffering responses. Add `proxy_buffering off;` in nginx or switch to Caddy. |
| Worker WS closes with code `1008` | Invalid `WORKER_SECRET` (or revoked API key on hosted relay). Verify secret matches server/dashboard. |
| Pi `/thinking` only shows `off` | Ensure `reasoning: true` and `thinkingLevelMap` are set in `~/.pi/agent/extensions/modelrelay.ts`, then run `pi update --models`. |
| Pi hits context overflow early | Lower `DEFAULT_CONTEXT_WINDOW` (or `modelOverrides.<id>.contextWindow`) to match your backend's actual `-c` / `--max-model-len`. |
