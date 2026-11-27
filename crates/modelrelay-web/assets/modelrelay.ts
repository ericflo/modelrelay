import { createHash, randomBytes } from "node:crypto";
import { createServer } from "node:http";
import type { ExtensionAPI } from "@earendil-works/pi-coding-agent";

const DEFAULT_BASE_URL =
  process.env.MODELRELAY_BASE_URL || "https://api.modelrelay.io/v1";
const DEFAULT_SITE_URL =
  process.env.MODELRELAY_SITE_URL || "https://modelrelay.io";
const EFFORT_LEVELS = ["minimal", "low", "medium", "high", "xhigh", "max"] as const;
const LLAMA_ROUTER_STATUSES = new Set([
  "loaded",
  "sleeping",
  "unloaded",
  "loading",
  "downloading",
]);

type ThinkingFormat =
  | "ant-ling"
  | "baseten"
  | "chat-template"
  | "deepseek"
  | "openai"
  | "openrouter"
  | "qwen"
  | "qwen-chat-template"
  | "string-thinking"
  | "together"
  | "zai";

type RelayModel = {
  id: string;
  name?: string;
  owned_by?: string;
  root?: string | null;
  source?: string;
  type?: string;
  arch?: string;
  publisher?: string;
  state?: string;
  status?: { value?: string; failed?: boolean; args?: string[] };
  contextWindow?: number;
  context_window?: number;
  context_length?: number;
  loaded_context_length?: number;
  max_model_len?: number;
  max_context_length?: number;
  n_ctx?: number;
  maxTokens?: number;
  max_output_tokens?: number;
  max_completion_tokens?: number;
  max_tokens?: number;
  reasoning?: boolean | { supported?: boolean; efforts?: string[]; allowed_options?: unknown[] };
  reasoning_efforts?: string[];
  supported_reasoning_efforts?: string[];
  reasoning_effort_levels?: string[];
  reasoning_effort?: string[];
  thinking_levels?: string[];
  thinkingLevelMap?: Record<string, string | null>;
  thinking_level_map?: Record<string, string | null>;
  thinking_format?: ThinkingFormat;
  supported_parameters?: string[];
  capabilities?:
    | string[]
    | {
        reasoning?: boolean | { supported?: boolean; efforts?: string[]; allowed_options?: unknown[] };
        thinking?: boolean;
        vision?: boolean;
        reasoning_efforts?: string[];
      };
  modalities?: { vision?: boolean; audio?: boolean };
  vision?: boolean;
  multimodal?: boolean;
  meta?: { n_ctx?: number; n_ctx_train?: number; multimodal?: boolean };
  top_provider?: { context_length?: number; max_completion_tokens?: number };
  architecture?: { input_modalities?: string[]; output_modalities?: string[] };
  compat?: Record<string, unknown>;
  samplingParams?: Record<string, unknown>;
  samplingParamsByThinkingLevel?: Record<string, Record<string, unknown>>;
};

type LlamaProps = {
  models_autoload?: boolean;
  chat_template?: string;
  default_generation_settings?: { n_ctx?: number };
  modalities?: { vision?: boolean; audio?: boolean };
};

type LmStudioModelMeta = {
  id: string;
  displayName?: string;
  arch?: string;
  isEmbedding: boolean;
  isLoaded: boolean;
  contextWindow?: number;
  vision?: boolean;
  reasoning?: boolean | { supported?: boolean; efforts?: string[]; allowed_options?: unknown[] };
};

type OllamaShowMeta = {
  isEmbedding: boolean;
  contextWindow?: number;
  vision?: boolean;
  thinking?: boolean;
  thinkingValues?: unknown[];
};

type ThinkingConfig = {
  reasoning: boolean;
  thinkingLevelMap?: Record<string, string | null>;
  thinkingFormat?: ThinkingFormat;
  supportsReasoningEffort: boolean;
  chatTemplateKwargs?: Record<string, unknown>;
};

const BINARY_THINKING_MAP: Record<string, string | null> = {
  off: "off",
  minimal: null,
  low: null,
  medium: "medium",
  high: null,
  xhigh: null,
};

const OPENAI_EFFORT_MAP: Record<string, string | null> = {
  off: null,
  minimal: null,
  low: "low",
  medium: "medium",
  high: "high",
  xhigh: null,
};

function positiveInt(v: unknown): number | undefined {
  return typeof v === "number" && Number.isFinite(v) && v > 0 ? Math.floor(v) : undefined;
}

function capsObject(m: RelayModel) {
  return m.capabilities && !Array.isArray(m.capabilities) ? m.capabilities : undefined;
}

function isLlamaRouterModel(m: RelayModel): boolean {
  return typeof m.status?.value === "string" && LLAMA_ROUTER_STATUSES.has(m.status.value);
}

function isLlamaCppModel(m: RelayModel): boolean {
  return (
    m.owned_by?.toLowerCase() === "llamacpp" ||
    isLlamaRouterModel(m) ||
    typeof m.meta?.n_ctx === "number" ||
    typeof m.meta?.n_ctx_train === "number"
  );
}

function isVllmModel(m: RelayModel): boolean {
  return m.owned_by?.toLowerCase() === "vllm" || typeof m.max_model_len === "number";
}

