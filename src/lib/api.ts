// API layer. In Tauri it calls Rust commands + listens to events.
// In a plain browser it falls back to mock data so the notch UI can be
// developed and previewed without the backend compiled.

import type {
  AskResult,
  Conversation,
  ConversationView,
  SavedMessage,
  Backend,
  BenchProgress,
  BenchReport,
  BenchRow,
  EngineInstallProgress,
  EngineName,
  EngineTrial,
  EngineStatus,
  InferenceOverview,
  InferenceStatus,
  PowerSettings,
  PowerSource,
  Target,
  TargetOption,
  AskStats,
  ChatTurn,
  FolderInfo,
  MonitorInfo,
  ModelInfo,
  RuntimeStatus,
  Source,
  Usage,
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

// ---- Performance & Battery -------------------------------------------------

const NVIDIA = "nvidia-geforce-rtx-4050-laptop-gpu";
const IRIS = "intel-r-iris-r-xe-graphics";
/** Preview data: a laptop with NVIDIA and Intel graphics; CUDA and OpenVINO not installed. */
const mockOptions: TargetOption[] = [
  { target: { backend: "cuda", device: null }, label: "CUDA — not installed", available: false, experimental: false, reason: "The CUDA engine is an optional download." },
  { target: { backend: "vulkan", device: IRIS }, label: "Intel Iris Xe Graphics (Vulkan)", available: true, experimental: false },
  { target: { backend: "vulkan", device: NVIDIA }, label: "NVIDIA GeForce RTX 4050 Laptop GPU (Vulkan)", available: true, experimental: false },
  { target: { backend: "openvino", device: null }, label: "OpenVINO — not installed", available: false, experimental: true, reason: "The OpenVINO engine is an optional download." },
  { target: { backend: "cpu", device: null }, label: "CPU only", available: true, experimental: false },
];
let mockSettings: PowerSettings = {
  pluggedIn: { mode: "automatic", target: null, embedTarget: null },
  onBattery: { mode: "automatic", target: null, embedTarget: null },
  autoSwitch: true,
  allowFallback: true,
};
let mockPower: PowerSource = "ac";
let mockOverride: Target | null = null;
/** Preview: optional engines "installed" from the Settings page. */
const mockInstalled = new Set<Backend>();
let mockBench: BenchRow[] = [];
const OV_CPU = "intel-r-core-tm-i7-12700h";
const OV_GPU = "intel-r-iris-r-xe-graphics-igpu";
/** The options as they stand, given what has been "installed". */
function mockOptionsNow(): TargetOption[] {
  return mockOptions.flatMap((o): TargetOption[] => {
    if (o.target.backend === "cuda" && mockInstalled.has("cuda")) {
      return [{ target: { backend: "cuda", device: NVIDIA }, label: "NVIDIA GeForce RTX 4050 Laptop GPU (CUDA)", available: true, experimental: false }];
    }
    if (o.target.backend === "openvino" && mockInstalled.has("openvino")) {
      return [
        { target: { backend: "openvino", device: OV_CPU }, label: "12th Gen Intel Core i7-12700H (OpenVINO)", available: true, experimental: true },
        { target: { backend: "openvino", device: OV_GPU }, label: "Intel Iris Xe Graphics (OpenVINO)", available: true, experimental: true },
      ];
    }
    return [o];
  });
}
const mockStatusListeners = new Set<(s: InferenceStatus) => void>();

/** A deep copy as plain data (the UI may hand over reactive proxies). */
const plain = <T,>(value: T): T => JSON.parse(JSON.stringify(value));

const sameTarget = (a?: Target | null, b?: Target | null) =>
  !!a && !!b && a.backend === b.backend && (a.device ?? null) === (b.device ?? null);

function mockEngine(engine: EngineName): EngineStatus {
  const onBattery = mockSettings.autoSwitch && mockPower === "battery";
  const profile = onBattery ? mockSettings.onBattery : mockSettings.pluggedIn;
  const wanted = mockOverride ?? (profile.mode === "manual" ? profile.target ?? null : null);
  const options = mockOptionsNow();
  const option = (t: Target) => options.find((o) => sameTarget(o.target, t));
  const automatic = onBattery ? mockOptions[1] : mockOptions[2];
  const chosen = wanted ? option(wanted) : automatic;
  const usable = chosen?.available ? chosen : mockSettings.allowFallback ? automatic : undefined;
  const label = (o?: TargetOption) => o?.label ?? null;
  return {
    engine,
    model: engine === "chat" ? "qwen2.5-3b-instruct-q4" : "nomic-embed-text-v1.5",
    state: usable ? "ready" : "error",
    requested: wanted ? label(chosen) ?? "Unavailable" : onBattery ? "Automatic (battery saving)" : "Automatic (performance)",
    requestedTarget: wanted,
    active: label(usable),
    activeTarget: usable?.target ?? null,
    activeBackend: usable?.target.backend ?? null,
    activeDevice: usable?.label.replace(/ \([^)]*\)$/, "") ?? "CPU",
    verified: !!usable,
    detail: usable ? "2,005 MiB of the model is on the device." : null,
    fallbackUsed: !!usable && usable !== chosen,
    fallbackReason: usable && usable !== chosen ? chosen?.reason ?? "it is not available" : null,
    pending: false,
    notes:
      engine === "chat" && wanted?.backend === "openvino"
        ? ["OpenVINO is experimental in llama.cpp: some models fail or answer less accurately than on other engines."]
        : [],
    error: usable ? null : `${label(chosen)}: ${chosen?.reason ?? "unavailable"}`,
    hint: null,
  };
}

