# Pi Coding Agent Setup

Configure ModelRelay as a zero-config dynamic **provider** in the [Pi coding agent](https://pi.dev) (`@earendil-works/pi-coding-agent`) with your API token stored cleanly in `~/.pi/agent/auth.json`.

ModelRelay preserves your local server's full `/v1/models` metadata end-to-end, so Pi automatically picks up:

- **Live models** via `GET https://api.modelrelay.io/v1/models` whenever you swap models on your GPU workers.
- **Context windows & output limits** (`context_length`, `context_window`, `max_model_len` for **vLLM**, `meta.n_ctx` & `/props` runtime `n_ctx` for **llama.cpp**, `loaded_instances[].config.context_length` / `loaded_context_length` for **LM Studio**, `num_ctx` / `model_info.*.context_length` for **Ollama**, plus `max_output_tokens` / `max_completion_tokens`).
- **Thinking levels & reasoning efforts** (`reasoning_efforts`, `supported_reasoning_efforts`, `thinkingLevelMap`, `reasoning`, `capabilities.reasoning`, **llama.cpp** `/props` `chat_template`, **vLLM** `enable_thinking` / `gpt-oss` / `DeepSeek-V3.1`, **LM Studio** `/api/v1/models` `capabilities.reasoning`, and **Ollama** `/api/show` `thinking`) plus vision modalities — with **zero `models.json` config required**.
- **Clean credential storage** in `~/.pi/agent/auth.json` (or interactively via `/login modelrelay`, or via a secret manager `!command`).

## 1. Install Pi

```bash
# macOS / Linux
curl -fsSL https://pi.dev/install.sh | sh
```

Windows PowerShell: `powershell -c "irm https://pi.dev/install.ps1 | iex"` — or via npm: `npm install -g --ignore-scripts @earendil-works/pi-coding-agent`.

## 2. Enable Live `/v1/models` Discovery (1 Command)

Pi's built-in `models.json` only supports static model lists and does not poll `/v1/models`. Install our small hosted provider file (`https://modelrelay.io/pi/modelrelay.ts`) with one command so Pi pulls live models, context windows, and reasoning efforts dynamically:

```bash
curl -fsSL https://modelrelay.io/pi/modelrelay.ts \
  --create-dirs -o ~/.pi/agent/extensions/modelrelay.ts
```

Self-hosting a custom relay URL? Keep the same file and set `export MODELRELAY_BASE_URL=https://relay.example.com/v1` (or set `providers.modelrelay.baseUrl` in `~/.pi/agent/models.json`).

## 3. Add Your API Token to `~/.pi/agent/auth.json`

Add a `"modelrelay"` entry to `~/.pi/agent/auth.json` (`chmod 600 ~/.pi/agent/auth.json`), or launch Pi and run `/login modelrelay`:

```json
{
  "modelrelay": {
    "type": "api_key",
    "key": "mr_live_your_api_key"
  }
}
```

## 4. Launch Pi & Pick a Model

```bash
pi   # starts interactive Pi and refreshes ModelRelay models automatically
# After the first interactive startup or /model open, CLI commands see the saved catalog too:
pi --list-models modelrelay
```

Inside Pi, run `/model` to refresh and select any model advertised by your workers (`Ctrl+S` saves a default), and run `/thinking` (or `Shift+Tab`) to cycle through the exact reasoning efforts advertised by your server.

> **Note (Pi 1.6+):** `pi update --models` only updates built-in providers and does not load `~/.pi/agent/extensions/modelrelay.ts`. Likewise, `pi --list-models` and `pi -p` read the last saved `~/.pi/agent/models-store.json` cache — open interactive `pi` or `/model` whenever you want to refresh your live ModelRelay catalog.

## Zero-Extension Static Alternative (`~/.pi/agent/models.json`)

If you only serve a fixed model ID and prefer zero extensions, skip Step 2 and declare it statically in `~/.pi/agent/models.json`:

```json
{
  "$schema": "https://pi.dev/schemas/models.schema.json",
  "providers": {
    "modelrelay": {
      "baseUrl": "https://api.modelrelay.io/v1",
      "api": "openai-completions",
      "apiKey": "$MODELRELAY_API_KEY",
      "models": [
        {
          "id": "qwen3.5-27b",
          "reasoning": true,
          "contextWindow": 131072,
          "maxTokens": 32768
        }
      ]
    }
  }
}
```
