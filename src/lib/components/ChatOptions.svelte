<script lang="ts">
  import {
    chatOptions,
    saveChatOptions,
    resetChatOptions,
    describeChatOptions,
    LENGTHS,
    TONES,
    SOURCE_PANELS,
    HISTORIES,
    MAX_TOKENS,
    type ChatOptions,
  } from "../chat-options.svelte";
  import Icon from "./Icon.svelte";

  let { onclose }: { onclose: () => void } = $props();

  /** Change one option and remember it. */
  function set<K extends keyof ChatOptions>(key: K, value: ChatOptions[K]) {
    chatOptions[key] = value;
    saveChatOptions();
  }

  let copied = $state<"" | "Copied" | "Could not copy">("");
  let copiedTimer: ReturnType<typeof setTimeout> | undefined;
  async function copy() {
    const text = describeChatOptions();
    try {
      await navigator.clipboard.writeText(text);
      copied = "Copied";
    } catch {
      // Older or stricter webviews: copy through a temporary text box.
      const box = document.createElement("textarea");
      box.value = text;
      box.style.position = "fixed";
      box.style.opacity = "0";
      document.body.appendChild(box);
      box.select();
      copied = document.execCommand("copy") ? "Copied" : "Could not copy";
      box.remove();
    }
    clearTimeout(copiedTimer);
    copiedTimer = setTimeout(() => (copied = ""), 1600);
  }

  type Choice = { id: string; name: string; hint?: string };
</script>

