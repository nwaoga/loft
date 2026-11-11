<script lang="ts">
  import { getAutostart, setAutostart, quitApp, listMonitors, setMonitor } from "../api";
  import type { MonitorInfo } from "../types";
  import { ui } from "../state.svelte";
  import { THEMES, theme, setTheme } from "../theme.svelte";
  import { PLACEMENTS, placement, choosePlacement, type Placement } from "../placement.svelte";
  import Models from "./Models.svelte";
  import Icon from "./Icon.svelte";

  let autostart = $state(false);
  let autostartError = $state("");

  // Read the real launch-at-login state each time Settings is opened.
  $effect(() => {
    if (ui.tab !== "settings") return;
    autostartError = "";
    getAutostart()
      .then((on) => (autostart = on))
      .catch(() => (autostart = false));
  });

  // Displays: read each time Settings is opened (they can be plugged in or out).
  let monitors = $state<MonitorInfo[]>([]);
  $effect(() => {
    if (ui.tab !== "settings") return;
    listMonitors()
      .then((list) => (monitors = list))
      .catch(() => (monitors = []));
  });

  async function chooseMonitor(m: MonitorInfo) {
    placementError = "";
    if (m.selected) return;
    try {
      await setMonitor(m.id, ui.expanded);
      monitors = await listMonitors();
    } catch (e) {
      placementError = String(e);
    }
  }

  let placementError = $state("");
  async function place(value: Placement) {
    placementError = "";
    try {
      await choosePlacement(value);
    } catch (e) {
      placementError = String(e);
    }
  }

  async function toggleAutostart() {
    autostartError = "";
    try {
      autostart = await setAutostart(!autostart);
    } catch (e) {
      autostartError = String(e);
    }
  }
</script>