function isLmStudioModel(m: RelayModel): boolean {
  const owner = m.owned_by?.toLowerCase();
  return (
    owner === "organization_owner" ||
    owner === "organization-owner" ||
    owner === "lmstudio" ||
    typeof m.loaded_context_length === "number" ||
    typeof m.publisher === "string"
  );
}

function isOllamaModel(m: RelayModel): boolean {
  const owner = m.owned_by?.toLowerCase();
  return owner === "library" || (!isLlamaCppModel(m) && !isVllmModel(m) && !isLmStudioModel(m) && m.id.includes(":"));
}

function isDecisionOnlyModel(m: RelayModel): boolean {
  const out = m.architecture?.output_modalities;
  return Array.isArray(out) && out.includes("decisions") && !out.includes("text");
}

function isEmbeddingModel(m: RelayModel): boolean {
  const t = m.type?.toLowerCase();
  return t === "embedding" || t === "embeddings";
}

function parseLlamaArgsContext(args?: string[]): number | undefined {
  if (!Array.isArray(args)) return undefined;
  for (let i = 0; i < args.length; i++) {
    const arg = args[i];
    if (arg === "--ctx-size" || arg === "-c" || arg === "-ctx") {
      const val = Number(args[i + 1]);
      if (Number.isSafeInteger(val) && val > 0) return val;
    } else if (arg.startsWith("--ctx-size=")) {
      const val = Number(arg.slice("--ctx-size=".length));
      if (Number.isSafeInteger(val) && val > 0) return val;
    }
  }
  return undefined;
}

function hasExplicitRuntimeContext(m: RelayModel): boolean {
  return Boolean(
    positiveInt(m.contextWindow) ??
      positiveInt(m.meta?.n_ctx) ??
      positiveInt(m.loaded_context_length) ??
      positiveInt(m.context_window) ??
      positiveInt(m.context_length) ??
      positiveInt(m.max_model_len) ??
      positiveInt(m.max_context_length) ??
      positiveInt(m.n_ctx) ??
      positiveInt(m.top_provider?.context_length),
  );
}

function hasExplicitReasoningMetadata(m: RelayModel): boolean {
  const caps = capsObject(m);
  return Boolean(
    m.thinkingLevelMap ||
      m.thinking_level_map ||
      m.reasoning_efforts ||
      m.supported_reasoning_efforts ||
      m.reasoning_effort_levels ||
      caps?.reasoning_efforts ||
      m.thinking_levels ||
      Array.isArray(m.reasoning_effort) ||
      typeof m.reasoning === "boolean" ||
      (typeof m.reasoning === "object" && m.reasoning !== null) ||
      typeof caps?.reasoning === "boolean" ||
      (typeof caps?.reasoning === "object" && caps.reasoning !== null) ||
      typeof caps?.thinking === "boolean" ||
      (Array.isArray(m.supported_parameters) &&
        m.supported_parameters.some((p) => p === "reasoning" || p === "reasoning_effort")),
  );
}

function mapEffortList(
  rawEfforts: unknown[],
  explicitFormat?: ThinkingFormat,
): ThinkingConfig {
  const strings = rawEfforts.filter((e): e is string => typeof e === "string");
  const byLower = new Map(strings.map((e) => [e.toLowerCase(), e]));
  const map: Record<string, string | null> = {
    off: byLower.get("none") ?? byLower.get("off") ?? byLower.get("disabled") ?? null,
  };
  let hasActive = false;
  for (const lvl of EFFORT_LEVELS) {
    const val = byLower.get(lvl) ?? null;
    map[lvl] = val;
    if (val) hasActive = true;
  }
  const thinkingFormat = explicitFormat ?? (hasActive ? "openai" : undefined);
  const usesChatTemplate =
    thinkingFormat === "qwen-chat-template" || thinkingFormat === "chat-template";
  return {
    reasoning: hasActive,
    ...(hasActive && { thinkingLevelMap: map }),
    thinkingFormat,
    supportsReasoningEffort: hasActive && !usesChatTemplate,
  };
}

function inferThinkingFromModelName(
  m: RelayModel,
  extraHints: (string | undefined)[] = [],
): ThinkingConfig | undefined {
  const hint = [m.id, m.root ?? undefined, m.arch, ...extraHints]
    .filter(Boolean)
    .join(" ")
    .toLowerCase();

  // OpenAI gpt-oss (Harmony template) uses OpenAI reasoning_effort on vLLM, llama.cpp & LM Studio.
  if (/(?:^|[-_./\s])gpt[-_]?oss(?:$|[-_./\s])/.test(hint)) {
    return {
      reasoning: true,
      thinkingLevelMap: OPENAI_EFFORT_MAP,
      thinkingFormat: "openai",
      supportsReasoningEffort: true,
    };
  }

  // DeepSeek-V3.1 / V3.2 hybrid thinking uses chat_template_kwargs.thinking on vLLM.
  if (/deepseek[-_.]?v3\.[12]/.test(hint)) {
    return {
      reasoning: true,
      thinkingLevelMap: BINARY_THINKING_MAP,
      thinkingFormat: "chat-template",
      supportsReasoningEffort: false,
      chatTemplateKwargs: { thinking: { $var: "thinking.enabled" } },
    };
  }

  // Hybrid enable_thinking models (Qwen3/3.5 non-instruct/non-coder, GLM-4.5+, SmolLM3).
  const isHybridQwen3 =
    /qwen[-_.]?3(?:\.5)?(?:$|[-_./\s])/.test(hint) &&
    !/(?:^|[-_./\s])(instruct|coder|thinking|embed|embedding|rerank|reranker|guard)(?:$|[-_./\s])/.test(
      hint,
    );
  const isHybridGlm = /glm[-_.]?(?:4\.[5-9]|5)(?:$|[-_./\s])/.test(hint);
  const isHybridSmol = /smollm3/.test(hint);

  if (isHybridQwen3 || isHybridGlm || isHybridSmol) {
    return {
      reasoning: true,
      thinkingLevelMap: BINARY_THINKING_MAP,
      thinkingFormat: "qwen-chat-template",
      supportsReasoningEffort: false,
    };
  }

  return undefined;
}