function mockInference(): InferenceOverview {
  const chat = mockEngine("chat");
  const profile = mockOverride ? "override" : mockSettings.autoSwitch && mockPower === "battery" ? "onBattery" : "pluggedIn";
  const ovInstalled = mockInstalled.has("openvino");
  const cudaInstalled = mockInstalled.has("cuda");
  return plain({
    settings: mockSettings,
    options: mockOptionsNow(),
    downloads: [
      { backend: "cuda" as const, sizeBytes: 592_700_000, note: "CUDA 13.4 for NVIDIA graphics cards; needs a current NVIDIA driver.", installed: cudaInstalled, blocked: null },
      { backend: "openvino" as const, sizeBytes: 104_700_000, note: "OpenVINO 2026.4.1 for Intel processors, graphics and NPUs. Experimental.", installed: ovInstalled, blocked: null },
    ],
    inventory: {
      cpu: "12th Gen Intel(R) Core(TM) i7-12700H",
      revision: "b12110",
      hardware: [
        { name: "NVIDIA GeForce RTX 4050 Laptop GPU", kind: "gpu" as const, vendor: "nvidia", driver: "32.0.16.2101" },
        { name: "Intel(R) Iris(R) Xe Graphics", kind: "gpu" as const, vendor: "intel", driver: "31.0.101.4502" },
      ],
      backends: [
        cudaInstalled
          ? {
              backend: "cuda" as const,
              status: "available" as const,
              experimental: false,
              detail: null,
              devices: [{ key: NVIDIA, name: "NVIDIA GeForce RTX 4050 Laptop GPU", kind: "gpu" as const, vendor: "nvidia", discrete: true, memoryMib: 5920, runtimeId: "CUDA0" }],
            }
          : { backend: "cuda" as const, status: "notInstalled" as const, experimental: false, devices: [], detail: "The CUDA engine is an optional download." },
        {
          backend: "vulkan" as const,
          status: "available" as const,
          experimental: false,
          detail: null,
          devices: [
            { key: IRIS, name: "Intel(R) Iris(R) Xe Graphics", kind: "gpu" as const, vendor: "intel", discrete: false, memoryMib: 16193, runtimeId: "Vulkan0" },
            { key: NVIDIA, name: "NVIDIA GeForce RTX 4050 Laptop GPU", kind: "gpu" as const, vendor: "nvidia", discrete: true, memoryMib: 5920, runtimeId: "Vulkan1" },
          ],
        },
        ovInstalled
          ? {
              backend: "openvino" as const,
              status: "available" as const,
              experimental: true,
              detail: "No NPU was found by OpenVINO (it needs an Intel Core Ultra processor and the Intel NPU driver).",
              devices: [
                { key: OV_CPU, name: "12th Gen Intel(R) Core(TM) i7-12700H", kind: "cpu" as const, vendor: "intel", discrete: false, memoryMib: null, runtimeId: "CPU" },
                { key: OV_GPU, name: "Intel(R) Iris(R) Xe Graphics (iGPU)", kind: "gpu" as const, vendor: "intel", discrete: false, memoryMib: 16193, runtimeId: "GPU" },
              ],
            }
          : { backend: "openvino" as const, status: "notInstalled" as const, experimental: true, devices: [], detail: "The OpenVINO engine is an optional download." },
        { backend: "cpu" as const, status: "available" as const, experimental: false, devices: [], detail: null },
      ],
    },
    status: {
      powerSource: mockPower,
      profile: profile as InferenceStatus["profile"],
      switching: false,
      pendingSwitch: false,
      overrideTarget: mockOverride,
      chat,
      embed: mockEngine("embed"),
      message: chat.error ?? (chat.fallbackUsed ? `${chat.requested} failed — fallback to ${chat.active}` : chat.active ?? ""),
    },
  });
}