<div class="settings">
  <section>
    <div class="section-label">Theme</div>
    <div class="themes" role="radiogroup" aria-label="Theme">
      {#each THEMES as t (t.id)}
        <button
          class="theme"
          class:selected={theme.id === t.id}
          role="radio"
          aria-checked={theme.id === t.id}
          onclick={() => setTheme(t.id)}
        >
          <!-- A miniature notch drawn with that theme's own colours. -->
          <span class="preview" data-theme={t.id} aria-hidden="true">
            <span class="p-dot"></span>
            <span class="p-lines"><i></i><i></i></span>
          </span>
          <span class="theme-name">{t.name}</span>
        </button>
      {/each}
    </div>
  </section>

  <section>
    <div class="section-label">Placement and display</div>
    <div class="placements" role="radiogroup" aria-label="Placement">
      {#each PLACEMENTS as p (p.id)}
        <button
          class="placement"
          class:selected={placement.value === p.id}
          role="radio"
          aria-checked={placement.value === p.id}
          onclick={() => place(p.id)}
        >
          <!-- A little screen with the notch marked on its edge. -->
          <span class="screen {p.id}" aria-hidden="true"><i></i></span>
          <span class="place-text">
            <span class="place-name">{p.name}</span>
            <span class="place-sub">{p.hint}</span>
          </span>
        </button>
      {/each}
    </div>
    {#if monitors.length > 0}
      <div class="displays" role="radiogroup" aria-label="Display">
        {#each monitors as m (m.id)}
          <button
            class="display"
            class:selected={m.selected}
            role="radio"
            aria-checked={m.selected}
            title={m.selected ? "Loft is on this display" : "Move Loft to this display"}
            onclick={() => chooseMonitor(m)}
          >
            <span class="display-name">{m.label}{#if m.primary}<span class="main">main</span>{/if}</span>
            <span class="display-size">{m.width} × {m.height}</span>
          </button>
        {/each}
      </div>
    {/if}
    {#if placementError}<div class="note">{placementError}</div>{/if}
  </section>

  <section>
    <Models />
  </section>

  <section>
    <div class="section-label">General</div>
    <div class="row card">
      <div class="row-text">
        <div class="row-title">Launch at login</div>
        <div class="row-sub">Start Loft when you sign in to this computer.</div>
      </div>
      <button
        class="switch"
        class:on={autostart}
        role="switch"
        aria-checked={autostart}
        aria-label="Launch at login"
        onclick={toggleAutostart}
      ><span></span></button>
    </div>
    {#if autostartError}<div class="note">{autostartError}</div>{/if}

    <button class="quit card" onclick={quitApp}>
      <Icon name="power" size={14} />
      Quit Loft
    </button>
  </section>
</div>

<style>
  .settings {
    display: flex;
    flex-direction: column;
    gap: 18px;
    padding-bottom: 4px;
  }
  section {
    display: flex;
    flex-direction: column;
    gap: 8px;
  }
  .section-label {
    margin: 4px 0 0 2px;
  }

  /* Theme picker */
  .themes {
    display: grid;
    grid-template-columns: repeat(4, 1fr);
    gap: 8px;
  }
  .theme {
    display: flex;
    flex-direction: column;
    gap: 6px;
    padding: 5px 5px 7px;
    border-radius: 13px;
    border: 1px solid var(--notch-border);
    background: var(--field-bg);
    transition:
      background 0.15s ease,
      border-color 0.15s ease;
  }
  .theme:hover {
    background: var(--field-bg-hover);
  }
  .theme.selected {
    border-color: color-mix(in srgb, var(--accent) 60%, transparent);
    background: color-mix(in srgb, var(--accent) 12%, transparent);
  }
  .preview {
    display: flex;
    align-items: center;
    gap: 7px;
    height: 40px;
    padding: 0 9px;
    border-radius: 9px;
    background: var(--notch);
    border: 1px solid var(--notch-border);
  }
  .p-dot {
    flex: none;
    width: 14px;
    height: 14px;
    border-radius: 50%;
    background: var(--accent-grad);
  }
  .p-lines {
    flex: 1;
    display: flex;
    flex-direction: column;
    gap: 5px;
  }
  .p-lines i {
    display: block;
    height: 3px;
    border-radius: 2px;
    background: var(--text-dim);
  }
  .p-lines i:last-child {
    width: 60%;
    background: var(--text-faint);
  }
  .theme-name {
    font-size: 11.5px;
    font-weight: 600;
    color: var(--text-dim);
    text-align: center;
  }
  .theme.selected .theme-name {
    color: var(--text);
  }

  /* Placement picker */
  .placements {
    display: grid;
    grid-template-columns: repeat(2, 1fr);
    gap: 8px;
  }
  .placement {
    display: flex;
    align-items: center;
    gap: 11px;
    padding: 9px 11px;
    border-radius: 13px;
    border: 1px solid var(--notch-border);
    background: var(--field-bg);
    text-align: left;
    transition:
      background 0.15s ease,
      border-color 0.15s ease;
  }
  .placement:hover {
    background: var(--field-bg-hover);
  }
  .placement.selected {
    border-color: color-mix(in srgb, var(--accent) 60%, transparent);
    background: color-mix(in srgb, var(--accent) 12%, transparent);
  }
  .screen {
    position: relative;
    flex: none;
    width: 44px;
    height: 30px;
    border-radius: 5px;
    border: 1.5px solid var(--text-faint);
  }
  .screen i {
    position: absolute;
    background: var(--text-dim);
  }
  .placement.selected .screen i {
    background: var(--accent);
  }
  .screen.top i {
    top: 0;
    left: 50%;
    width: 16px;
    height: 5px;
    transform: translateX(-50%);
    border-radius: 0 0 3px 3px;
  }
  .screen.right i {
    right: 0;
    top: 50%;
    width: 5px;
    height: 14px;
    transform: translateY(-50%);
    border-radius: 3px 0 0 3px;
  }
  .place-text {
    display: flex;
    flex-direction: column;
    min-width: 0;
  }
  .place-name {
    font-size: 13px;
    font-weight: 600;
  }
  .place-sub {
    font-size: 11px;
    color: var(--text-faint);
    margin-top: 1px;
  }

  /* Display picker (under Placement) */
  .displays {
    display: grid;
    grid-template-columns: repeat(2, 1fr);
    gap: 8px;
  }
  .display {
    display: flex;
    flex-direction: column;
    gap: 1px;
    padding: 8px 11px;
    border-radius: 13px;
    border: 1px solid var(--notch-border);
    background: var(--field-bg);
    text-align: left;
    transition:
      background 0.15s ease,
      border-color 0.15s ease;
  }
  .display:hover {
    background: var(--field-bg-hover);
  }
  .display.selected {
    border-color: color-mix(in srgb, var(--accent) 60%, transparent);
    background: color-mix(in srgb, var(--accent) 12%, transparent);
  }
  .display-name {
    display: flex;
    align-items: center;
    gap: 7px;
    font-size: 13px;
    font-weight: 600;
  }
  .main {
    font-size: 9.5px;
    font-weight: 600;
    text-transform: uppercase;
    letter-spacing: 0.6px;
    color: var(--text-faint);
    border: 1px solid var(--notch-border);
    border-radius: 5px;
    padding: 1px 5px;
  }
  .display-size {
    font-size: 11px;
    font-variant-numeric: tabular-nums;
    color: var(--text-faint);
  }

  /* General */
  .row {
    display: flex;
    align-items: center;
    gap: 12px;
    padding: 10px 12px 10px 14px;
  }
  .row-text {
    flex: 1;
    min-width: 0;
  }
  .row-title {
    font-size: 13.5px;
    font-weight: 600;
  }
  .row-sub {
    font-size: 11.5px;
    color: var(--text-faint);
    margin-top: 2px;
  }
  .switch {
    flex: none;
    width: 38px;
    height: 22px;
    padding: 2px;
    border-radius: 11px;
    background: var(--raised);
    box-shadow: inset 0 0 0 1px var(--notch-border);
    transition: background 0.18s ease;
  }
  .switch span {
    display: block;
    width: 18px;
    height: 18px;
    border-radius: 50%;
    background: #fff;
    box-shadow: 0 1px 3px rgba(0, 0, 0, 0.35);
    transition: transform 0.18s var(--ease);
  }
  .switch.on {
    background: var(--accent);
    box-shadow: none;
  }
  .switch.on span {
    transform: translateX(16px);
  }
  .note {
    font-size: 11.5px;
    line-height: 1.4;
    color: var(--warn);
    padding: 0 4px;
  }
  .quit {
    display: flex;
    align-items: center;
    justify-content: center;
    gap: 7px;
    padding: 9px 12px;
    font-size: 13px;
    font-weight: 600;
    color: var(--text-dim);
    transition:
      background 0.14s ease,
      color 0.14s ease,
      border-color 0.14s ease;
  }
  .quit:hover {
    color: var(--bad);
    background: color-mix(in srgb, var(--bad) 10%, transparent);
    border-color: color-mix(in srgb, var(--bad) 40%, transparent);
  }
</style>