function resolveThinking(
  m: RelayModel,
  props?: LlamaProps,
  lm?: LmStudioModelMeta,
  ollama?: OllamaShowMeta,
  cached?: {
    reasoning?: boolean;
    thinkingLevelMap?: Record<string, string | null>;
    compat?: Record<string, unknown>;
  },
): ThinkingConfig {
  const caps = capsObject(m);

  // 1. Explicit thinkingLevelMap from /v1/models
  const explicit = m.thinkingLevelMap ?? m.thinking_level_map;
  if (explicit && typeof explicit === "object") {
    const reasoning =
      m.reasoning !== false &&
      Object.entries(explicit).some(([k, v]) => k !== "off" && typeof v === "string");
    const thinkingFormat =
      (m.thinking_format ?? (m.compat?.thinkingFormat as ThinkingFormat | undefined)) ??
      (reasoning ? "openai" : undefined);
    const usesChatTemplate =
      thinkingFormat === "qwen-chat-template" || thinkingFormat === "chat-template";
    return {
      reasoning,
      thinkingLevelMap: explicit,
      thinkingFormat,
      supportsReasoningEffort: reasoning && !usesChatTemplate,
    };
  }

  // 2. Explicit effort list from /v1/models
  const rawEfforts =
    m.reasoning_efforts ??
    m.supported_reasoning_efforts ??
    m.reasoning_effort_levels ??
    caps?.reasoning_efforts ??
    m.thinking_levels ??
    (Array.isArray(m.reasoning_effort) ? m.reasoning_effort : undefined) ??
    (typeof m.reasoning === "object" && m.reasoning !== null
      ? m.reasoning.efforts ?? m.reasoning.allowed_options
      : undefined) ??
    (typeof caps?.reasoning === "object" && caps.reasoning !== null
      ? caps.reasoning.efforts ?? caps.reasoning.allowed_options
      : undefined);

  if (Array.isArray(rawEfforts)) {
    return mapEffortList(rawEfforts, m.thinking_format);
  }

  // 3. Explicit boolean reasoning flag from /v1/models
  if (hasExplicitReasoningMetadata(m)) {
    const reasoning =
      typeof m.reasoning === "boolean"
        ? m.reasoning
        : typeof m.reasoning === "object" && m.reasoning !== null && "supported" in m.reasoning
          ? Boolean(m.reasoning.supported)
          : typeof caps?.reasoning === "boolean"
            ? caps.reasoning
            : typeof caps?.reasoning === "object" && caps.reasoning !== null && "supported" in caps.reasoning
              ? Boolean(caps.reasoning.supported)
              : caps?.thinking ??
                (Array.isArray(m.supported_parameters)
                  ? m.supported_parameters.some((p) => p === "reasoning" || p === "reasoning_effort")
                  : false);
    if (!reasoning) {
      return { reasoning: false, supportsReasoningEffort: false };
    }
    const thinkingFormat = m.thinking_format ?? "openai";
    if (thinkingFormat === "qwen-chat-template") {
      return {
        reasoning: true,
        thinkingLevelMap: BINARY_THINKING_MAP,
        thinkingFormat,
        supportsReasoningEffort: false,
      };
    }
    return {
      reasoning: true,
      thinkingLevelMap: OPENAI_EFFORT_MAP,
      thinkingFormat,
      supportsReasoningEffort: true,
    };
  }

  // 4. Authoritative llama.cpp /props chat_template inspection
  if (typeof props?.chat_template === "string") {
    if (props.chat_template.includes("enable_thinking")) {
      return {
        reasoning: true,
        thinkingLevelMap: BINARY_THINKING_MAP,
        thinkingFormat: "qwen-chat-template",
        supportsReasoningEffort: false,
      };
    }
    if (props.chat_template.includes("reasoning_effort")) {
      return {
        reasoning: true,
        thinkingLevelMap: OPENAI_EFFORT_MAP,
        thinkingFormat: "openai",
        supportsReasoningEffort: true,
      };
    }
    return { reasoning: false, supportsReasoningEffort: false };
  }

  // 5. LM Studio native /api/v1/models capabilities.reasoning metadata
  if (lm?.reasoning !== undefined) {
    if (lm.reasoning === false) {
      return { reasoning: false, supportsReasoningEffort: false };
    }
    if (typeof lm.reasoning === "object" && lm.reasoning !== null) {
      const opts = lm.reasoning.allowed_options ?? lm.reasoning.efforts;
      if (Array.isArray(opts)) {
        const mapped = mapEffortList(opts);
        if (mapped.reasoning) return mapped;
        // Binary on/off options (e.g. ["off", "on"] or [false, true]) -> Qwen-style chat_template_kwargs
        const hasToggle = opts.some((v) => v === true || String(v).toLowerCase() === "on");
        if (hasToggle) {
          return {
            reasoning: true,
            thinkingLevelMap: BINARY_THINKING_MAP,
            thinkingFormat: "qwen-chat-template",
            supportsReasoningEffort: false,
          };
        }
        return { reasoning: false, supportsReasoningEffort: false };
      }
      if ("supported" in lm.reasoning && !lm.reasoning.supported) {
        return { reasoning: false, supportsReasoningEffort: false };
      }
    }
    return (
      inferThinkingFromModelName(m, [lm.displayName, lm.arch]) ?? {
        reasoning: true,
        thinkingLevelMap: OPENAI_EFFORT_MAP,
        thinkingFormat: "openai",
        supportsReasoningEffort: true,
      }
    );
  }

  // 6. Ollama native /api/show capabilities & thinking.values metadata
  if (ollama?.thinking !== undefined) {
    if (!ollama.thinking) {
      return { reasoning: false, supportsReasoningEffort: false };
    }
    if (Array.isArray(ollama.thinkingValues)) {
      const mapped = mapEffortList(ollama.thinkingValues);
      if (mapped.reasoning) return mapped;
    }
    return {
      reasoning: true,
      thinkingLevelMap: { off: "none", minimal: null, low: "low", medium: "medium", high: "high", xhigh: null },
      thinkingFormat: "openai",
      supportsReasoningEffort: true,
    };
  }

  // 7. Sleeping / unloaded llama.cpp router model: preserve cached thinking config from when loaded
  if (isLlamaRouterModel(m) && m.status?.value !== "loaded" && cached && cached.reasoning !== undefined) {
    const fmt = cached.compat?.thinkingFormat as ThinkingFormat | undefined;
    const usesChatTemplate = fmt === "qwen-chat-template" || fmt === "chat-template";
    return {
      reasoning: Boolean(cached.reasoning),
      ...(cached.thinkingLevelMap && { thinkingLevelMap: cached.thinkingLevelMap }),
      ...(fmt && { thinkingFormat: fmt }),
      supportsReasoningEffort: Boolean(cached.reasoning) && !usesChatTemplate,
      ...(cached.compat?.chatTemplateKwargs &&
      typeof cached.compat.chatTemplateKwargs === "object"
        ? { chatTemplateKwargs: cached.compat.chatTemplateKwargs as Record<string, unknown> }
        : {}),
    };
  }

  // 8. Fallback for stock vLLM, LM Studio v0 (/api/v0/models), and llama.cpp if /props was unreachable
  if (isVllmModel(m) || isLlamaCppModel(m) || isLmStudioModel(m) || lm !== undefined) {
    const inferred = inferThinkingFromModelName(m, [lm?.displayName, lm?.arch]);
    if (inferred) return inferred;
  }

  return { reasoning: false, supportsReasoningEffort: false };
}