function mockChanged(): InferenceOverview {
  const overview = mockInference();
  mockStatusListeners.forEach((cb) => cb(overview.status));
  return overview;
}

/** Engines, devices, the saved choices and what is running right now. */
export async function getInference(): Promise<InferenceOverview> {
  if (isTauri()) return invoke<InferenceOverview>("get_inference");
  return mockInference();
}

/** Save the plugged-in and battery choices and apply them. */
export async function setInferenceSettings(settings: PowerSettings): Promise<InferenceOverview> {
  if (isTauri()) return invoke<InferenceOverview>("set_inference_settings", { settings });
  mockSettings = plain(settings);
  return mockChanged();
}

/** "Try another engine now": run the chat model on `target` until called with null. */
export async function setEngineOverride(target: Target | null): Promise<InferenceOverview> {
  if (isTauri()) return invoke<InferenceOverview>("set_engine_override", { target });
  mockOverride = plain(target);
  return mockChanged();
}

/** Look for engines and devices again, forget earlier failures, and try again. */
export async function retryEngines(): Promise<InferenceOverview> {
  if (isTauri()) return invoke<InferenceOverview>("retry_engines");
  return mockChanged();
}

/** The last lines a model server logged. */
/** What each part of Loft is using right now. */
export async function getUsage(): Promise<Usage> {
  if (isTauri()) return invoke<Usage>("get_usage");
  // Preview: a chat model on a graphics card, the embedder on the processor.
  const GB = 1024 ** 3;
  const wobble = () => Math.round(Math.random() * 40) / 10;
  return {
    parts: [
      {
        kind: "chat",
        label: "Chat model",
        model: "Qwen 2.5 3B Instruct",
        runsOn: "NVIDIA GeForce RTX 4050 Laptop GPU (CUDA)",
        memoryBytes: 0.42 * GB,
        cpuPercent: wobble(),
        device: { name: "NVIDIA GeForce RTX 4050 Laptop GPU", bytes: 3.2 * GB, totalBytes: 6 * GB, shared: false },
        weightsBytes: 1.95 * GB,
        contextBytes: 1.45 * GB,
        contextTokens: 32768,
        processes: 1,
      },
      {
        kind: "embed",
        label: "Embedding model",
        model: "Nomic Embed Text v1.5",
        runsOn: "CPU only",
        memoryBytes: 0.55 * GB,
        cpuPercent: 0,
        weightsBytes: 0.26 * GB,
        contextBytes: 0.24 * GB,
        contextTokens: 8192,
        processes: 1,
      },
      { kind: "app", label: "Loft and its window", memoryBytes: 0.31 * GB, cpuPercent: wobble(), processes: 6 },
    ],
    memoryBytes: 1.28 * GB,
    deviceBytes: 3.2 * GB,
    cpuPercent: 3.1,
    systemTotalBytes: 31.7 * GB,
    systemAvailableBytes: 14.2 * GB,
    cores: 20,
  };
}

