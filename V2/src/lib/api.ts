// API layer. In Tauri it calls Rust commands + listens to events.
// In a plain browser it falls back to mock data so the notch UI can be
// developed and previewed without the backend compiled.

import type {
  AskResult,
  AskStats,
  ChatTurn,
  FolderInfo,
  MonitorInfo,
  ModelInfo,
  RuntimeStatus,
  Source,
} from "./types";

import type { AnswerOptions } from "./chat-options.svelte";

export const isTauri = (): boolean =>
  typeof window !== "undefined" &&
  ("__TAURI_INTERNALS__" in window || "__TAURI__" in window);

// --- Lazy Tauri bindings (only imported when running inside Tauri) --------
async function invoke<T>(cmd: string, args?: Record<string, unknown>): Promise<T> {
  const { invoke } = await import("@tauri-apps/api/core");
  return invoke<T>(cmd, args);
}

// ============================ MOCK STATE ===================================
const sleep = (ms: number) => new Promise((r) => setTimeout(r, ms));

let mockModels: ModelInfo[] = [
  { id: "qwen2.5-3b-instruct-q4", name: "Qwen2.5 3B Instruct", type: "chat", params: "3B", quant: "Q4_K_M", sizeBytes: 2_020_000_000, ctx: 32768, installed: true, selected: true, downloadProgress: null },
  { id: "llama-3.2-3b-instruct-q4", name: "Llama 3.2 3B Instruct", type: "chat", params: "3B", quant: "Q4_K_M", sizeBytes: 2_020_000_000, ctx: 131072, installed: false, selected: false, downloadProgress: null },
  { id: "gemma-3-4b-it-q4", name: "Gemma 3 4B Instruct", type: "chat", params: "4B", quant: "Q4_K_M", sizeBytes: 2_490_000_000, ctx: 8192, installed: false, selected: false, downloadProgress: null },
  { id: "phi-3.5-mini-q4", name: "Phi 3.5 Mini", type: "chat", params: "3.8B", quant: "Q4_K_M", sizeBytes: 2_200_000_000, ctx: 131072, installed: false, selected: false, downloadProgress: null },
  { id: "nomic-embed-text-v1.5", name: "Nomic Embed Text v1.5", type: "embed", params: "137M", quant: "F16", sizeBytes: 274_000_000, ctx: 8192, installed: true, selected: true, downloadProgress: null },
  { id: "bge-m3", name: "BGE-M3 (multilingual)", type: "embed", params: "567M", quant: "F16", sizeBytes: 1_130_000_000, ctx: 8192, installed: false, selected: false, downloadProgress: null },
  { id: "embeddinggemma-300m", name: "EmbeddingGemma 300M", type: "embed", params: "300M", quant: "Q8_0", sizeBytes: 320_000_000, ctx: 2048, installed: false, selected: false, downloadProgress: null },
  {
    id: "smolvlm-500m-q8",
    name: "SmolVLM 500M",
    type: "vision",
    params: "500M",
    quant: "Q8_0",
    sizeBytes: 636275712,
    ctx: 8192,
    installed: true,
    selected: false,
    downloadProgress: null,
  },
];

let mockFolders: FolderInfo[] = [
  { id: "f1", path: "~/Documents/Notes", status: "ready", fileCount: 84, chunkCount: 1203 },
  { id: "f2", path: "~/Projects/loft/docs", status: "ready", fileCount: 12, chunkCount: 198 },
];

const mockSources: Source[] = [
  { file: "architecture.pdf", path: "~/Projects/loft/docs/architecture.pdf", page: 3, location: "page 3", score: 0.82, snippet: "Two llama-server processes run locally: one for chat completions and one dedicated to embeddings…" },
  { file: "meeting-2026-01.md", path: "~/Documents/Notes/meeting-2026-01.md", score: 0.74, snippet: "We agreed the indexer should hash each file with blake3 so unchanged files are skipped on reindex." },
];

// ============================ PUBLIC API ===================================

export async function getRuntimeStatus(): Promise<RuntimeStatus> {
  if (isTauri()) return invoke<RuntimeStatus>("get_runtime_status");
  return { chat: "ready", embed: "ready", accelerator: "cpu" };
}

type Unsub = () => void;

/** Subscribe to live runtime status pushed from the backend. */
export async function onRuntimeStatus(cb: (s: RuntimeStatus) => void): Promise<Unsub> {
  if (!isTauri()) return () => {};
  const { listen } = await import("@tauri-apps/api/event");
  return listen<RuntimeStatus>("runtime:status", (e) => cb(e.payload));
}