function resolveVision(
  m: RelayModel,
  props?: LlamaProps,
  lm?: LmStudioModelMeta,
  ollama?: OllamaShowMeta,
): boolean {
  const caps = capsObject(m);
  if (Array.isArray(m.architecture?.input_modalities)) {
    return m.architecture.input_modalities.includes("image");
  }
  if (typeof props?.modalities?.vision === "boolean") {
    return props.modalities.vision;
  }
  if (typeof lm?.vision === "boolean") {
    return lm.vision;
  }
  if (typeof ollama?.vision === "boolean") {
    return ollama.vision;
  }
  if (typeof m.modalities?.vision === "boolean") {
    return m.modalities.vision;
  }
  if (typeof caps?.vision === "boolean") {
    return caps.vision;
  }
  if (Array.isArray(m.capabilities) && m.capabilities.includes("vision")) {
    return true;
  }
  if (typeof m.vision === "boolean" || typeof m.multimodal === "boolean") {
    return Boolean(m.vision || m.multimodal);
  }
  if (typeof m.meta?.multimodal === "boolean") {
    return m.meta.multimodal;
  }
  if (m.type?.toLowerCase() === "vlm") {
    return true;
  }
  if (isVllmModel(m)) {
    const hint = `${m.id} ${m.root ?? ""}`.toLowerCase();
    return /(?:^|[-_./])(vl|vision|pixtral|llava|internvl|minicpm-v|molmo)(?:$|[-_./])/.test(hint);
  }
  return false;
}

function timedSignal(signal?: AbortSignal, ms = 5_000): AbortSignal {
  const timeout = AbortSignal.timeout(ms);
  return signal && typeof AbortSignal.any === "function"
    ? AbortSignal.any([signal, timeout])
    : signal ?? timeout;
}

async function fetchLlamaProps(
  serverOrigin: string,
  modelId: string,
  apiKey: string,
  signal?: AbortSignal,
): Promise<LlamaProps | undefined> {
  try {
    const query = new URLSearchParams({ model: modelId, autoload: "false" });
    const res = await fetch(`${serverOrigin}/props?${query}`, {
      headers: {
        Authorization: `Bearer ${apiKey}`,
        "X-Model": modelId,
      },
      signal: timedSignal(signal),
    });
    if (!res.ok) return undefined;
    const json = (await res.json()) as unknown;
    return json && typeof json === "object" ? (json as LlamaProps) : undefined;
  } catch {
    return undefined;
  }
}