export async function getEngineLog(engine: EngineName): Promise<string[]> {
  if (isTauri()) return invoke<string[]>("get_engine_log", { engine });
  return [`(preview) ${engine} server log`, "load_tensors:      Vulkan1 model buffer size =  2005.12 MiB"];
}

/** Fires when an engine starts, switches, falls back or fails, or the power source changes. */
export async function onInferenceStatus(cb: (s: InferenceStatus) => void): Promise<Unsub> {
  if (!isTauri()) {
    mockStatusListeners.add(cb);
    return () => mockStatusListeners.delete(cb);
  }
  const { listen } = await import("@tauri-apps/api/event");
  return listen<InferenceStatus>("inference:status", (e) => cb(e.payload));
}

/** Download and install an optional engine (CUDA, OpenVINO); progress arrives through `onEngineInstall`. */
export async function installEngine(backend: Backend): Promise<InferenceOverview> {
  if (isTauri()) return invoke<InferenceOverview>("install_engine", { backend });
  const total = backend === "cuda" ? 592_700_000 : 104_700_000;
  mockInstallCancelled = false;
  for (let step = 1; step <= 12; step++) {
    await sleep(160);
    if (mockInstallCancelled) {
      mockInstallListeners.forEach((cb) => cb({ backend, stage: null, received: 0, total, done: true, error: "Cancelled." }));
      throw "Cancelled.";
    }
    mockInstallListeners.forEach((cb) => cb({ backend, stage: step < 11 ? "downloading" : "unpacking", received: Math.min(total, (total * step) / 10), total, done: false }));
  }
  mockInstalled.add(backend);
  mockInstallListeners.forEach((cb) => cb({ backend, stage: null, received: total, total, done: true }));
  return mockChanged();
}

let mockInstallCancelled = false;
const mockInstallListeners = new Set<(p: EngineInstallProgress) => void>();

export async function cancelEngineInstall(): Promise<void> {
  if (isTauri()) return invoke<void>("cancel_engine_install");
  mockInstallCancelled = true;
}

/** Remove an optional engine that was installed from Settings. */
export async function removeEngine(backend: Backend): Promise<InferenceOverview> {
  if (isTauri()) return invoke<InferenceOverview>("remove_engine", { backend });
  mockInstalled.delete(backend);
  if (mockOverride?.backend === backend) mockOverride = null;
  return mockChanged();
}

export async function onEngineInstall(cb: (p: EngineInstallProgress) => void): Promise<Unsub> {
  if (!isTauri()) {
    mockInstallListeners.add(cb);
    return () => mockInstallListeners.delete(cb);
  }
  const { listen } = await import("@tauri-apps/api/event");
  return listen<EngineInstallProgress>("engine:install", (e) => cb(e.payload));
}

/**
 * "Try another engine now" / "Try NPU now": run the chat model on `target`
 * (temporarily), confirm where it really runs, and ask it a short question.
 */
