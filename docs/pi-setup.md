# Pi Coding Agent Setup

Configure ModelRelay as a native **provider** in the [Pi coding agent](https://pi.dev) (`@earendil-works/pi-coding-agent`) with your API token stored cleanly in `~/.pi/agent/auth.json` — plus full thinking-level, reasoning-effort, and context-window conveyance.

Registering ModelRelay as a provider extension instead of hardcoding models in `models.json` gives you:

- **Automatic model discovery** via `GET https://api.modelrelay.io/v1/models` whenever you swap models on your GPU workers.
- **Proper thinking levels & context limits** — conveys `reasoning`, `thinkingLevelMap`, `compat`, `contextWindow`, and `maxTokens` so `/thinking`, `Shift+Tab`, context meters, and auto-compaction work accurately.
- **Clean credential storage** in `~/.pi/agent/auth.json` (or interactively via `/login modelrelay`, or via a secret manager `!command`).
- **Instant offline startup** via Pi's persisted model catalog cache (preserving all thinking & compat metadata).

## 1. Install Pi

```bash
# macOS / Linux
curl -fsSL https://pi.dev/install.sh | sh
```

Windows PowerShell:

```powershell
powershell -c "irm https://pi.dev/install.ps1 | iex"
```

Or via npm (Node.js 22.19+):

```bash
npm install -g --ignore-scripts @earendil-works/pi-coding-agent
```

## 2. Register ModelRelay as a Provider

Create `~/.pi/agent/extensions/modelrelay.ts`:

```typescript
import type { ExtensionAPI } from "@earendil-works/pi-coding-agent";

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
}
```

## 3. Add Your API Token to `~/.pi/agent/auth.json`

Add a `"modelrelay"` entry to `~/.pi/agent/auth.json` (and lock permissions with `chmod 600 ~/.pi/agent/auth.json`):

```json
{
  "modelrelay": {
    "type": "api_key",
    "key": "mr_live_your_api_key"
  }
}
```

Alternatively, launch Pi and run `/login modelrelay` to paste your API key interactively, or load it from a keychain command using `"key": "!security find-generic-password -ws 'modelrelay'"`.

## 4. Verify, Pick a Model & Set Thinking Level

```bash
pi update --models
pi --list-models
```

Inside Pi, run `/model` to select any model advertised by your workers (`Ctrl+S` saves a default), and run `/thinking` (or `Shift+Tab`) to toggle `off`, `low`, `medium`, and `high`.

## Thinking Levels & Reasoning Effort

Pi only offers `/thinking` levels when `reasoning: true`, and translates each level into wire parameters via `thinkingLevelMap` and `compat`:

- **`thinkingLevelMap.off: "none"`** — Explicitly sends `"reasoning_effort": "none"` when `/thinking off` is selected, turning thinking off on `llama-server` and vLLM instead of leaving backend defaults enabled.
- **`thinkingLevelMap.minimal: null`** — Hides `minimal` so strict Jinja chat templates that only accept `low` / `medium` / `high` do not return HTTP 400.
- **`xhigh` / `max`** — Opt-in in Pi; only offered when mapped to a string in `thinkingLevelMap`.
- **`compat.thinkingTokenBudgetField`** — Caps reasoning tokens per thinking level when reasoning and final answer share `max_tokens` (`"thinking_budget_tokens"` for `llama.cpp`, `"thinking_token_budget"` for vLLM, `"thinking_budget"` for Qwen/SGLang).

| Backend / Model Family | `compat.thinkingFormat` | Wire Parameters Sent by Pi |
|---|---|---|
| llama.cpp / vLLM / Ollama / gpt-oss | `"openai"` (default) | `reasoning_effort: "none" \| "low" \| "medium" \| "high"` (+ optional `thinking_budget_tokens` / `thinking_token_budget`) |
| Qwen3 / Qwen3.5 chat templates | `"qwen-chat-template"` | `chat_template_kwargs: { enable_thinking, preserve_thinking: true }` (+ optional `thinking_budget_tokens`) |
| DeepSeek R1 / V3 / V4 | `"deepseek"` | `thinking: { type: "enabled" \| "disabled" }` + `reasoning_effort` + `requiresReasoningContentOnAssistantMessages: true` |
| GLM (ZAI) | `"zai"` | `thinking: { type: "enabled", clear_thinking: false }` / `{ type: "disabled" }` |

## Context Window, Output Limits & Per-Model Overrides

Pi uses `contextWindow` and `maxTokens` to drive its context gauge, dynamically clamp `max_tokens` to remaining room, and trigger auto-compaction before overflowing your GPU's KV cache. Keeping `compat.supportsUsageInStreaming: true` ensures streamed turns report accurate token counts.

Match `MODELRELAY_CONTEXT_WINDOW` (or `DEFAULT_CONTEXT_WINDOW` in `modelrelay.ts`) to your worker's `-c` / `--max-model-len`. For heterogeneous GPU fleets, layer per-model overrides in `~/.pi/agent/models.json` without putting secrets into `models.json`:

```json
{
  "$schema": "https://pi.dev/schemas/models.schema.json",
  "providers": {
    "modelrelay": {
      "baseUrl": "https://api.modelrelay.io/v1",
      "modelOverrides": {
        "qwen3.5-27b": {
          "reasoning": true,
          "input": ["text", "image"],
          "contextWindow": 131072,
          "maxTokens": 32768,
          "thinkingLevelMap": {
            "off": "none",
            "minimal": null,
            "low": "low",
            "medium": "medium",
            "high": "high"
          },
          "compat": {
            "thinkingFormat": "qwen-chat-template",
            "thinkingTokenBudgetField": "thinking_budget_tokens"
          },
          "samplingParamsByThinkingLevel": {
            "off": { "temperature": 0.7, "top_p": 0.8 },
            "high": { "temperature": 1.0, "top_p": 0.95 }
          }
        },
        "llama-3.3-70b": {
          "reasoning": false,
          "contextWindow": 65536,
          "maxTokens": 16384
        }
      }
    }
  }
}
```
