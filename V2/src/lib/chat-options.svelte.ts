// The "Chat & Answers" options: how answers are written and shown, and what
// the conversation remembers. Kept in the webview's local storage, so they
// survive a restart; nothing here leaves this computer.

import type { AnswerLength } from "./types";

export type Tone = "natural" | "professional" | "technical";
/** When the sources drawer under an answer is open. */
export type SourcePanel = "auto" | "expanded" | "collapsed";
/** How long the conversation is kept. */
export type ChatHistory = "none" | "session" | "local";

export interface ChatOptions {
  length: AnswerLength;
  tone: Tone;
  /** Show the files an answer was written from. */
  showCitations: boolean;
  sourcePanel: SourcePanel;
  /** Say so when the files don't hold the answer, rather than answering from general knowledge. */
  filesOnly: boolean;
  /** Show the answer as it is written, rather than all at once. */
  stream: boolean;
  /** Send earlier turns along, so follow-up questions are understood. */
  remember: boolean;
  history: ChatHistory;
  /** 0 = most predictable, 1 = most varied. */
  temperature: number;
  /** Hard ceiling on the length of an answer. */
  maxTokens: number;
}

export const DEFAULTS: ChatOptions = {
  length: "balanced",
  tone: "natural",
  showCitations: true,
  sourcePanel: "auto",
  filesOnly: true,
  stream: true,
  remember: true,
  history: "session",
  temperature: 0.2,
  maxTokens: 1024,
};

export const LENGTHS: { id: AnswerLength; name: string }[] = [
  { id: "concise", name: "Concise" },
  { id: "balanced", name: "Balanced" },
  { id: "detailed", name: "Detailed" },
];
export const TONES: { id: Tone; name: string; hint: string }[] = [
  { id: "natural", name: "Natural", hint: "conversational and clear" },
  { id: "professional", name: "Professional", hint: "structured and direct" },
  { id: "technical", name: "Technical", hint: "precise, with code details" },
];
export const SOURCE_PANELS: { id: SourcePanel; name: string; hint: string }[] = [
  { id: "auto", name: "Auto", hint: "show when sources are found" },
  { id: "expanded", name: "Always expanded", hint: "" },
  { id: "collapsed", name: "Collapsed by default", hint: "" },
];
export const HISTORIES: { id: ChatHistory; name: string }[] = [
  { id: "none", name: "Don't save" },
  { id: "session", name: "Current session only" },
  { id: "local", name: "Save locally" },
];
export const MAX_TOKENS: { value: number; name: string }[] = [
  { value: 256, name: "Short" },
  { value: 512, name: "Compact" },
  { value: 1024, name: "Balanced" },
  { value: 2048, name: "Long" },
];

const KEY = "loft.chatOptions";

/** Saved options, with anything missing or invalid replaced by its default. */
function load(): ChatOptions {
  let saved: Partial<ChatOptions> = {};
  try {
    saved = JSON.parse(localStorage.getItem(KEY) ?? "{}") ?? {};
  } catch {
    /* unreadable or unavailable: defaults */
  }
  const pick = <T,>(value: unknown, allowed: readonly T[], fallback: T): T =>
    allowed.includes(value as T) ? (value as T) : fallback;
  const flag = (value: unknown, fallback: boolean) => (typeof value === "boolean" ? value : fallback);
  const temperature = Number(saved.temperature);
  return {
    length: pick(saved.length, LENGTHS.map((l) => l.id), DEFAULTS.length),
    tone: pick(saved.tone, TONES.map((t) => t.id), DEFAULTS.tone),
    showCitations: flag(saved.showCitations, DEFAULTS.showCitations),
    sourcePanel: pick(saved.sourcePanel, SOURCE_PANELS.map((p) => p.id), DEFAULTS.sourcePanel),
    filesOnly: flag(saved.filesOnly, DEFAULTS.filesOnly),
    stream: flag(saved.stream, DEFAULTS.stream),
    remember: flag(saved.remember, DEFAULTS.remember),
    history: pick(saved.history, HISTORIES.map((h) => h.id), DEFAULTS.history),
    temperature: Number.isFinite(temperature) && saved.temperature != null ? Math.min(Math.max(temperature, 0), 1) : DEFAULTS.temperature,
    maxTokens: pick(saved.maxTokens, MAX_TOKENS.map((m) => m.value), DEFAULTS.maxTokens),
  };
}

export const chatOptions = $state<ChatOptions>(load());

/** Remember the options as they are now. Call after changing any of them. */
export function saveChatOptions() {
  try {
    localStorage.setItem(KEY, JSON.stringify(chatOptions));
  } catch {
    /* not remembered, but in force for this run */
  }
}

export function resetChatOptions() {
  Object.assign(chatOptions, DEFAULTS);
  saveChatOptions();
}

/** True when anything differs from the defaults. */
export function chatOptionsChanged(): boolean {
  return (Object.keys(DEFAULTS) as (keyof ChatOptions)[]).some((key) => chatOptions[key] !== DEFAULTS[key]);
}

/** The options the answer engine needs (the rest only affect the display). */
export function answerOptions() {
  return {
    length: chatOptions.length,
    tone: chatOptions.tone,
    filesOnly: chatOptions.filesOnly,
    temperature: chatOptions.temperature,
    maxTokens: chatOptions.maxTokens,
  };
}
export type AnswerOptions = ReturnType<typeof answerOptions>;

/** The settings as text, for "Copy settings". */
export function describeChatOptions(): string {
  return JSON.stringify({ app: "Loft", chatAndAnswers: chatOptions }, null, 2);
}