export async function tryEngine(target: Target): Promise<{ trial: EngineTrial; overview: InferenceOverview }> {
  if (isTauri()) return invoke<{ trial: EngineTrial; overview: InferenceOverview }>("try_engine", { target });
  await sleep(700);
  mockOverride = plain(target);
  const overview = mockChanged();
  const chat = overview.status.chat;
  const speed = target.backend === "cpu" ? 8.2 : target.device === NVIDIA ? 56 : 9.6;
  return {
    overview,
    trial: {
      requested: chat.requested,
      active: chat.active,
      ok: true,
      pending: false,
      verified: true,
      detail: chat.detail,
      fallbackUsed: false,
      note: chat.notes[0] ?? null,
      answer: "Running an AI model locally means the model's files and all of its computation stay on your own computer. Nothing you ask is sent to an online service.",
      firstTokenMs: Math.round(9000 / speed),
      totalMs: Math.round(9000 / speed + 34000 / speed),
      tokensPerSecond: speed,
      answerTokens: 34,
    },
  };
}

function mockReport(): BenchReport {
  const current = mockBench.filter((r) => r.ok);
  const best = [...current].sort((a, b) => (b.tokensPerSecond ?? 0) - (a.tokensPerSecond ?? 0));
  return plain({
    model: "qwen2.5-3b-instruct-q4",
    running: false,
    rows: mockBench,
    unmeasured: mockBench.length
      ? mockOptionsNow()
          .filter((o) => o.available && !mockBench.some((r) => sameTarget(r.target, o.target)))
          .map((o) => o.label)
      : [],
    advice: {
      pluggedIn: best[0]
        ? {
            target: best[0].target,
            label: best[0].label,
            reason: `Fastest in the test: ${Math.round(best[0].tokensPerSecond ?? 0)} tokens/s${best[1] ? ` (next: ${best[1].label}, ${(best[1].tokensPerSecond ?? 0).toFixed(1)} tokens/s)` : ""}.`,
          }
        : null,
      onBattery: null,
      batteryNote:
        "Power efficiency is unmeasured: energy can only be measured while running on battery. Unplug and test again to compare engines by battery use; until then Automatic on battery keeps to its rule of thumb (it stays off the discrete graphics card).",
    },
  });
}

const mockBenchListeners = new Set<(p: BenchProgress) => void>();

/** The stored measurements for the selected chat model, and what they suggest. */
export async function getBenchmarks(): Promise<BenchReport> {
  if (isTauri()) return invoke<BenchReport>("get_benchmarks");
  return mockReport();
}

/** "Test performance": measure every available engine with the selected chat model. */
export async function runBenchmark(targets?: Target[]): Promise<BenchReport> {
  if (isTauri()) return invoke<BenchReport>("run_benchmark", { targets: targets ?? null });
  const options = mockOptionsNow().filter((o) => o.available);
  mockBench = [];
  for (const [index, o] of options.entries()) {
    const label = o.label;
    for (const stage of ["loading", "measuring"] as const) {
      mockBenchListeners.forEach((cb) => cb({ running: true, index, total: options.length, label, stage }));
      await sleep(450);
    }
    const speed = o.target.backend === "cpu" ? 8.1 : o.target.device === NVIDIA ? (o.target.backend === "cuda" ? 61.4 : 55.2) : 9.6;
    mockBench.push({
      target: o.target,
      label,
      at: Math.floor(Date.now() / 1000),
      powerSource: mockPower,
      ok: true,
      verified: true,
      actual: o.target.backend === "cpu" ? "The model is held in system memory; nothing was offloaded." : "1835 MiB of the model is on the device.",
      loadMs: o.target.backend === "cpu" ? 900 : 2600,
      firstTokenMs: Math.round(42000 / speed),
      totalMs: Math.round(42000 / speed + 96000 / speed),
      tokensPerSecond: speed,
      answerTokens: 96,
      peakMemoryMib: o.target.backend === "cpu" ? 2380 : 640,
      deviceMemoryMib: o.target.backend === "cpu" ? null : 2211,
      identity: { modelId: "qwen2.5-3b-instruct-q4", context: 32768, revision: "b12110", device: o.target.device ? o.label.replace(/ \([^)]*\)$/, "") : null, driver: null },
      stale: null,
    });
  }
  mockBenchListeners.forEach((cb) => cb({ running: false, index: options.length, total: options.length, label: "", stage: "done" }));
  return mockReport();
}

