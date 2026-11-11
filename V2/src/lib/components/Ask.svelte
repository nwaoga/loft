<script lang="ts">
  import { tick } from "svelte";
  import { ask, cancelAsk, openSource } from "../api";
  import { ui, type Tab } from "../state.svelte";
  import type { AskStats, Source } from "../types";
  import { fileTag } from "../format";
  import { chatOptions, answerOptions, chatOptionsChanged } from "../chat-options.svelte";
  import Icon from "./Icon.svelte";
  import ChatOptions from "./ChatOptions.svelte";

  let {
    ready = true,
    hint = null,
  }: {
    ready?: boolean;
    /** What still has to happen before answers work, and the tab that fixes it. */
    hint?: { text: string; tab?: Tab } | null;
  } = $props();

  /** One question and its answer, as shown in the conversation. */
  interface TurnView {
    id: number;
    question: string;
    answer: string;
    sources: Source[];
    /** The sources drawer as the user left it; `null` = follow the "Source panel" option. */
    sourcesOpen: boolean | null;
    /** Context usage of this answer, once it has finished. */
    stats: AskStats | null;
    error: string;
  }

  // The conversation. Each new question is sent with the turns before it, so
  // a follow-up ("and when does it start?") is understood. "New chat" clears it.
  // (Declared before `turns`, which is restored from here at start-up.)
  const CHAT_KEY = "loft.chat";
  const MAX_SAVED_TURNS = 40;
  let nextId = 0;
  let turns = $state<TurnView[]>(savedChat());
  let question = $state("");
  let openError = $state("");
  let inputEl: HTMLTextAreaElement | undefined = $state();
  let threadEl: HTMLDivElement | undefined = $state();
  // Follow the answer as it grows, unless the user has scrolled up to read.
  let stick = true;

  // "Chat & Answers" options: a sheet over the conversation, opened from the
  // button beside Send. Esc closes it (and must not also close the panel).
  let optionsOpen = $state(false);
  $effect(() => {
    if (!optionsOpen) return;
    // Caught on the way down, wherever the focus is, so that the panel's own
    // Esc handling never sees it.
    const onKey = (e: KeyboardEvent) => {
      if (e.key !== "Escape") return;
      optionsOpen = false;
      e.stopPropagation();
    };
    window.addEventListener("keydown", onKey, true);
    return () => window.removeEventListener("keydown", onKey, true);
  });

  // Chat history option. "Save locally" keeps the conversation in the
  // webview's local storage and brings it back at the next launch; "Don't
  // save" forgets it whenever the panel is closed; otherwise it lasts until
  // Loft quits.
  function savedChat(): TurnView[] {
    if (chatOptions.history !== "local") return [];
    try {
      const saved = JSON.parse(localStorage.getItem(CHAT_KEY) ?? "[]");
      if (!Array.isArray(saved)) return [];
      return saved
        .filter((t) => t && typeof t.question === "string" && typeof t.answer === "string")
        .map((t) => ({
          id: nextId++,
          question: t.question,
          answer: t.answer,
          sources: Array.isArray(t.sources) ? t.sources : [],
          sourcesOpen: null,
          stats: t.stats ?? null,
          error: typeof t.error === "string" ? t.error : "",
        }));
    } catch {
      return [];
    }
  }
  $effect(() => {
    try {
      if (chatOptions.history !== "local") {
        localStorage.removeItem(CHAT_KEY);
      } else if (!ui.asking) {
        const keep = turns.slice(-MAX_SAVED_TURNS).map(({ question, answer, sources, stats, error }) => ({ question, answer, sources, stats, error }));
        localStorage.setItem(CHAT_KEY, JSON.stringify(keep));
      }
    } catch {
      /* storage unavailable: the conversation simply is not kept */
    }
  });
  $effect(() => {
    if (chatOptions.history === "none" && !ui.expanded && !ui.asking && turns.length) turns = [];
  });

  /** Whether a turn's sources drawer is open, given the "Source panel" option. */
  function sourcesOpen(t: TurnView, last: boolean): boolean {
    if (t.sourcesOpen != null) return t.sourcesOpen;
    return chatOptions.sourcePanel === "expanded" || (chatOptions.sourcePanel === "auto" && last);
  }

  // With "Stream responses" off, the answer is held back until it is complete.
  let writing = $state(false);

  const busy = $derived(ui.asking);
  const lastTurn = $derived(turns[turns.length - 1]);

  async function submit() {
    const q = question.trim();
    if (!q || busy) return;
    // Turns that never got an answer (stopped at once, failed) are not context;
    // and with "Remember conversation context" off, nothing is.
    const history = chatOptions.remember
      ? turns.filter((t) => t.answer.trim() && !t.error).map((t) => ({ question: t.question, answer: t.answer }))
      : [];
    turns.push({ id: nextId++, question: q, answer: "", sources: [], sourcesOpen: null, stats: null, error: "" });
    const turn = turns[turns.length - 1];
    question = "";
    openError = "";
    stick = true;
    ui.asking = true;
    const stream = chatOptions.stream;
    let held = ""; // the answer so far, when it is not being shown as it arrives
    try {
      const res = await ask(
        q,
        history,
        answerOptions(),
        (chunk) => {
          if (stream) {
            turn.answer += chunk;
          } else {
            held += chunk;
            writing = true;
          }
        },
        (found) => (turn.sources = found),
      );
      if (!stream) turn.answer = held;
      turn.sources = res.sources;
      turn.stats = res.stats ?? null;
    } catch (e) {
      if (!stream) turn.answer = held;
      turn.error = String(e);
    } finally {
      writing = false;
      ui.asking = false;
    }
  }

  async function newChat() {
    if (busy) await cancelAsk();
    turns = [];
    openError = "";
    inputEl?.focus();
  }

  function onKey(e: KeyboardEvent) {
    if (e.key === "Enter" && !e.shiftKey) {
      e.preventDefault();
      submit();
    }
  }

  async function open(s: Source) {
    openError = "";
    try {
      await openSource(s.path, s.page);
    } catch (e) {
      openError = String(e);
    }
  }

  async function toggleSources(t: TurnView, e: MouseEvent) {
    const drawer = (e.currentTarget as HTMLElement).closest(".sources");
    t.sourcesOpen = !sourcesOpen(t, t === lastTurn);
    if (!t.sourcesOpen) return;
    // Bring the opened list into view when it lands below the fold.
    await tick();
    drawer?.scrollIntoView({ block: "nearest", behavior: "smooth" });
  }

  /** "plan.md, faq.txt": the files behind an answer, each named once. */
  function fileList(sources: Source[]): string {
    return [...new Set(sources.map((s) => s.file))].join(", ");
  }

  const num = (n: number) => Math.round(n).toLocaleString();

  /** "58 tokens · 41/s" under an answer. */
  function turnStats(s: AskStats): string {
    const speed = s.tokensPerSecond ? ` · ${Math.round(s.tokensPerSecond)}/s` : "";
    return `${num(s.answerTokens)} tokens${speed}`;
  }

  /** What went into an answer, spelled out for the tooltip. */
  function statsDetail(s: AskStats): string {
    const parts = [
      `${s.excerpts} ${s.excerpts === 1 ? "excerpt" : "excerpts"} from your files`,
      s.turnsSent ? `${s.turnsSent} earlier ${s.turnsSent === 1 ? "turn" : "turns"}` : "",
      "the question and Loft's instructions",
    ].filter(Boolean);
    const lines = [
      `Sent to the model: ${num(s.promptTokens)} tokens (${parts.join(", ")}).`,
      `Answer: ${num(s.answerTokens)} tokens${s.tokensPerSecond ? ` at ${Math.round(s.tokensPerSecond)} tokens per second` : ""}.`,
    ];
    if (s.contextSize) {
      lines.push(`Context window: ${num(s.promptTokens + s.answerTokens)} of ${num(s.contextSize)} tokens used.`);
    }
    if (s.turnsDropped) {
      lines.push(`${s.turnsDropped} older ${s.turnsDropped === 1 ? "turn was" : "turns were"} left out: only the latest few are remembered.`);
    }
    return lines.join("\n");
  }

  // Context meter: how full the model's window was for the latest answer that
  // has stats (a stopped or refused question has none).
  const meter = $derived.by(() => {
    const s = [...turns].reverse().find((t) => t.stats)?.stats;
    if (!s) return null;
    const used = s.promptTokens + s.answerTokens;
    const fraction = s.contextSize ? Math.min(used / s.contextSize, 1) : null;
    return { stats: s, used, fraction };
  });

  function onThreadScroll() {
    if (!threadEl) return;
    stick = threadEl.scrollHeight - threadEl.scrollTop - threadEl.clientHeight < 48;
  }

  // Keep the newest text in view while it streams, and when the tab or panel
  // is shown again (a hidden element loses its scroll position).
  $effect(() => {
    void turns.length;
    void lastTurn?.answer.length;
    void lastTurn?.sources.length;
    void lastTurn?.error;
    void ui.tab;
    void ui.expanded;
    if (stick && threadEl) threadEl.scrollTop = threadEl.scrollHeight;
  });

  // The question box grows with what is typed, up to a few lines.
  $effect(() => {
    void question;
    if (!inputEl) return;
    inputEl.style.height = "auto";
    inputEl.style.height = `${Math.min(inputEl.scrollHeight, 92)}px`;
  });

  // What to show while nothing has been generated yet.
  const waiting = $derived(
    !busy || !lastTurn || lastTurn.answer
      ? ""
      : writing
        ? "Writing the answer…"
        : lastTurn.sources.length
        ? `Reading ${lastTurn.sources.length} ${lastTurn.sources.length === 1 ? "excerpt" : "excerpts"}…`
        : "Searching your files…",
  );

  const examples = [
    "What are these files about?",
    "What dates or deadlines are mentioned?",
    "Who is responsible for what?",
  ];
