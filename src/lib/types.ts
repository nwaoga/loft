// Shared types mirrored by the Rust backend (serde) and the UI.

/** "vision" models describe pictures while indexing; optional, off by default. */
export type ModelType = "chat" | "embed" | "vision";

export interface ModelInfo {
  id: string;
  name: string;
  type: ModelType;
  params: string; // e.g. "3B", "137M"
  quant: string; // e.g. "Q4_K_M"
  sizeBytes: number;
  ctx: number;
  installed: boolean;
  selected: boolean;
  /** 0..1 while downloading, else null */
  downloadProgress: number | null;
}

export type FolderStatus =
  | "idle" // index deleted by the user; not indexed until they ask
  | "pending" // waiting to be indexed
  | "indexing"
  | "ready"
  | "error";

export interface FolderInfo {
  id: string;
  path: string;
  status: FolderStatus;
  fileCount: number;
  chunkCount: number;
  /** present while status === "indexing" */
  progress?: { done: number; total: number; current: string };
  error?: string;
}

export interface Source {
  file: string;
  path: string;
  page?: number | null;
  /** "page 3" / "slide 2" / "sheet 1" where known */
  location?: string | null;
  score: number;
  snippet: string;
  /**
   * For a citation in a saved conversation: what has become of the file
   * since the answer was written. Absent for an answer just given.
   */
  state?: "same" | "changed" | "missing";
}

/** A saved conversation, as listed in the chat history. */
export interface Conversation {
  id: string;
  title: string;
  /** The user named it. */
  titleManual: boolean;
  /** Milliseconds since 1970, UTC. */
  createdAt: number;
  updatedAt: number;
  openedAt?: number | null;
  pinned: boolean;
  archived: boolean;
  /** Questions asked in it. */
  questions: number;
}

/** How an answer ended. Only "complete" is a finished answer. */
export type MessageStatus = "pending" | "complete" | "stopped" | "failed" | "interrupted";

/** One saved message of a conversation. */
export interface SavedMessage {
  id: string;
  role: "user" | "assistant";
  content: string;
  createdAt: number;
  status: MessageStatus;
  model?: string | null;
  stats?: AskStats | null;
  error?: string | null;
  sources: Source[];
}

export interface ConversationView {
  conversation: Conversation;
  messages: SavedMessage[];
}

export type RuntimeState = "stopped" | "loading" | "ready" | "error";

export interface RuntimeStatus {
  chat: RuntimeState;
  embed: RuntimeState;
  accelerator: "cpu" | "metal" | "cuda" | "vulkan" | "openvino";
  message?: string | null;
}

/** An earlier question and its answer, sent along with a follow-up. */
export interface ChatTurn {
  question: string;
  answer: string;
}

/** How long answers should be (chat options). */
export type AnswerLength = "concise" | "balanced" | "detailed";

/** How much of the model's context window one answer used. */
export interface AskStats {
  /** Tokens sent to the model: rules, earlier turns, excerpts, question. */
  promptTokens: number;
  /** Tokens in the answer. */
  answerTokens: number;
  /** The model's context window in tokens, when known. */
  contextSize?: number | null;
  tokensPerSecond?: number | null;
  /** Earlier turns sent along with the question. */
  turnsSent: number;
  /** Earlier turns left out because the conversation memory was full. */
  turnsDropped: number;
  /** Excerpts from the files that were sent. */
  excerpts: number;
}

export interface AskResult {
  answer: string;
  sources: Source[];
  /** Absent when the model was not called (nothing found) or was stopped. */
  stats?: AskStats | null;
  /** How the answer ended: complete, or stopped. */
  status?: MessageStatus;
  /** The conversation the turn was saved in; absent when it was not saved. */
  conversation?: Conversation | null;
  /** The turn could not be saved, and why. */
  saveError?: string | null;
}

/** An attached display, for the picker in Settings. */
export interface MonitorInfo {
  id: string;
  /** "Display 2" */
  label: string;
  width: number;
  height: number;
  primary: boolean;
  /** The one the notch is on now. */
  selected: boolean;
}

// ---- Performance & Battery (mirrors src-tauri/src/inference.rs and engine.rs)

/** A llama.cpp build: the software path a model runs through. */
export type Backend = "cuda" | "vulkan" | "openvino" | "metal" | "cpu";
export type PowerSource = "ac" | "battery" | "unknown";
export type EngineName = "chat" | "embed";

/** What to run a model on: a backend and, for GPU and NPU backends, a device. */
export interface Target {
  backend: Backend;
  /** A stable device key; absent = the backend's best device, or the CPU. */
  device?: string | null;
}

/** The choice for one power state. */
export interface InferenceProfile {
  mode: "automatic" | "manual";
  target?: Target | null;
  /** Advanced: a different target for the embedding model. */
  embedTarget?: Target | null;
}

export interface PowerSettings {
  pluggedIn: InferenceProfile;
  onBattery: InferenceProfile;
  /** Use the battery choice while on battery. Off: the plugged-in choice always. */
  autoSwitch: boolean;
  /** If the chosen engine cannot run the model, try another one. */
  allowFallback: boolean;
}

/** Something selectable in an engine dropdown. */
export interface TargetOption {
  target: Target;
  label: string;
  available: boolean;
  experimental: boolean;
  /** Why it cannot be used right now. */
  reason?: string | null;
}

export interface EngineDevice {
  key: string;
  name: string;
  kind: "gpu" | "npu" | "cpu";
  vendor: string;
  discrete: boolean;
  memoryMib?: number | null;
  runtimeId: string;
}

