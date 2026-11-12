<script lang="ts">
  import { onMount } from "svelte";
  import { cancelBenchmark, getBenchmarks, onBenchProgress, runBenchmark } from "../api";
  import { ui } from "../state.svelte";
  import type { BenchPick, BenchProgress, BenchReport, BenchRow, Target } from "../types";
  import Icon from "./Icon.svelte";

  // "Test performance": measure every installed engine with the selected chat
  // model, and say what the measurements suggest. Loft recommends; the user
  // decides, so a recommendation is applied only when its button is pressed.
  let {
    disabled = false,
    onuse,
    ondone,
  }: {
    /** Something else is changing the engines right now. */
    disabled?: boolean;
    /** Make `target` the choice for a power state. */
    onuse: (profile: "pluggedIn" | "onBattery", target: Target) => void;
    /** A test has finished (the engines may have been restarted). */
    ondone: () => void;
  } = $props();

  let report = $state<BenchReport | null>(null);
  let progress = $state<BenchProgress | null>(null);
  let error = $state("");

  async function load() {
    try {
      report = await getBenchmarks();
    } catch (e) {
      error = String(e);
    }
  }

  onMount(() => {
    let unsub = () => {};
    onBenchProgress((p) => (progress = p.running ? p : null)).then((un) => (unsub = un));
    return () => unsub();
  });

  $effect(() => {
    if (ui.tab === "settings") load();
  });

  const running = $derived(!!progress || !!report?.running);

  async function test() {
    error = "";
    progress = { running: true, index: 0, total: 0, label: "", stage: "loading" };
    try {
      report = await runBenchmark();
    } catch (e) {
      error = String(e);
    } finally {
      progress = null;
      ondone();
    }
  }

  const seconds = (ms?: number | null) => (ms == null ? "—" : ms < 950 ? `${(ms / 1000).toFixed(2)} s` : `${(ms / 1000).toFixed(1)} s`);
  const speed = (v?: number | null) => (v == null ? "—" : v >= 10 ? v.toFixed(0) : v.toFixed(1));
  const memory = (mib?: number | null) => (mib == null ? null : mib >= 1024 ? `${(mib / 1024).toFixed(1)} GB` : `${mib} MB`);
  const when = (at: number) => new Date(at * 1000).toLocaleDateString(undefined, { day: "numeric", month: "short" });
  /** Energy is only ever a battery measurement; anything else is unmeasured. */
  const energy = (r: BenchRow) =>
    r.joulesPerToken != null && r.drainWatts != null ? `${r.joulesPerToken.toFixed(1)} J per token (${r.drainWatts.toFixed(0)} W whole computer)` : "energy unmeasured";
  const facts = (r: BenchRow) =>
    [
      `first token ${seconds(r.firstTokenMs)}`,
      `whole answer ${seconds(r.totalMs)}`,
      `loaded in ${seconds(r.loadMs)}`,
      memory(r.peakMemoryMib) ? `${memory(r.peakMemoryMib)} system memory` : null,
      memory(r.deviceMemoryMib) ? `${memory(r.deviceMemoryMib)} on the device` : null,
      energy(r),
      r.powerSource === "battery" ? "on battery" : null,
      `tested ${when(r.at)}`,
    ]
      .filter(Boolean)
      .join(" · ");
</script>