</script>

<div class="ask">
  <div class="thread scroll" bind:this={threadEl} onscroll={onThreadScroll}>
    {#if turns.length === 0 && !hint}
      <div class="examples">
        <div class="section-label">Try asking</div>
        {#each examples as ex}
          <button class="card" disabled={!ready} onclick={() => { question = ex; submit(); }}>
            <span class="ex-icon"><Icon name="search" size={13} /></span>
            <span class="ex-text">{ex}</span>
            <span class="ex-go"><Icon name="arrow-right" size={14} /></span>
          </button>
        {/each}
      </div>
    {/if}

    {#each turns as t, i (t.id)}
      {@const last = i === turns.length - 1}
      <section class="turn" class:last>
        <p class="question">{t.question}</p>

        {#if t.error}
          <div class="error">{t.error}</div>
        {:else}
          <div class="answer">
            {#if last && waiting}<p class="waiting">{waiting}</p>{/if}
            {#if t.answer}
              <p>{t.answer}{#if last && busy}<span class="caret"></span>{/if}</p>
            {:else if !(last && busy)}
              <p class="stopped">Stopped before an answer.</p>
            {/if}
          </div>
        {/if}

        {#if t.sources.length && chatOptions.showCitations}
          {@const isOpen = sourcesOpen(t, last)}
          <!-- The files behind the answer, in a drawer. Whether it starts open
               is the "Source panel" option; "Show citations" off hides it. -->
          <div class="sources" class:open={isOpen}>
            <div class="meta-row">
            <button
              class="sources-toggle"
              aria-expanded={isOpen}
              title={isOpen ? "Hide sources" : "Show the excerpts this answer was written from"}
              onclick={(e) => toggleSources(t, e)}
            >
              <span class="chev"><Icon name="chevron-up" size={13} stroke={2.3} /></span>
              <span class="count">{t.sources.length} {t.sources.length === 1 ? "source" : "sources"}</span>
              <span class="files">{fileList(t.sources)}</span>
            </button>
            {#if t.stats}<span class="turn-stats" title={statsDetail(t.stats)}>{turnStats(t.stats)}</span>{/if}
            </div>
            {#if isOpen}
              <div class="source-list">
                {#each t.sources as s}
                  {@const tag = fileTag(s.file)}
                  <button class="source card" title={s.path} onclick={() => open(s)}>
                    <span class="tag tone-{tag.tone}">{tag.label}</span>
                    <span class="source-main">
                      <span class="source-head">
                        <span class="source-file">{s.file}{#if s.location}<span class="page">· {s.location}</span>{/if}</span>
                        <span class="score" title="How closely this excerpt matches your question">{Math.round(s.score * 100)}%</span>
                      </span>
                      <span class="snippet">{s.snippet}</span>
                    </span>
                  </button>
                {/each}
              </div>
            {/if}
          </div>
        {:else if t.stats}
          <!-- Answered (or declined) by the model without sources to show. -->
          <div class="meta-row"><span class="turn-stats" title={statsDetail(t.stats)}>{turnStats(t.stats)}</span></div>
        {/if}
      </section>
    {/each}
  </div>

  <div class="composer">
    {#if hint}
      <div class="hint-card card">
        <span>{hint.text}</span>
        {#if hint.tab}
          {@const tab = hint.tab}
          <button onclick={() => (ui.tab = tab)}>Open {tab === "settings" ? "Settings" : "Folders"}</button>
        {/if}
      </div>
    {/if}
    {#if openError}<div class="error small">{openError}</div>{/if}

    <div class="field" class:busy>
      <textarea
        bind:this={inputEl}
        bind:value={question}
        onkeydown={onKey}
        rows="1"
        placeholder={!ready ? "Getting ready…" : turns.length && chatOptions.remember ? "Ask a follow-up…" : "Ask anything about your files…"}
        disabled={!ready}
      ></textarea>
      <button
        class="options-btn"
        class:open={optionsOpen}
        class:changed={chatOptionsChanged()}
        onclick={() => (optionsOpen = !optionsOpen)}
        aria-label="Chat options"
        aria-expanded={optionsOpen}
        title="Chat &amp; answers options{chatOptionsChanged() ? ' (changed from the defaults)' : ''}"
      >
        <Icon name="sliders" size={15} />
      </button>
      {#if busy}
        <button class="send stop" onclick={cancelAsk} aria-label="Stop" title="Stop">
          <Icon name="stop" size={13} />
        </button>
      {:else}
        <button class="send" onclick={submit} disabled={!ready || !question.trim()} aria-label="Ask" title="Ask (Enter)">
          <Icon name="arrow-up" size={16} stroke={2.4} />
        </button>
      {/if}
    </div>

    {#if turns.length}
      <div class="foot">
        {#if meter}
          <span class="meter" title={statsDetail(meter.stats)}>
            {#if meter.fraction != null}
              <span class="meter-bar" class:high={meter.fraction > 0.8}><i style="width:{Math.max(meter.fraction * 100, 3)}%"></i></span>
              <span class="meter-text">Context {Math.round(meter.fraction * 100)}% · {num(meter.used)} of {num(meter.stats.contextSize ?? 0)} tokens</span>
            {:else}
              <span class="meter-text">Context {num(meter.used)} tokens</span>
            {/if}
            {#if meter.stats.turnsSent}<span class="meter-turns">· {meter.stats.turnsSent} {meter.stats.turnsSent === 1 ? "turn" : "turns"} remembered</span>{/if}
          </span>
        {:else}
          <span>{chatOptions.remember ? "Follow-ups continue this conversation" : "Each question is answered on its own"}</span>
        {/if}
        <button class="new-chat" onclick={newChat} title="Start over: the next question will not refer to this conversation">
          <Icon name="plus" size={12} stroke={2.4} />
          New chat
        </button>
      </div>
    {/if}
  </div>
  {#if optionsOpen}
    <ChatOptions onclose={() => (optionsOpen = false)} />
  {/if}
</div>

<style>
  /* The conversation scrolls; the question box stays at the bottom. */
  .ask {
    position: relative;
    display: flex;
    flex-direction: column;
    gap: 10px;
    height: 100%;
    min-height: 0;
  }
  .thread {
    flex: 1;
    min-height: 0;
    display: flex;
    flex-direction: column;
    gap: 18px;
    /* The scrollbar sits in the panel's margin, so cards line up with the question box. */
    margin-right: -8px;
    padding: 2px 0;
    scrollbar-gutter: stable;
  }
  .turn {
    display: flex;
    flex-direction: column;
    gap: 10px;
  }
  .question {
    align-self: flex-end;
    max-width: 86%;
    padding: 8px 12px;
    border-radius: 14px 14px 4px 14px;
    background: var(--field-bg-hover);
    font-size: 13.5px;
    line-height: 1.45;
    color: var(--text);
    white-space: pre-wrap;
    overflow-wrap: anywhere;
    user-select: text;
  }
  .composer {
    flex: none;
    display: flex;
    flex-direction: column;
    gap: 7px;
  }
  .foot {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 8px;
    padding: 0 3px 0 4px;
    font-size: 11px;
    color: var(--text-faint);
  }
  /* Context meter: how full the model's window was for the latest answer */
  .meter {
    display: flex;
    align-items: center;
    gap: 7px;
    min-width: 0;
    cursor: default;
    font-variant-numeric: tabular-nums;
  }
  .meter-bar {
    flex: none;
    width: 44px;
    height: 4px;
    border-radius: 3px;
    background: var(--field-bg-hover);
    overflow: hidden;
  }
  .meter-bar i {
    display: block;
    height: 100%;
    border-radius: 3px;
    background: var(--accent);
    transition: width 0.3s var(--ease);
  }
  .meter-bar.high i {
    background: var(--warn);
  }
  .meter-text,
  .meter-turns {
    white-space: nowrap;
  }
  .meter-turns {
    overflow: hidden;
    text-overflow: ellipsis;
  }
  .new-chat {
    flex: none;
    display: flex;
    align-items: center;
    gap: 4px;
    font-size: 11.5px;
    font-weight: 600;
    color: var(--text-dim);
    padding: 3px 8px;
    border-radius: 7px;
    transition:
      background 0.13s ease,
      color 0.13s ease;
  }
  .new-chat:hover {
    color: var(--text);
    background: var(--field-bg-hover);
  }

  /* Question box */
  .field {
    position: relative;
    border-radius: 16px;
    background: var(--field-bg);
    border: 1px solid var(--notch-border);
    transition:
      border-color 0.18s ease,
      box-shadow 0.18s ease,
      background 0.18s ease;
  }
  .field:focus-within {
    background: var(--field-bg-focus);
    border-color: color-mix(in srgb, var(--accent) 65%, transparent);
    box-shadow: 0 0 0 3px color-mix(in srgb, var(--accent) 18%, transparent);
  }
  textarea {
    display: block;
    width: 100%;
    resize: none;
    border: none;
    background: none;
    padding: 12px 86px 12px 15px;
    font-size: 14px;
    line-height: 1.45;
    user-select: text;
    scrollbar-width: none;
  }
  textarea:focus-visible {
    outline: none; /* the field shows the focus ring */
  }
  /* Opens the "Chat & Answers" options */
  .options-btn {
    position: absolute;
    right: 45px;
    bottom: 6px;
    display: grid;
    place-items: center;
    width: 32px;
    height: 32px;
    border-radius: 50%;
    color: var(--text-faint);
    transition:
      background 0.14s ease,
      color 0.14s ease;
  }
  .options-btn:hover,
  .options-btn.open {
    background: var(--field-bg-hover);
    color: var(--text);
  }
  .options-btn.changed {
    color: var(--accent); /* something other than the defaults is in effect */
  }

  .send {
    position: absolute;
    right: 7px;
    bottom: 6px;
    display: grid;
    place-items: center;
    width: 32px;
    height: 32px;
    border-radius: 50%;
    background: var(--accent-grad);
    color: white;
    box-shadow: 0 2px 10px color-mix(in srgb, var(--accent) 45%, transparent);
    transition:
      opacity 0.15s ease,
      transform 0.12s ease,
      box-shadow 0.15s ease;
  }
  .send:disabled {
    background: var(--field-bg-hover);
    color: var(--text-faint);
    box-shadow: none;
    cursor: default;
  }
  .send:not(:disabled):hover {
    transform: scale(1.06);
  }
  .send:not(:disabled):active {
    transform: scale(0.94);
  }
  .send.stop {
    background: var(--stop-bg);
    color: var(--stop-fg);
    box-shadow: none;
  }

  /* Guidance */
  .hint-card {
    display: flex;
    align-items: center;
    gap: 10px;
    font-size: 13px;
    line-height: 1.45;
    color: var(--text-dim);
    padding: 11px 12px 11px 14px;
  }
  .hint-card span {
    flex: 1;
  }
  .hint-card button {
    flex: none;
    font-size: 12.5px;
    font-weight: 600;
    color: var(--text);
    background: var(--field-bg-hover);
    border-radius: 9px;
    padding: 6px 11px;
    transition: background 0.13s ease;
  }
  .hint-card button:hover {
    background: var(--raised-hover);
  }

  /* Example questions */
  .examples {
    display: flex;
    flex-direction: column;
    gap: 7px;
  }
  .examples .section-label {
    margin: 2px 0 1px 2px;
  }
  .examples button {
    display: flex;
    align-items: center;
    gap: 10px;
    text-align: left;
    font-size: 13px;
    color: var(--text-dim);
    padding: 10px 12px;
    border-radius: 12px;
    transition:
      background 0.14s ease,
      color 0.14s ease,
      border-color 0.14s ease;
  }
  .examples button:hover:not(:disabled) {
    color: var(--text);
    background: var(--field-bg-hover);
    border-color: var(--border-strong);
  }
  .examples button:disabled {
    opacity: 0.5;
    cursor: default;
  }
  .ex-icon {
    color: var(--accent);
    display: grid;
  }
  .ex-text {
    flex: 1;
  }
  .ex-go {
    display: grid;
    color: var(--text-faint);
    opacity: 0;
    transform: translateX(-4px);
    transition:
      opacity 0.14s ease,
      transform 0.14s ease;
  }
  .examples button:hover:not(:disabled) .ex-go {
    opacity: 1;
    transform: none;
  }

  /* Answer */
  .answer p {
    font-size: 14px;
    line-height: 1.6;
    color: var(--text);
    white-space: pre-wrap;
    user-select: text;
    padding: 0 2px;
  }
  .answer .waiting {
    width: fit-content;
    color: transparent;
    background: linear-gradient(90deg, var(--text-faint) 35%, var(--text) 50%, var(--text-faint) 65%) 100% 0 / 250% 100%;
    -webkit-background-clip: text;
    background-clip: text;
    animation: shimmer 1.5s linear infinite;
  }
  .answer .stopped {
    font-size: 13px;
    color: var(--text-faint);
  }
  /* Earlier turns step back a little so the latest answer leads. */
  .turn:not(.last) .answer p {
    color: var(--text-dim);
  }
  @keyframes shimmer {
    to { background-position: -50% 0; }
  }
  .caret {
    display: inline-block;
    width: 7px;
    height: 15px;
    margin-left: 3px;
    background: var(--accent);
    border-radius: 2px;
    transform: translateY(2px);
    animation: blink 1s steps(2) infinite;
  }
  @keyframes blink {
    50% { opacity: 0; }
  }
  .error {
    font-size: 13px;
    line-height: 1.5;
    color: var(--bad);
    padding: 0 2px;
  }
  .error.small {
    font-size: 11.5px;
  }

  /* Sources: a drawer under the answer, closed until asked for */
  .sources {
    display: flex;
    flex-direction: column;
    gap: 8px;
    min-width: 0;
  }
  .meta-row {
    display: flex;
    align-items: center;
    gap: 10px;
    min-width: 0;
  }
  .turn-stats {
    flex: none;
    margin-left: auto;
    font-size: 11px;
    font-variant-numeric: tabular-nums;
    color: var(--text-faint);
    cursor: default;
  }
  .sources-toggle {
    min-width: 0;
    display: flex;
    align-items: center;
    gap: 6px;
    max-width: 100%;
    padding: 4px 10px 4px 6px;
    border-radius: 9px;
    border: 1px solid var(--notch-border);
    font-size: 12px;
    color: var(--text-dim);
    transition:
      background 0.14s ease,
      color 0.14s ease,
      border-color 0.14s ease;
  }
  .sources-toggle:hover,
  .sources.open .sources-toggle {
    color: var(--text);
    background: var(--field-bg-hover);
  }
  .sources-toggle .chev {
    display: grid;
    color: var(--text-faint);
    transform: rotate(90deg); /* closed: points right */
    transition: transform 0.18s var(--ease);
  }
  .sources.open .chev {
    transform: rotate(180deg); /* open: points down */
  }
  .sources-toggle .count {
    flex: none;
    font-weight: 600;
  }
  .sources-toggle .files {
    min-width: 0;
    color: var(--text-faint);
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
  }
  .source-list {
    display: flex;
    flex-direction: column;
    gap: 7px;
  }
  .source {
    display: flex;
    align-items: flex-start;
    gap: 11px;
    width: 100%;
    text-align: left;
    padding: 10px 12px;
    border-radius: 12px;
    transition:
      background 0.14s ease,
      border-color 0.14s ease;
    animation: rise 0.25s var(--ease) backwards;
  }
  .source:nth-of-type(2) { animation-delay: 0.04s; }
  .source:nth-of-type(3) { animation-delay: 0.08s; }
  .source:nth-of-type(4) { animation-delay: 0.12s; }
  @keyframes rise {
    from { opacity: 0; transform: translateY(5px); }
  }
  .source:hover {
    background: var(--field-bg-hover);
    border-color: var(--border-strong);
  }
  .tag {
    flex: none;
    min-width: 36px;
    margin-top: 1px;
    padding: 4px 0;
    border-radius: 7px;
    font-size: 9.5px;
    font-weight: 700;
    letter-spacing: 0.4px;
    text-align: center;
    color: var(--tone);
    background: color-mix(in srgb, var(--tone) 16%, transparent);
  }
  .tone-red { --tone: var(--tone-red); }
  .tone-blue { --tone: var(--tone-blue); }
  .tone-green { --tone: var(--tone-green); }
  .tone-orange { --tone: var(--tone-orange); }
  .tone-violet { --tone: var(--tone-violet); }
  .tone-grey { --tone: var(--tone-grey); }
  .source-main {
    flex: 1;
    min-width: 0;
    display: flex;
    flex-direction: column;
    gap: 3px;
  }
  .source-head {
    display: flex;
    justify-content: space-between;
    align-items: baseline;
    gap: 8px;
  }
  .source-file {
    font-size: 13px;
    font-weight: 600;
    color: var(--text);
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
  }
  .page {
    margin-left: 6px;
    color: var(--text-faint);
    font-weight: 400;
  }
  .score {
    flex: none;
    font-size: 11px;
    font-variant-numeric: tabular-nums;
    color: var(--text-faint);
  }
  .snippet {
    font-size: 12.5px;
    line-height: 1.45;
    color: var(--text-dim);
    display: -webkit-box;
    -webkit-line-clamp: 2;
    line-clamp: 2;
    -webkit-box-orient: vertical;
    overflow: hidden;
  }
</style>
