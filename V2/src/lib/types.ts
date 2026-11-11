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
}

export type RuntimeState = "stopped" | "loading" | "ready" | "error";

export interface RuntimeStatus {
  chat: RuntimeState;
  embed: RuntimeState;
  accelerator: "cpu" | "metal" | "cuda" | "vulkan";
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