async function fetchLmStudioCatalog(
  serverOrigin: string,
  routingModelId: string,
  apiKey: string,
  signal?: AbortSignal,
): Promise<Map<string, LmStudioModelMeta> | undefined> {
  const headers = {
    Authorization: `Bearer ${apiKey}`,
    "X-Model": routingModelId,
  };

  // Try LM Studio 0.4+ native REST API: GET /api/v1/models
  try {
    const res = await fetch(`${serverOrigin}/api/v1/models`, {
      headers,
      signal: timedSignal(signal),
    });
    if (res.ok) {
      const json = (await res.json()) as {
        models?: Array<{
          key?: string;
          display_name?: string;
          type?: string;
          architecture?: string;
          max_context_length?: number;
          capabilities?: {
            vision?: boolean;
            reasoning?: boolean | { supported?: boolean; efforts?: string[]; allowed_options?: unknown[] };
          };
          loaded_instances?: Array<{
            id?: string;
            config?: { context_length?: number };
          }>;
        }>;
      };
      if (Array.isArray(json?.models)) {
        const out = new Map<string, LmStudioModelMeta>();
        for (const entry of json.models) {
          const t = entry.type?.toLowerCase();
          const isEmbedding = t === "embedding" || t === "embeddings";
          const instances = Array.isArray(entry.loaded_instances) ? entry.loaded_instances : [];
          const defaultCtx =
            positiveInt(instances[0]?.config?.context_length) ??
            positiveInt(entry.max_context_length);
          const baseMeta: Omit<LmStudioModelMeta, "id" | "contextWindow" | "isLoaded"> = {
            displayName: entry.display_name,
            arch: entry.architecture,
            isEmbedding,
            vision:
              typeof entry.capabilities?.vision === "boolean"
                ? entry.capabilities.vision
                : t === "vlm"
                  ? true
                  : undefined,
            reasoning: entry.capabilities?.reasoning,
          };
          if (entry.key) {
            out.set(entry.key, {
              ...baseMeta,
              id: entry.key,
              isLoaded: instances.length > 0,
              contextWindow: defaultCtx,
            });
          }
          for (const inst of instances) {
            if (inst.id) {
              out.set(inst.id, {
                ...baseMeta,
                id: inst.id,
                isLoaded: true,
                contextWindow:
                  positiveInt(inst.config?.context_length) ?? defaultCtx,
              });
            }
          }
        }
        return out;
      }
    }
  } catch {
    // Fall through to /api/v0/models
  }

  // Fallback to LM Studio 0.3.x / 0.4.x REST API: GET /api/v0/models
  try {
    const res = await fetch(`${serverOrigin}/api/v0/models`, {
      headers,
      signal: timedSignal(signal),
    });
    if (!res.ok) return undefined;
    const json = (await res.json()) as {
      data?: Array<{
        id?: string;
        type?: string;
        arch?: string;
        state?: string;
        max_context_length?: number;
        loaded_context_length?: number;
        capabilities?: string[] | { vision?: boolean; reasoning?: boolean };
      }>;
    };
    if (!Array.isArray(json?.data)) return undefined;
    const out = new Map<string, LmStudioModelMeta>();
    for (const entry of json.data) {
      if (!entry.id) continue;
      const t = entry.type?.toLowerCase();
      const isEmbedding = t === "embedding" || t === "embeddings";
      const vision =
        t === "vlm"
          ? true
          : Array.isArray(entry.capabilities)
            ? entry.capabilities.includes("vision")
              ? true
              : undefined
            : typeof entry.capabilities?.vision === "boolean"
              ? entry.capabilities.vision
              : undefined;
      out.set(entry.id, {
        id: entry.id,
        arch: entry.arch,
        isEmbedding,
        isLoaded:
          entry.state === "loaded" || positiveInt(entry.loaded_context_length) !== undefined,
        contextWindow:
          positiveInt(entry.loaded_context_length) ?? positiveInt(entry.max_context_length),
        vision,
        reasoning:
          !Array.isArray(entry.capabilities) && typeof entry.capabilities?.reasoning === "boolean"
            ? entry.capabilities.reasoning
            : undefined,
      });
    }
    return out;
  } catch {
    return undefined;
  }
}