{#snippet pick(title: string, profile: "pluggedIn" | "onBattery", p: BenchPick)}
  <div class="pick">
    <div class="pick-text">
      <span class="pick-title">{title}: <strong>{p.label}</strong></span>
      <span class="pick-reason">{p.reason}</span>
    </div>
    <button class="small-btn" disabled={disabled || running} onclick={() => onuse(profile, $state.snapshot(p.target))}>Use it</button>
  </div>
{/snippet}

<div class="bench card">
  <div class="bench-title">Test performance</div>
  <p class="bench-text">
    Runs each installed engine in turn with your chat model and the same question, and checks where the model really ran. It takes about
    half a minute per engine, and the chat model is unavailable until it has finished. Other busy programs slow the engines down unevenly, so
    close them first for a fair comparison.
  </p>

  {#if running}
    <div class="running">
      <span class="spinner" aria-hidden="true"></span>
      <span class="running-text">
        {#if progress?.total}
          Testing {Math.min(progress.index + 1, progress.total)} of {progress.total}: {progress.label}
          ({progress.stage === "measuring" ? "measuring" : "loading the model"})
        {:else}
          Starting the test…
        {/if}
      </span>
      <button class="small-btn" onclick={() => cancelBenchmark()} title="Stop after the engine being tested">Stop</button>
    </div>
  {:else}
    <button class="small-btn primary" {disabled} onclick={test}>{report?.rows.length ? "Test again" : "Test performance"}</button>
  {/if}

  {#if error}<div class="note">{error}</div>{/if}

  {#if report?.rows.length}
    <div class="rows">
      {#each report.rows as r (r.label + r.powerSource)}
        <div class="row" class:stale={!!r.stale} class:failed={!r.ok}>
          <div class="row-line">
            <span class="row-label">{r.label}</span>
            {#if r.ok}
              <span class="row-speed">{speed(r.tokensPerSecond)} <span class="unit">tokens/s</span></span>
            {:else}
              <span class="row-fail">Failed</span>
            {/if}
          </div>
          {#if r.ok}
            <div class="row-sub">{facts(r)}</div>
            {#if r.verified && r.actual}
              <div class="row-sub"><Icon name="check" size={11} stroke={2.6} /> {r.actual}</div>
            {/if}
          {:else}
            <div class="row-sub bad-text">{r.error}</div>
          {/if}
          {#if r.stale}<div class="row-sub warn">Out of date: {r.stale}. Not used for recommendations.</div>{/if}
        </div>
      {/each}
    </div>

    {#if report.unmeasured.length}
      <p class="bench-text warn">Not measured yet: {report.unmeasured.join(", ")}. Test again to include {report.unmeasured.length === 1 ? "it" : "them"}.</p>
    {/if}
    {#if report.advice.pluggedIn}{@render pick("Recommended when plugged in", "pluggedIn", report.advice.pluggedIn)}{/if}
    {#if report.advice.onBattery}{@render pick("Recommended on battery", "onBattery", report.advice.onBattery)}{/if}
    {#if report.advice.pluggedIn || report.advice.onBattery}
      <p class="bench-text">Automatic follows these measurements. Your own choice above always takes priority.</p>
    {/if}
    <p class="bench-text faint">{report.advice.batteryNote}</p>
  {/if}
</div>

<style>
  .bench {
    display: flex;
    flex-direction: column;
    gap: 8px;
    padding: 10px 12px 11px 14px;
  }
  .bench-title {
    font-size: 13px;
    font-weight: 600;
  }
  .bench-text {
    font-size: 11.5px;
    line-height: 1.45;
    color: var(--text-dim);
  }
  .bench-text.faint {
    color: var(--text-faint);
  }
  .bench-text.warn {
    color: var(--warn);
  }
  .note {
    font-size: 11.5px;
    line-height: 1.4;
    color: var(--warn);
  }
  .running {
    display: flex;
    align-items: center;
    gap: 8px;
  }
  .running-text {
    flex: 1;
    min-width: 0;
    font-size: 12px;
    color: var(--text);
  }
  .spinner {
    flex: none;
    width: 11px;
    height: 11px;
    border-radius: 50%;
    border: 2px solid var(--field-bg-hover);
    border-top-color: var(--accent);
    animation: spin 0.8s linear infinite;
  }
  @keyframes spin {
    to { transform: rotate(360deg); }
  }
  .rows {
    display: flex;
    flex-direction: column;
  }
  .row {
    display: flex;
    flex-direction: column;
    gap: 2px;
    padding: 7px 0;
    border-top: 1px solid var(--notch-border);
  }
  .row.stale {
    opacity: 0.62;
  }
  .row-line {
    display: flex;
    align-items: baseline;
    gap: 8px;
    font-size: 12.5px;
  }
  .row-label {
    flex: 1;
    min-width: 0;
    font-weight: 600;
    color: var(--text);
    overflow-wrap: anywhere;
  }
  .row-speed {
    flex: none;
    font-weight: 600;
    font-variant-numeric: tabular-nums;
    color: var(--text);
  }
  .unit {
    font-size: 11px;
    font-weight: 500;
    color: var(--text-faint);
  }
  .row-fail {
    flex: none;
    font-size: 11px;
    font-weight: 600;
    color: var(--bad);
  }
  .row-sub {
    display: flex;
    gap: 5px;
    align-items: baseline;
    font-size: 11px;
    line-height: 1.4;
    color: var(--text-faint);
  }
  .row-sub.warn {
    color: var(--warn);
  }
  .bad-text {
    color: var(--bad);
  }
  .pick {
    display: flex;
    align-items: center;
    gap: 10px;
    padding-top: 8px;
    border-top: 1px solid var(--notch-border);
  }
  .pick-text {
    flex: 1;
    min-width: 0;
    display: flex;
    flex-direction: column;
    gap: 1px;
  }
  .pick-title {
    font-size: 12px;
    color: var(--text-dim);
  }
  .pick-title strong {
    font-weight: 600;
    color: var(--text);
  }
  .pick-reason {
    font-size: 11px;
    line-height: 1.4;
    color: var(--text-faint);
  }
  .small-btn {
    align-self: flex-start;
    flex: none;
    display: inline-flex;
    align-items: center;
    gap: 5px;
    font-size: 12px;
    font-weight: 600;
    padding: 6px 11px;
    border-radius: 9px;
    color: var(--text);
    background: var(--field-bg-hover);
    transition: background 0.14s ease;
  }
  .small-btn:hover:not(:disabled) {
    background: var(--raised-hover);
  }
  .small-btn:disabled {
    opacity: 0.5;
    cursor: default;
  }
  .small-btn.primary {
    background: var(--accent);
    color: #fff;
  }
  .small-btn.primary:hover:not(:disabled) {
    background: var(--accent);
    opacity: 0.88;
  }
  .pick .small-btn {
    align-self: center;
  }
</style>