export async function cancelBenchmark(): Promise<void> {
  if (isTauri()) return invoke<void>("cancel_benchmark");
}

export async function onBenchProgress(cb: (p: BenchProgress) => void): Promise<Unsub> {
  if (!isTauri()) {
    mockBenchListeners.add(cb);
    return () => mockBenchListeners.delete(cb);
  }
  const { listen } = await import("@tauri-apps/api/event");
  return listen<BenchProgress>("bench:progress", (e) => cb(e.payload));
}

/** Development builds only: pretend the power source changed. */
export async function devSetPower(source: PowerSource): Promise<void> {
  if (isTauri()) return invoke<void>("dev_set_power", { source });
  mockPower = source;
  mockChanged();
}

/** The open panel's size (logical px); it depends on the display when large. */
export interface PanelSize {
  width: number;
  height: number;
}

const NORMAL_PANEL: PanelSize = { width: 460, height: 520 };
/** Preview: twice the normal size, kept inside the browser window. */
const previewPanel = (large: boolean): PanelSize =>
  large
    ? { width: Math.max(460, Math.min(920, window.innerWidth - 80)), height: Math.max(520, Math.min(1040, window.innerHeight - 80)) }
    : NORMAL_PANEL;

/** Grow the native window before the panel opens / shrink it after it closes. */
export async function setExpanded(expanded: boolean): Promise<PanelSize> {
  if (isTauri()) return ownMove(() => invoke<PanelSize>("set_expanded", { expanded }));
  return previewPanel(mockWindow.large);
}

/** The larger panel on or off (remembered); resizes the window. Returns the panel's size. */
export async function setLarge(large: boolean, expanded: boolean): Promise<PanelSize> {
  if (isTauri()) return ownMove(() => invoke<PanelSize>("set_large", { large, expanded }));
  mockWindow.large = large;
  return previewPanel(large);
}

/** The panel's size changed (another display, another edge). */
export async function onPanelSize(cb: (size: PanelSize) => void): Promise<Unsub> {
  if (!isTauri()) return () => {};
  const { listen } = await import("@tauri-apps/api/event");
  return listen<PanelSize>("loft://panel-size", (e) => cb(e.payload));
}

// Loft moves and resizes its own window (opening, closing, changing edge or
// style). Those moves must not be mistaken for the user dragging it.
let ownMoveAt = 0;
async function ownMove<T>(action: () => Promise<T>): Promise<T> {
  ownMoveAt = Date.now();
  try {
    return await action();
  } finally {
    ownMoveAt = Date.now();
  }
}
/** True if Loft itself moved the window in the last moment. */
export const movedByLoft = (withinMs = 900): boolean => Date.now() - ownMoveAt < withinMs;

export interface WindowSettings {
  style: "notch" | "floating";
  /** No pill on screen: Loft is opened from its tray icon or the hotkey. */
  trayOnly: boolean;
  /** This desktop gave Loft a tray (menu bar) icon. */
  trayAvailable: boolean;
  /** The panel opens twice as large. */
  large: boolean;
}

let mockWindow: WindowSettings = { style: "notch", trayOnly: false, trayAvailable: true, large: false };

/** How Loft appears: notch or floating, and whether it lives in the tray. */
export async function getWindowSettings(): Promise<WindowSettings> {
  if (isTauri()) return invoke<WindowSettings>("get_window_settings");
  return { ...mockWindow };
}

/** Switch between the docked notch and the floating window, and remember it. */
export async function setWindowStyle(style: "notch" | "floating", expanded: boolean): Promise<void> {
  if (isTauri()) return ownMove(() => invoke<void>("set_window_style", { style, expanded }));
  mockWindow.style = style;
}