async function fetchOllamaShow(
  serverOrigin: string,
  modelId: string,
  apiKey: string,
  signal?: AbortSignal,
): Promise<OllamaShowMeta | undefined> {
  try {
    const res = await fetch(`${serverOrigin}/api/show`, {
      method: "POST",
      headers: {
        Authorization: `Bearer ${apiKey}`,
        "Content-Type": "application/json",
        "X-Model": modelId,
      },
      body: JSON.stringify({ model: modelId }),
      signal: timedSignal(signal),
    });
    if (!res.ok) return undefined;
    const json = (await res.json()) as {
      parameters?: string;
      model_info?: Record<string, unknown>;
      capabilities?: string[];
      thinking?: { values?: unknown[] };
    };
    if (!json || typeof json !== "object") return undefined;

    const caps = Array.isArray(json.capabilities)
      ? json.capabilities.map((c) => String(c).toLowerCase())
      : undefined;
    const isEmbedding = Boolean(
      caps && caps.includes("embedding") && !caps.includes("completion"),
    );

    let paramCtx: number | undefined;
    if (typeof json.parameters === "string") {
      const match = json.parameters.match(/^\s*num_ctx\s+(\d+)/m);
      if (match) paramCtx = positiveInt(Number(match[1]));
    }
    let archCtx: number | undefined;
    if (json.model_info && typeof json.model_info === "object") {
      for (const [k, v] of Object.entries(json.model_info)) {
        if (k.endsWith(".context_length")) {
          archCtx = positiveInt(v);
          if (archCtx) break;
        }
      }
    }

    return {
      isEmbedding,
      contextWindow: paramCtx ?? archCtx,
      vision: caps ? caps.includes("vision") : undefined,
      thinking: caps ? caps.includes("thinking") : undefined,
      thinkingValues: Array.isArray(json.thinking?.values) ? json.thinking.values : undefined,
    };
  } catch {
    return undefined;
  }
}

function extractOAuthCode(input: string, expectedState: string): string {
  const trimmed = input.trim();
  if (!trimmed) throw new Error("Empty authorization input");
  if (trimmed.startsWith("http://") || trimmed.startsWith("https://")) {
    const parsed = new URL(trimmed);
    const err = parsed.searchParams.get("error");
    if (err) throw new Error(`ModelRelay authorization failed: ${err}`);
    const state = parsed.searchParams.get("state");
    if (state && state !== expectedState) throw new Error("OAuth state mismatch");
    const code = parsed.searchParams.get("code");
    if (!code) throw new Error("Missing authorization code in callback URL");
    return code;
  }
  return trimmed;
}

async function loginWithModelRelayOAuth(callbacks: {
  onAuth(info: { url: string; instructions?: string }): void;
  onProgress?(message: string): void;
  onManualCodeInput?(): Promise<string>;
  signal?: AbortSignal;
}) {
  const siteOrigin = DEFAULT_SITE_URL.replace(/\/+$/, "");
  const codeVerifier = randomBytes(32).toString("base64url");
  const codeChallenge = createHash("sha256").update(codeVerifier).digest("base64url");
  const state = randomBytes(16).toString("base64url");

  let resolveCode!: (code: string) => void;
  let rejectCode!: (err: Error) => void;
  const callbackPromise = new Promise<string>((resolve, reject) => {
    resolveCode = resolve;
    rejectCode = reject;
  });

  const server = createServer((req, res) => {
    const url = new URL(req.url ?? "/", "http://127.0.0.1");
    if (req.method !== "GET" || url.pathname !== "/callback") {
      res.writeHead(404, { "content-type": "text/plain; charset=utf-8" });
      res.end("Not found");
      return;
    }
    const err = url.searchParams.get("error");
    if (err) {
      res.writeHead(400, { "content-type": "text/html; charset=utf-8" });
      res.end("<h1>ModelRelay sign-in cancelled</h1><p>You can close this tab.</p>");
      rejectCode(new Error(`ModelRelay authorization failed: ${err}`));
      return;
    }
    const gotState = url.searchParams.get("state");
    const code = url.searchParams.get("code");
    if (!code || gotState !== state) {
      res.writeHead(400, { "content-type": "text/html; charset=utf-8" });
      res.end("<h1>Invalid OAuth callback</h1><p>Please retry /login modelrelay in Pi.</p>");
      rejectCode(new Error("Invalid OAuth callback state or missing code"));
      return;
    }
    res.writeHead(200, { "content-type": "text/html; charset=utf-8", "cache-control": "no-store" });
    res.end("<h1>Connected to Pi!</h1><p>You can close this tab and return to your terminal.</p>");
    resolveCode(code);
  });

  await new Promise<void>((resolve, reject) => {
    server.once("error", reject);
    server.listen(0, "127.0.0.1", () => {
      server.off("error", reject);
      resolve();
    });
  });

  const addr = server.address();
  if (!addr || typeof addr === "string") {
    server.close();
    throw new Error("Failed to bind local OAuth callback server");
  }

  const callbackUrl = `http://127.0.0.1:${addr.port}/callback`;
  const authorizeParams = new URLSearchParams({
    callback_url: callbackUrl,
    code_challenge: codeChallenge,
    code_challenge_method: "S256",
    state,
    key_label: "Pi Coding Agent",
  });
  const authorizeUrl = `${siteOrigin}/oauth/authorize?${authorizeParams}`;

  const onAbort = () => rejectCode(new Error("Login cancelled"));
  callbacks.signal?.addEventListener("abort", onAbort, { once: true });

  callbacks.onAuth({
    url: authorizeUrl,
    instructions:
      "Approve access in your browser to connect Pi to ModelRelay (no API key copy-paste needed).",
  });

  try {
    const racers: Promise<string>[] = [callbackPromise];
    if (callbacks.onManualCodeInput) {
      racers.push(
        callbacks
          .onManualCodeInput()
          .then((input) => extractOAuthCode(input, state)),
      );
    }
    const code = await Promise.race(racers);
    callbacks.onProgress?.("Exchanging ModelRelay authorization code...");

    const tokenRes = await fetch(`${siteOrigin}/oauth/token`, {
      method: "POST",
      headers: { "Content-Type": "application/json", Accept: "application/json" },
      body: JSON.stringify({
        code,
        code_verifier: codeVerifier,
        code_challenge_method: "S256",
        callback_url: callbackUrl,
      }),
      signal: callbacks.signal,
    });
    if (!tokenRes.ok) {
      const text = await tokenRes.text().catch(() => "");
      throw new Error(`ModelRelay token exchange failed (${tokenRes.status}): ${text}`);
    }
    const token = (await tokenRes.json()) as { key?: string; access_token?: string };
    const apiKey = token.key || token.access_token;
    if (!apiKey) {
      throw new Error("ModelRelay token exchange did not return an API key");
    }
    return {
      access: apiKey,
      refresh: apiKey,
      expires: Date.now() + 100 * 365 * 24 * 60 * 60 * 1000,
    };
  } finally {
    callbacks.signal?.removeEventListener("abort", onAbort);
    server.close();
    if (typeof server.closeAllConnections === "function") {
      server.closeAllConnections();
    }
  }
}