{#snippet radios(label: string, key: "tone" | "sourcePanel" | "history", choices: Choice[])}
  <div class="field-label" id="opt-{key}">{label}</div>
  <div class="radios" role="radiogroup" aria-labelledby="opt-{key}">
    {#each choices as choice (choice.id)}
      {@const on = chatOptions[key] === choice.id}
      <button class="radio" class:on role="radio" aria-checked={on} onclick={() => set(key, choice.id as never)}>
        <span class="ring"></span>
        <span>{choice.name}{#if choice.hint}{" "}<span class="dim">— {choice.hint}</span>{/if}</span>
      </button>
    {/each}
  </div>
{/snippet}

{#snippet toggle(key: "showCitations" | "filesOnly" | "stream" | "remember", title: string, hint: string)}
  <button class="toggle" class:on={chatOptions[key]} role="switch" aria-checked={chatOptions[key]} onclick={() => set(key, !chatOptions[key])}>
    <span class="toggle-text">
      <span class="toggle-title">{title}</span>
      <span class="toggle-hint">{hint}</span>
    </span>
    <span class="tick">{#if chatOptions[key]}<Icon name="check" size={11} stroke={3.4} />{/if}</span>
  </button>
{/snippet}

<div class="chat-options" role="dialog" aria-label="Chat and answers options">
  <header>
    <span class="head-icon"><Icon name="sliders" size={14} /></span>
    <h2>Loft — Chat &amp; Answers</h2>
    <span class="badge" title="These options are kept on this computer and apply only to this copy of Loft">Local</span>
    <button class="icon-btn" onclick={onclose} aria-label="Close chat options" title="Close (Esc)">
      <Icon name="x" size={15} />
    </button>
  </header>

  <div class="options-body scroll">
    <section>
      <div class="section-label">Response style</div>
      <div class="field-label" id="opt-length">Answer length</div>
      <div class="segments" role="radiogroup" aria-labelledby="opt-length">
        {#each LENGTHS as l (l.id)}
          <button class:on={chatOptions.length === l.id} role="radio" aria-checked={chatOptions.length === l.id} onclick={() => set("length", l.id)}>
            {l.name}
          </button>
        {/each}
      </div>
      <p class="help">Controls how much explanation Loft provides.</p>
      {@render radios("Response tone", "tone", TONES)}
    </section>

    <section>
      <div class="section-label">Sources &amp; accuracy</div>
      {@render toggle("showCitations", "Show citations", "Reference the files used in answers.")}
      {@render radios("Source panel", "sourcePanel", SOURCE_PANELS)}
      {@render toggle("filesOnly", "Answer from my files only", "Say when the indexed files don't contain an answer.")}
    </section>

    <section>
      <div class="section-label">Chat behaviour</div>
      {@render toggle("stream", "Stream responses", "Display generated text as it arrives.")}
      {@render toggle("remember", "Remember conversation context", "Use earlier messages for follow-up questions.")}
      {@render radios("Chat history", "history", HISTORIES)}
    </section>

    <section>
      <div class="section-label with-badge">Advanced model settings <span class="badge">Optional</span></div>
      <div class="field-label split">
        <label for="opt-temperature">Temperature</label>
        <span class="value">{chatOptions.temperature.toFixed(Math.round(chatOptions.temperature * 100) % 10 ? 2 : 1)}</span>
      </div>
      <input
        id="opt-temperature"
        type="range"
        min="0"
        max="1"
        step="0.05"
        value={chatOptions.temperature}
        oninput={(e) => set("temperature", Number(e.currentTarget.value))}
      />
      <div class="ends"><span>More predictable</span><span>More varied</span></div>

      <label class="field-label" for="opt-max-tokens">Maximum output tokens</label>
      <div class="select">
        <select id="opt-max-tokens" value={chatOptions.maxTokens} onchange={(e) => set("maxTokens", Number(e.currentTarget.value))}>
          {#each MAX_TOKENS as m (m.value)}
            <option value={m.value}>{m.value.toLocaleString()} — {m.name}</option>
          {/each}
        </select>
        <span class="select-arrow"><Icon name="chevron-up" size={14} /></span>
      </div>
    </section>

    <div class="actions">
      {#if copied}<span class="copied">{copied}</span>{/if}
      <button class="reset" onclick={resetChatOptions} title="Put every option back to its default">Reset</button>
      <button class="copy" onclick={copy} title="Copy these options as text">Copy settings</button>
    </div>
  </div>
</div>

<style>
  /* A sheet that slides up over the conversation. */
  .chat-options {
    position: absolute;
    inset: 0;
    z-index: 6;
    display: flex;
    flex-direction: column;
    background: var(--notch);
    animation: sheet-in 0.22s var(--ease);
  }
  @keyframes sheet-in {
    from { opacity: 0; transform: translateY(14px); }
  }
  header {
    flex: none;
    display: flex;
    align-items: center;
    gap: 8px;
    padding: 2px 0 10px 2px;
    border-bottom: 1px solid var(--notch-border);
  }
  .head-icon {
    display: grid;
    color: var(--accent);
  }
  h2 {
    flex: 1;
    font-size: 13.5px;
    font-weight: 650;
    letter-spacing: 0.1px;
  }
  .badge {
    flex: none;
    font-size: 10px;
    font-weight: 600;
    color: var(--text-dim);
    background: var(--field-bg-hover);
    border-radius: 7px;
    padding: 2px 7px;
    text-transform: none;
    letter-spacing: 0;
  }
  .options-body {
    flex: 1;
    min-height: 0;
    margin-right: -8px;
    padding-right: 0;
    scrollbar-gutter: stable;
  }
  section {
    display: flex;
    flex-direction: column;
    gap: 7px;
    padding: 13px 2px 14px;
    border-bottom: 1px solid var(--notch-border);
  }
  .section-label {
    margin-bottom: 2px;
  }
  .section-label.with-badge {
    display: flex;
    align-items: center;
    justify-content: space-between;
  }
  .field-label {
    font-size: 12px;
    font-weight: 500;
    color: var(--text-dim);
    margin-top: 3px;
  }
  .field-label.split {
    display: flex;
    justify-content: space-between;
    align-items: baseline;
  }
  .value {
    font-size: 12.5px;
    font-weight: 650;
    color: var(--text);
    font-variant-numeric: tabular-nums;
  }
  .help {
    font-size: 11.5px;
    color: var(--text-faint);
  }

  /* Answer length: three equal segments */
  .segments {
    display: grid;
    grid-template-columns: repeat(3, 1fr);
    gap: 2px;
    padding: 3px;
    border-radius: 11px;
    background: var(--field-bg);
  }
  .segments button {
    padding: 6px 0;
    border-radius: 8px;
    font-size: 12.5px;
    font-weight: 550;
    color: var(--text-dim);
    transition:
      background 0.15s ease,
      color 0.15s ease;
  }
  .segments button:hover {
    color: var(--text);
  }
  .segments button.on {
    color: var(--text);
    background: var(--raised);
    box-shadow: 0 1px 2px rgba(0, 0, 0, 0.3);
  }

  /* One-of-several choices */
  .radios {
    display: flex;
    flex-direction: column;
  }
  .radio {
    display: flex;
    align-items: center;
    gap: 10px;
    padding: 5px 0;
    text-align: left;
    font-size: 13px;
    color: var(--text-dim);
    transition: color 0.13s ease;
  }
  .radio:hover,
  .radio.on {
    color: var(--text);
  }
  .radio .dim {
    color: var(--text-dim);
  }
  .ring {
    flex: none;
    position: relative;
    width: 16px;
    height: 16px;
    border-radius: 50%;
    border: 1.5px solid var(--text-faint);
    transition:
      background 0.13s ease,
      border-color 0.13s ease;
  }
  .radio.on .ring {
    border-color: var(--text);
    background: var(--text);
  }
  .radio.on .ring::after {
    content: "";
    position: absolute;
    inset: 3.5px;
    border-radius: 50%;
    background: var(--notch);
  }

  /* On/off options: a title, a line of explanation, and a tick */
  .toggle {
    display: flex;
    align-items: center;
    gap: 12px;
    padding: 4px 0;
    text-align: left;
  }
  .toggle-text {
    flex: 1;
    display: flex;
    flex-direction: column;
    gap: 1px;
    min-width: 0;
  }
  .toggle-title {
    font-size: 13px;
    font-weight: 600;
    color: var(--text);
  }
  .toggle-hint {
    font-size: 11.5px;
    color: var(--text-faint);
  }
  .tick {
    flex: none;
    display: grid;
    place-items: center;
    width: 18px;
    height: 18px;
    border-radius: 50%;
    border: 1.5px solid var(--text-faint);
    color: var(--notch);
    transition:
      background 0.13s ease,
      border-color 0.13s ease;
  }
  .toggle.on .tick {
    border-color: var(--text);
    background: var(--text);
  }

  /* Temperature */
  input[type="range"] {
    -webkit-appearance: none;
    appearance: none;
    display: block;
    width: 100%;
    height: 18px;
    margin: 2px 0 0;
    padding: 0;
    border: none;
    border-radius: 9px;
    background: var(--field-bg-hover);
    cursor: pointer;
  }
  input[type="range"]::-webkit-slider-thumb {
    -webkit-appearance: none;
    appearance: none;
    width: 16px;
    height: 16px;
    border-radius: 50%;
    border: none;
    background: var(--text);
    box-shadow: 0 1px 3px rgba(0, 0, 0, 0.4);
  }
  input[type="range"]::-moz-range-thumb {
    width: 16px;
    height: 16px;
    border-radius: 50%;
    border: none;
    background: var(--text);
  }
  .ends {
    display: flex;
    justify-content: space-between;
    font-size: 11px;
    color: var(--text-faint);
  }

  /* Maximum output tokens */
  .select {
    position: relative;
  }
  select {
    -webkit-appearance: none;
    appearance: none;
    width: 100%;
    padding: 8px 34px 8px 13px;
    border-radius: 12px;
    border: 1px solid var(--border-strong);
    background: var(--field-bg);
    color: var(--text);
    font-family: inherit;
    font-size: 12.5px;
    cursor: pointer;
  }
  select:focus-visible {
    outline: 2px solid color-mix(in srgb, var(--accent) 70%, transparent);
    outline-offset: 2px;
  }
  option {
    background: var(--notch);
    color: var(--text);
  }
  .select-arrow {
    position: absolute;
    right: 11px;
    top: 50%;
    display: grid;
    color: var(--text-dim);
    transform: translateY(-50%) rotate(180deg);
    pointer-events: none;
  }

  .actions {
    display: flex;
    align-items: center;
    justify-content: flex-end;
    gap: 8px;
    padding: 12px 2px 4px;
  }
  .copied {
    margin-right: auto;
    font-size: 11.5px;
    color: var(--text-dim);
  }
  .reset,
  .copy {
    font-size: 12.5px;
    font-weight: 600;
    padding: 7px 14px;
    border-radius: 16px;
    transition:
      background 0.14s ease,
      opacity 0.14s ease;
  }
  .reset {
    color: var(--text);
    border: 1px solid var(--border-strong);
  }
  .reset:hover {
    background: var(--field-bg-hover);
  }
  .copy {
    color: var(--notch);
    background: var(--text);
  }
  .copy:hover {
    opacity: 0.88;
  }
</style>