/** Live in the tray (nothing on screen until asked for), or not. Returns the resulting state. */
export async function setTrayOnly(enabled: boolean, expanded: boolean): Promise<boolean> {
  if (isTauri()) return ownMove(() => invoke<boolean>("set_tray_only", { enabled, expanded }));
  mockWindow.trayOnly = enabled;
  return enabled;
}

/** Floating: remember where the window is now (it was just dragged there). */
export async function rememberPosition(): Promise<void> {
  if (isTauri()) return invoke<void>("remember_position");
}

/** Let the user drag the (floating) window; ends when the mouse button is released. */
export async function startWindowDrag(): Promise<void> {
  if (!isTauri()) return;
  const { getCurrentWindow } = await import("@tauri-apps/api/window");
  await getCurrentWindow().startDragging();
}

/** Calls back whenever the window has been moved (by anyone). */
export async function onWindowMoved(cb: () => void): Promise<Unsub> {
  if (!isTauri()) return () => {};
  const { getCurrentWindow } = await import("@tauri-apps/api/window");
  return getCurrentWindow().onMoved(() => cb());
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
  if (isTauri()) return ownMove(() => invoke<void>("set_placement", { placement, expanded }));
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
  if (isTauri()) return ownMove(() => invoke<void>("set_monitor", { id, expanded }));
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
  saving?: {
    /** The conversation this question belongs to; null starts a new one. */
    conversation: string | null;
    /** Keep the turn in the chat history. */
    save: boolean;
    /** The question has been stored: here is its conversation. */
    onSaved?: (conversation: Conversation) => void;
  },
): Promise<AskResult> {
  if (isTauri()) {
    const { listen } = await import("@tauri-apps/api/event");
    // Every event of this answer carries this id, and nothing else is
    // listened to: text from an answer that was just stopped cannot land here.
    const request = crypto.randomUUID();
    let answer = "";
    const unToken = await listen<{ request: string; text: string }>("ask:token", (e) => {
      if (e.payload.request !== request) return;
      answer += e.payload.text;
      onToken(e.payload.text);
    });
    const unSources = await listen<{ request: string; sources: Source[] }>("ask:sources", (e) => {
      if (e.payload.request === request) onSources?.(e.payload.sources);
    });
    const unSaved = await listen<{ request: string; conversation: Conversation }>("ask:saved", (e) => {
      if (e.payload.request === request) saving?.onSaved?.(e.payload.conversation);
    });
    try {
      const done = await invoke<Omit<AskResult, "answer">>("ask", {
        question,
        history,
        options,
        request,
        conversation: saving?.conversation ?? null,
        save: saving?.save ?? false,
      });
      return { answer, ...done };
    } finally {
      unToken();
      unSources();
      unSaved();
    }
  }
  // Mock: sources first, then a streamed answer.
  mockAskStopped = false;
  const mockSaved = saving?.save || saving?.conversation ? mockBeginTurn(saving?.conversation ?? null, question) : null;
  if (mockSaved) saving?.onSaved?.(plain(mockSaved.conversation));
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
  if (mockAskStopped) {
    return { answer, sources, status: "stopped", conversation: mockFinishTurn(mockSaved, answer, sources, null, "stopped") };
  }
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
  return { answer, sources, stats, status: "complete", conversation: mockFinishTurn(mockSaved, answer, sources, stats, "complete") };
}

// ---- Chat history -----------------------------------------------------------

/** Preview data: conversations kept in memory. */
const mockChats: ConversationView[] = [];
let mockChatId = 0;

function mockTitle(question: string): string {
  const tidy = question.trim().replace(/\s+/g, " ").replace(/[?.!: ]+$/, "");
  const cut = tidy.length > 48 ? `${tidy.slice(0, 48).replace(/\s+\S*$/, "")}…` : tidy;
  return cut ? cut[0].toUpperCase() + cut.slice(1) : "New conversation";
}