export default function (pi: ExtensionAPI) {
  pi.registerProvider("modelrelay", {
    name: "ModelRelay",
    baseUrl: DEFAULT_BASE_URL,
    apiKey: "$MODELRELAY_API_KEY",
    api: "openai-completions",
    oauth: {
      name: "ModelRelay (Browser OAuth)",
      login: loginWithModelRelayOAuth,
      async refreshToken(credentials) {
        return credentials;
      },
      getApiKey(credentials) {
        return credentials.access;
      },
    },
    async refreshModels({ credential, stored, publish, allowNetwork, baseUrl, signal }) {
      const root = (baseUrl ?? DEFAULT_BASE_URL).replace(/\/+$/, "");
      const serverOrigin = root.replace(/\/v1$/u, "");
      const cached = (stored?.models ?? [])
        .filter((m) => m.provider === "modelrelay" && "maxTokens" in m)
        .map(({ api: _a, provider: _p, baseUrl: _b, ...rest }) => rest);
      const cachedById = new Map(cached.map((m) => [m.id, m]));

      const apiKey =
        credential?.type === "api_key"
          ? credential.key
          : credential?.type === "oauth"
            ? credential.access
            : process.env.MODELRELAY_API_KEY;

      if (!allowNetwork || !apiKey) {
        return cached;
      }

      const res = await fetch(`${root}/models`, {
        headers: { Authorization: `Bearer ${apiKey}` },
        signal,
      });
      if (!res.ok) return cached;

      const { data = [] } = (await res.json()) as { data?: RelayModel[] };

      // Skip broken router models, embedding models, and llama.cpp decision-only classifiers.
      const candidates = data.filter(
        (m) => !m.status?.failed && !isDecisionOnlyModel(m) && !isEmbeddingModel(m),
      );

      // 1. llama.cpp: probe /props in parallel ONLY for loaded llama.cpp models.
      const propsById = new Map<string, LlamaProps>();
      let routerAutoload: boolean | undefined;

      const llamaModelsToProbe = candidates.filter(
        (m) =>
          isLlamaCppModel(m) &&
          (!isLlamaRouterModel(m) || m.status?.value === "loaded") &&
          (!hasExplicitReasoningMetadata(m) || !hasExplicitRuntimeContext(m)),
      );

      await Promise.all(
        llamaModelsToProbe.map(async (m) => {
          const props = await fetchLlamaProps(serverOrigin, m.id, apiKey, signal);
          if (props) {
            propsById.set(m.id, props);
            if (typeof props.models_autoload === "boolean") {
              routerAutoload = props.models_autoload;
            }
          }
        }),
      );

      const firstUnloadedPreset = candidates.find(
        (m) => isLlamaRouterModel(m) && m.status?.value === "unloaded" && m.source === "preset",
      );
      if (firstUnloadedPreset && routerAutoload === undefined) {
        const props = await fetchLlamaProps(
          serverOrigin,
          firstUnloadedPreset.id,
          apiKey,
          signal,
        );
        if (typeof props?.models_autoload === "boolean") {
          routerAutoload = props.models_autoload;
        }
      }

      // 2. LM Studio: query /api/v1/models (or /api/v0/models) once per distinct LM Studio worker.
      //    Each response covers every model on that machine, so models already returned by an
      //    earlier catalog are skipped while models on a second LM Studio machine get probed.
      const lmById = new Map<string, LmStudioModelMeta>();
      const mergeLmCatalog = (catalog: Map<string, LmStudioModelMeta>) => {
        for (const [k, v] of catalog) {
          const existing = lmById.get(k);
          if (!existing || (!existing.isLoaded && v.isLoaded)) {
            lmById.set(k, v);
          }
        }
      };

      const lmCandidates = candidates.filter(
        (m) =>
          !isLlamaCppModel(m) &&
          !isVllmModel(m) &&
          (!hasExplicitRuntimeContext(m) || !hasExplicitReasoningMetadata(m)) &&
          (isLmStudioModel(m) || !isOllamaModel(m)),
      );
      const explicitLm = lmCandidates.filter(isLmStudioModel);
      const unknownBare = lmCandidates.filter((m) => !isLmStudioModel(m));

      for (const m of explicitLm) {
        if (lmById.has(m.id)) continue;
        const catalog = await fetchLmStudioCatalog(serverOrigin, m.id, apiKey, signal);
        if (catalog) mergeLmCatalog(catalog);
      }

      for (const m of unknownBare) {
        if (lmById.has(m.id)) continue;
        const catalog = await fetchLmStudioCatalog(serverOrigin, m.id, apiKey, signal);
        if (!catalog) break;
        mergeLmCatalog(catalog);
      }

      // 3. Ollama: probe POST /api/show ONLY for models still missing metadata after LM Studio.
      const ollamaById = new Map<string, OllamaShowMeta>();
      const ollamaCandidates = candidates.filter(
        (m) =>
          !isLlamaCppModel(m) &&
          !isVllmModel(m) &&
          !lmById.has(m.id) &&
          (!hasExplicitRuntimeContext(m) || !hasExplicitReasoningMetadata(m)),
      );
      const explicitOllama = ollamaCandidates.filter(isOllamaModel);
      if (explicitOllama.length > 0) {
        await Promise.all(
          explicitOllama.map(async (m) => {
            const info = await fetchOllamaShow(serverOrigin, m.id, apiKey, signal);
            if (info) ollamaById.set(m.id, info);
          }),
        );
      } else if (ollamaCandidates.length > 0) {
        // Probe the first bare model; only fan out if the backend actually speaks Ollama /api/show.
        const firstInfo = await fetchOllamaShow(
          serverOrigin,
          ollamaCandidates[0].id,
          apiKey,
          signal,
        );
        if (firstInfo) {
          ollamaById.set(ollamaCandidates[0].id, firstInfo);
          await Promise.all(
            ollamaCandidates.slice(1).map(async (m) => {
              const info = await fetchOllamaShow(serverOrigin, m.id, apiKey, signal);
              if (info) ollamaById.set(m.id, info);
            }),
          );
        }
      }

      const selectable = candidates.filter((m) => {
        if (lmById.get(m.id)?.isEmbedding || ollamaById.get(m.id)?.isEmbedding) {
          return false;
        }
        if (!isLlamaRouterModel(m)) return true;
        const status = m.status!.value;
        if (status === "loaded" || status === "sleeping") return true;
        if (status === "unloaded" && m.source === "preset") {
          return routerAutoload !== false;
        }
        return false;
      });

      const models = selectable.map((m) => {
        const props = propsById.get(m.id);
        const lm = lmById.get(m.id);
        const ollama = ollamaById.get(m.id);
        const prev = cachedById.get(m.id);

        const contextWindow =
          positiveInt(m.contextWindow) ??
          positiveInt(m.meta?.n_ctx) ??
          positiveInt(m.loaded_context_length) ??
          positiveInt(m.context_window) ??
          positiveInt(m.context_length) ??
          positiveInt(m.max_model_len) ??
          positiveInt(m.max_context_length) ??
          positiveInt(m.n_ctx) ??
          positiveInt(m.top_provider?.context_length) ??
          positiveInt(props?.default_generation_settings?.n_ctx) ??
          positiveInt(lm?.contextWindow) ??
          positiveInt(ollama?.contextWindow) ??
          parseLlamaArgsContext(m.status?.args) ??
          positiveInt(prev?.contextWindow) ??
          positiveInt(m.meta?.n_ctx_train) ??
          128000;

        const maxTokens = Math.min(
          positiveInt(m.maxTokens) ??
            positiveInt(m.max_output_tokens) ??
            positiveInt(m.max_completion_tokens) ??
            positiveInt(m.max_tokens) ??
            positiveInt(m.top_provider?.max_completion_tokens) ??
            32768,
          contextWindow,
        );

        const thinking = resolveThinking(
          m,
          props,
          lm,
          ollama,
          prev as
            | {
                reasoning?: boolean;
                thinkingLevelMap?: Record<string, string | null>;
                compat?: Record<string, unknown>;
              }
            | undefined,
        );
        const hasVision = resolveVision(m, props, lm, ollama);

        return {
          id: m.id,
          name: m.name || lm?.displayName || m.id,
          reasoning: thinking.reasoning,
          ...(thinking.thinkingLevelMap && { thinkingLevelMap: thinking.thinkingLevelMap }),
          input: (hasVision ? ["text", "image"] : ["text"]) as ("text" | "image")[],
          cost: { input: 0, output: 0, cacheRead: 0, cacheWrite: 0 },
          contextWindow,
          maxTokens,
          ...(m.samplingParams && { samplingParams: m.samplingParams }),
          ...(m.samplingParamsByThinkingLevel && {
            samplingParamsByThinkingLevel: m.samplingParamsByThinkingLevel,
          }),
          compat: {
            supportsStore: false,
            supportsDeveloperRole: false,
            supportsReasoningEffort: thinking.supportsReasoningEffort,
            supportsUsageInStreaming: true,
            supportsStrictMode: false,
            maxTokensField: "max_tokens" as const,
            ...(thinking.thinkingFormat && { thinkingFormat: thinking.thinkingFormat }),
            ...(thinking.chatTemplateKwargs && {
              chatTemplateKwargs: thinking.chatTemplateKwargs,
            }),
            ...(m.compat ?? {}),
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