export interface EngineBackend {
  backend: Backend;
  status: "available" | "notInstalled" | "hardwareUnavailable" | "driverMissing" | "failed";
  experimental: boolean;
  devices: EngineDevice[];
  detail?: string | null;
}

export interface EngineInventory {
  cpu: string;
  hardware: { name: string; kind: "gpu" | "npu" | "cpu"; vendor: string; driver?: string | null }[];
  backends: EngineBackend[];
  /** The llama.cpp revision the engine builds come from. */
  revision: string;
}

/** One model server: what was asked for and what is really running. */
export interface EngineStatus {
  engine: EngineName;
  model?: string | null;
  state: RuntimeState;
  requested: string;
  requestedTarget?: Target | null;
  active?: string | null;
  activeTarget?: Target | null;
  activeBackend?: Backend | null;
  activeDevice?: string | null;
  /** The device was confirmed from the server's own load log. */
  verified: boolean;
  detail?: string | null;
  fallbackUsed: boolean;
  fallbackReason?: string | null;
  /** A switch is waiting for work in progress to end. */
  pending: boolean;
  notes: string[];
  error?: string | null;
  /** What to do about a failure or a fallback, where there is something to do. */
  hint?: string | null;
}

export interface InferenceStatus {
  powerSource: PowerSource;
  /** Which choice is in force. */
  profile: "override" | "pluggedIn" | "onBattery";
  switching: boolean;
  pendingSwitch: boolean;
  overrideTarget?: Target | null;
  chat: EngineStatus;
  embed: EngineStatus;
  message: string;
}

/** An optional engine build that can be added to, or removed from, this computer. */
export interface EngineDownload {
  backend: Backend;
  sizeBytes: number;
  note?: string | null;
  /** It is in Loft's own engines folder, so it can be removed again. */
  installed: boolean;
  /** Why installing it here would be pointless, if it would. */
  blocked?: string | null;
}

export interface InferenceOverview {
  settings: PowerSettings;
  inventory: EngineInventory;
  options: TargetOption[];
  downloads: EngineDownload[];
  status: InferenceStatus;
}

/** Progress of an engine install (`engine:install`). */
export interface EngineInstallProgress {
  backend: Backend;
  stage?: "downloading" | "unpacking" | "installing" | null;
  received: number;
  total: number;
  done: boolean;
  error?: string | null;
}

/** Memory a model has on a device other than the processor. */
export interface DeviceUse {
  name: string;
  bytes: number;
  /** All the memory the device has, where the engine reports it. */
  totalBytes?: number | null;
  /** No memory of its own (built-in graphics, an NPU, Apple silicon): this is system memory, already in memoryBytes. */
  shared: boolean;
}

/** One part of Loft in the "What Loft is using" panel. */
export interface UsagePart {
  kind: "chat" | "embed" | "otherModel" | "app";
  label: string;
  model?: string | null;
  runsOn?: string | null;
  /** System memory the part's processes have in RAM right now. */
  memoryBytes?: number | null;
  cpuPercent?: number | null;
  device?: DeviceUse | null;
  weightsBytes?: number | null;
  /** Room for the conversation and excerpts, plus working memory. */
  contextBytes?: number | null;
  contextTokens?: number | null;
  processes: number;
}

export interface Usage {
  parts: UsagePart[];
  /** System memory used by all of Loft. */
  memoryBytes: number;
  /** Memory on graphics cards with memory of their own. */
  deviceBytes: number;
  cpuPercent?: number | null;
  systemTotalBytes?: number | null;
  systemAvailableBytes?: number | null;
  cores: number;
}

/** What happened when an engine was tried ("Try another engine now", "Try NPU now"). */
export interface EngineTrial {
  requested: string;
  active?: string | null;
  /** The requested engine is running the model and answered the test question. */
  ok: boolean;
  /** The switch is waiting for an answer in progress. */
  pending: boolean;
  verified: boolean;
  detail?: string | null;
  fallbackUsed: boolean;
  problem?: string | null;
  hint?: string | null;
  note?: string | null;
  answer?: string | null;
  firstTokenMs?: number | null;
  totalMs?: number | null;
  tokensPerSecond?: number | null;
  answerTokens?: number | null;
}

/** One engine measured by "Test performance". */
export interface BenchRow {
  target: Target;
  label: string;
  /** Seconds since 1970. */
  at: number;
  powerSource: PowerSource;
  ok: boolean;
  error?: string | null;
  /** The engine's own log confirmed where the model was loaded. */
  verified: boolean;
  actual?: string | null;
  loadMs?: number | null;
  firstTokenMs?: number | null;
  totalMs?: number | null;
  tokensPerSecond?: number | null;
  answerTokens?: number | null;
  peakMemoryMib?: number | null;
  deviceMemoryMib?: number | null;
  /** The whole computer's battery drain during the test, in watts. */
  drainWatts?: number | null;
  joulesPerToken?: number | null;
  identity: { modelId: string; context: number; revision: string; device?: string | null; driver?: string | null };
  /** Why the measurement no longer applies; absent = current. */
  stale?: string | null;
}

export interface BenchPick {
  target: Target;
  label: string;
  reason: string;
}

export interface BenchReport {
  model?: string | null;
  running: boolean;
  rows: BenchRow[];
  /** Usable engines with no current measurement. */
  unmeasured: string[];
  advice: { pluggedIn?: BenchPick | null; onBattery?: BenchPick | null; batteryNote: string };
}

/** Progress of a running performance test (`bench:progress`). */
export interface BenchProgress {
  running: boolean;
  index: number;
  total: number;
  label: string;
  stage: "loading" | "measuring" | "done";
}