function mockBeginTurn(conversation: string | null, question: string) {
  const now = Date.now();
  let view = mockChats.find((c) => c.conversation.id === conversation);
  if (!view) {
    view = {
      conversation: { id: `mock-${++mockChatId}`, title: mockTitle(question), titleManual: false, createdAt: now, updatedAt: now, openedAt: now, pinned: false, archived: false, questions: 0 },
      messages: [],
    };
    mockChats.push(view);
  }
  view.messages.push({ id: `m${now}u`, role: "user", content: question, createdAt: now, status: "complete", sources: [] });
  const message: SavedMessage = { id: `m${now}a`, role: "assistant", content: "", createdAt: now, status: "pending", sources: [] };
  view.messages.push(message);
  view.conversation.questions += 1;
  view.conversation.updatedAt = now;
  return { conversation: view.conversation, message };
}

function mockFinishTurn(
  saved: ReturnType<typeof mockBeginTurn> | null,
  answer: string,
  sources: Source[],
  stats: AskStats | null,
  status: "complete" | "stopped",
): Conversation | null {
  if (!saved) return null;
  Object.assign(saved.message, { content: answer, sources: sources.map((s) => ({ ...s, state: "same" as const })), stats, status });
  saved.conversation.updatedAt = Date.now();
  return plain(saved.conversation);
}

/** The saved conversations, most recently updated first; with `query`, those that mention it. */
export async function listConversations(query?: string): Promise<{ conversations: Conversation[]; total: number }> {
  if (isTauri()) return invoke<{ conversations: Conversation[]; total: number }>("list_conversations", { query: query ?? null });
  const q = (query ?? "").trim().toLowerCase();
  const all = [...mockChats].sort((a, b) => b.conversation.updatedAt - a.conversation.updatedAt);
  const matching = q ? all.filter((c) => c.conversation.title.toLowerCase().includes(q) || c.messages.some((m) => m.content.toLowerCase().includes(q))) : all;
  return plain({ conversations: matching.map((c) => c.conversation), total: all.length });
}

/** One conversation in full, as it was saved. */
export async function getConversation(id: string): Promise<ConversationView> {
  if (isTauri()) return invoke<ConversationView>("get_conversation", { id });
  const view = mockChats.find((c) => c.conversation.id === id);
  if (!view) throw "That conversation no longer exists.";
  return plain(view);
}

export async function renameConversation(id: string, title: string): Promise<Conversation> {
  if (isTauri()) return invoke<Conversation>("rename_conversation", { id, title });
  const view = mockChats.find((c) => c.conversation.id === id);
  const tidy = title.trim().replace(/\s+/g, " ");
  if (!view) throw "That conversation no longer exists.";
  if (!tidy) throw "A conversation needs a name.";
  Object.assign(view.conversation, { title: tidy, titleManual: true });
  return plain(view.conversation);
}

/** Delete a conversation and its messages. Files and their index are not touched. */
export async function deleteConversation(id: string): Promise<void> {
  if (isTauri()) return invoke<void>("delete_conversation", { id });
  const at = mockChats.findIndex((c) => c.conversation.id === id);
  if (at >= 0) mockChats.splice(at, 1);
}

/** Bring the one conversation an older Loft kept in the webview's storage into the history. */
export async function importConversation(turns: { question: string; answer: string; sources: Source[]; stats: AskStats | null; error: string }[]): Promise<Conversation> {
  if (isTauri()) return invoke<Conversation>("import_conversation", { turns });
  let saved: ReturnType<typeof mockBeginTurn> | null = null;
  for (const turn of turns) {
    saved = mockBeginTurn(saved?.conversation.id ?? null, turn.question);
    mockFinishTurn(saved, turn.answer, turn.sources, turn.stats, "complete");
  }
  if (!saved) throw "There is nothing to import.";
  return plain(saved.conversation);
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