/** Grow the native window before the panel opens / shrink it after it closes. */
export async function setExpanded(expanded: boolean): Promise<void> {
  if (isTauri()) return invoke<void>("set_expanded", { expanded });
}

let mockPlacement: "top" | "right" = "top";

/** Which screen edge the notch is docked at. */
export async function getPlacement(): Promise<"top" | "right"> {
  if (isTauri()) return invoke<"top" | "right">("get_placement");
  return mockPlacement;
}

/**
 * Dock the notch at another edge and remember it. `expanded` is the panel's
 * current state, so the window is moved at the right size.
 */
export async function setPlacement(placement: "top" | "right", expanded: boolean): Promise<void> {
  if (isTauri()) return invoke<void>("set_placement", { placement, expanded });
  mockPlacement = placement;
}

let mockMonitors: MonitorInfo[] = [
  { id: "mock-1", label: "Display 1", width: 1920, height: 1200, primary: true, selected: true },
  { id: "mock-2", label: "Display 2", width: 3440, height: 1440, primary: false, selected: false },
];

/** The attached displays, with the one the notch is on marked. */
export async function listMonitors(): Promise<MonitorInfo[]> {
  if (isTauri()) return invoke<MonitorInfo[]>("list_monitors");
  return structuredClone(mockMonitors);
}

/** Move the notch to another display and remember it. */
export async function setMonitor(id: string, expanded: boolean): Promise<void> {
  if (isTauri()) return invoke<void>("set_monitor", { id, expanded });
  mockMonitors = mockMonitors.map((m) => ({ ...m, selected: m.id === id }));
}

/** Quit Loft (stops the model servers too). */
export async function quitApp(): Promise<void> {
  if (isTauri()) return invoke<void>("quit_app");
}

let mockAutostart = false;

export async function getAutostart(): Promise<boolean> {
  if (isTauri()) return invoke<boolean>("get_autostart");
  return mockAutostart;
}

/** Turn launch-at-login on or off; resolves with the resulting state. */
export async function setAutostart(enabled: boolean): Promise<boolean> {
  if (isTauri()) return invoke<boolean>("set_autostart", { enabled });
  mockAutostart = enabled;
  return mockAutostart;
}

/** Summon-hotkey toggle event from the backend. */
export async function onToggle(cb: () => void): Promise<Unsub> {
  if (!isTauri()) return () => {};
  const { listen } = await import("@tauri-apps/api/event");
  return listen("loft://toggle", () => cb());
}

export async function listModels(): Promise<ModelInfo[]> {
  if (isTauri()) return invoke<ModelInfo[]>("list_models");
  return structuredClone(mockModels);
}

export async function downloadModel(
  id: string,
  onProgress?: (p: number) => void,
): Promise<void> {
  if (isTauri()) {
    const { listen } = await import("@tauri-apps/api/event");
    const un = await listen<{ id: string; progress: number }>(
      "download:progress",
      (e) => {
        if (e.payload.id === id) onProgress?.(e.payload.progress);
      },
    );
    try {
      await invoke<void>("download_model", { id });
    } finally {
      un();
    }
    return;
  }
  // Mock: animate a download
  const m = mockModels.find((x) => x.id === id);
  if (!m) return;
  for (let p = 0; p <= 1.0001; p += 0.08) {
    m.downloadProgress = Math.min(p, 1);
    onProgress?.(m.downloadProgress);
    await sleep(120);
  }
  m.downloadProgress = null;
  m.installed = true;
}

export async function selectModel(id: string): Promise<void> {
  if (isTauri()) return invoke<void>("select_model", { id });
  const target = mockModels.find((x) => x.id === id);
  if (!target) return;
  mockModels = mockModels.map((m) =>
    m.type === target.type ? { ...m, selected: m.id === id } : m,
  );
}

/**
 * Turn picture descriptions on (with that vision model) or off (`null`).
 * Indexed pictures are read again to match.
 */
export async function setPictureModel(id: string | null): Promise<void> {
  if (isTauri()) return invoke<void>("set_picture_model", { id });
  mockModels = mockModels.map((m) => (m.type === "vision" ? { ...m, selected: m.id === id } : m));
}

/** Fires whenever folders, their status or indexing progress change. */
export async function onFoldersChanged(cb: () => void): Promise<Unsub> {
  if (!isTauri()) return () => {};
  const { listen } = await import("@tauri-apps/api/event");
  return listen("folders:changed", () => cb());
}

export async function listFolders(): Promise<FolderInfo[]> {
  if (isTauri()) return invoke<FolderInfo[]>("list_folders");
  return structuredClone(mockFolders);
}

export async function addFolder(): Promise<FolderInfo | null> {
  if (isTauri()) return invoke<FolderInfo | null>("add_folder");
  const f: FolderInfo = {
    id: "f" + (mockFolders.length + 1),
    path: "~/New Folder " + (mockFolders.length + 1),
    status: "indexing",
    fileCount: 0,
    chunkCount: 0,
    progress: { done: 0, total: 40, current: "scanning…" },
  };
  mockFolders = [...mockFolders, f];
  return f;
}

export async function reindexFolder(id: string): Promise<void> {
  if (isTauri()) return invoke<void>("reindex_folder", { id });
  const f = mockFolders.find((x) => x.id === id);
  if (f) f.status = "indexing";
}

export async function deleteIndex(id: string): Promise<void> {
  if (isTauri()) return invoke<void>("delete_index", { id });
  const f = mockFolders.find((x) => x.id === id);
  if (f) {
    f.status = "idle";
    f.chunkCount = 0;
  }
}

export async function removeFolder(id: string): Promise<void> {
  if (isTauri()) return invoke<void>("remove_folder", { id });
  mockFolders = mockFolders.filter((x) => x.id !== id);
}

let mockAskStopped = false;

/**
 * Ask a question. `history` is the conversation so far (empty for a first
 * question); with it the question is treated as a follow-up. `options` are
 * the chat options that shape the answer. `onSources`
 * fires as soon as the relevant files are found (before any text is
 * generated), `onToken` streams the answer, and the promise resolves with the
 * final sources once the answer ends or is stopped.
 */
export async function ask(
  question: string,
  history: ChatTurn[],
  options: AnswerOptions,
  onToken: (chunk: string) => void,
  onSources?: (sources: Source[]) => void,
): Promise<AskResult> {
  if (isTauri()) {
    const { listen } = await import("@tauri-apps/api/event");
    let answer = "";
    const unToken = await listen<string>("ask:token", (e) => {
      answer += e.payload;
      onToken(e.payload);
    });
    const unSources = await listen<Source[]>("ask:sources", (e) => onSources?.(e.payload));
    try {
      const done = await invoke<{ sources: Source[]; stats: AskStats | null }>("ask", { question, history, options });
      return { answer, sources: done.sources, stats: done.stats };
    } finally {
      unToken();
      unSources();
    }
  }
  // Mock: sources first, then a streamed answer.
  mockAskStopped = false;
  await sleep(350);
  const sources = structuredClone(mockSources);
  onSources?.(sources);
  await sleep(500);
  const demo = history.length
    ? `Following on from "${history[history.length - 1].question}": the embedding server is the smaller of the two. ` +
      `(Preview data — follow-up ${history.length} — you asked: "${question.trim()}")`
    : `Loft runs two local llama.cpp servers, one for chat and one for embeddings. ` +
      `The indexer hashes each file with blake3 so unchanged files are skipped on reindex. ` +
      `(Preview data — you asked: "${question.trim()}")`;
  let answer = "";
  const demoText =
    options.length === "concise"
      ? "Two local llama.cpp servers: chat and embeddings."
      : options.length === "detailed"
        ? `${demo} In detail: the chat server writes the answer from the excerpts it is given, while the embedding server turns text into vectors, both when a folder is indexed and when a question is asked.`
        : demo;
  for (const word of demoText.split(" ")) {
    if (mockAskStopped) break;
    const chunk = word + " ";
    answer += chunk;
    onToken(chunk);
    await sleep(40);
  }
  if (mockAskStopped) return { answer, sources };
  // Made-up usage that grows with the conversation, like the real thing.
  const turnsSent = Math.min(history.length, 4);
  const stats: AskStats = {
    promptTokens: 640 + turnsSent * 230 + Math.round(question.length / 4),
    answerTokens: Math.round(answer.length / 4),
    contextSize: 8192,
    tokensPerSecond: 41.5,
    turnsSent,
    turnsDropped: history.length - turnsSent,
    excerpts: sources.length,
  };
  return { answer, sources, stats };
}

/** Stop the answer that is currently streaming. */
export async function cancelAsk(): Promise<void> {
  if (isTauri()) return invoke<void>("cancel_ask");
  mockAskStopped = true;
}

/** Open a cited file (documents) or show it in the file manager (code etc.). */
export async function openSource(path: string, page?: number | null): Promise<void> {
  if (isTauri()) return invoke<void>("open_source", { path, page: page ?? null });
}
