<script lang="ts">
  import { getUsage } from "../api";
  import { ui } from "../state.svelte";
  import type { Usage, UsagePart } from "../types";

  // Usage: what Loft is using right now, as meters (memory, graphics card,
  // processor) and then part by part. Read every couple of seconds while
  // this view is open.
  const EVERY_MS = 2000;

  let usage = $state<Usage | null>(null);
  let failed = $state("");

  $effect(() => {
    if (ui.tab !== "usage" || !ui.expanded) return;
    let stopped = false;
    let timer: ReturnType<typeof setTimeout> | undefined;
    async function read() {
      if (stopped) return;
      if (document.visibilityState === "visible") {
        try {
          usage = await getUsage();
          failed = "";
        } catch (e) {
          failed = String(e);
        }
      }
      if (!stopped) timer = setTimeout(read, EVERY_MS);
    }
    read();
    return () => {
      stopped = true;
      clearTimeout(timer);
    };
  });

  const size = (bytes?: number | null) => {
    if (bytes == null) return "—";
    const gb = bytes / 1024 ** 3;
    if (gb >= 1) return `${gb >= 10 ? gb.toFixed(0) : gb.toFixed(1)} GB`;
    return `${Math.max(1, Math.round(bytes / 1024 ** 2))} MB`;
  };
  const percent = (value?: number | null) =>
    value == null ? "…" : value > 0 && value < 1 ? "<1%" : `${Math.round(value)}%`;
  const share = (part?: number | null, whole?: number | null) =>
    part != null && whole ? Math.min(100, (part / whole) * 100) : null;
  const tokens = (n: number) => n.toLocaleString();

  /** The graphics card the models use, when one with memory of its own is. */
  const card = $derived.by(() => {
    const devices = (usage?.parts ?? []).map((p) => p.device).filter((d) => d && !d.shared);
    if (!devices.length) return null;
    return {
      name: devices[0]!.name,
      bytes: devices.reduce((sum, d) => sum + (d!.bytes ?? 0), 0),
      totalBytes: devices[0]!.totalBytes ?? null,
    };
  });
  const memoryShare = $derived(share(usage?.memoryBytes, usage?.systemTotalBytes));
  const othersShare = $derived(
    usage?.systemTotalBytes && usage.systemAvailableBytes != null
      ? Math.max(0, share(usage.systemTotalBytes - usage.systemAvailableBytes - usage.memoryBytes, usage.systemTotalBytes) ?? 0)
      : null,
  );

  /** What a part is, in a line: its model and where it runs. */
  const what = (p: UsagePart) => [p.model, p.runsOn].filter(Boolean).join(" · ");
</script>

{#snippet meter(title: string, caption: string, used: number | null, figure: string, others: number | null = null)}
  <div class="meter">
    <div class="meter-text">
      <div class="meter-title">{title}</div>
      <div class="meter-caption">{caption}</div>
    </div>
    <div class="meter-figure">{figure}</div>
    <div class="bar" role="meter" aria-label={title} aria-valuemin={0} aria-valuemax={100} aria-valuenow={Math.round(used ?? 0)}>
      <span class="fill" style="width: {used ?? 0}%"></span>
      {#if others != null}<span class="others" style="width: {Math.min(others, 100 - (used ?? 0))}%"></span>{/if}
    </div>
  </div>
{/snippet}

<div class="usage-view">
  <section>
    <div class="section-label">What Loft is using</div>
    {#if !usage}
      <div class="faint">{failed || "Measuring…"}</div>
    {:else}
      <div class="meters card">
        {@render meter(
          "Memory",
          usage.systemTotalBytes
            ? `${size(usage.memoryBytes)} of ${size(usage.systemTotalBytes)}${usage.systemAvailableBytes != null ? ` · ${size(usage.systemAvailableBytes)} free on this computer` : ""}`
            : size(usage.memoryBytes),
          memoryShare,
          memoryShare == null ? size(usage.memoryBytes) : `${percent(memoryShare)} used`,
          othersShare,
        )}
        {#if card}
          {@render meter(
            "Graphics card",
            `${size(card.bytes)}${card.totalBytes ? ` of ${size(card.totalBytes)}` : ""} · ${card.name}`,
            share(card.bytes, card.totalBytes),
            card.totalBytes ? `${percent(share(card.bytes, card.totalBytes))} used` : size(card.bytes),
          )}
        {/if}
        {@render meter("Processor", `Share of all ${usage.cores} cores`, usage.cpuPercent ?? 0, `${percent(usage.cpuPercent)} used`)}
      </div>
      {#if othersShare != null}
        <div class="faint legend"><i class="dot mine"></i>Loft <i class="dot others"></i>other programs</div>
      {/if}
    {/if}
  </section>

  {#if usage}
    <section>
      <div class="section-label">By part</div>
      <div class="parts card">
        {#each usage.parts as p (p.kind)}
          <div class="part">
            <div class="part-line">
              <span class="part-name">{p.label}</span>
              <span class="part-figures">
                {size(p.memoryBytes)}{#if p.device && !p.device.shared}&nbsp;+ {size(p.device.bytes)} on the card{/if}&nbsp;· {percent(p.cpuPercent)}
              </span>
            </div>
            <div class="bar thin" aria-hidden="true">
              <span class="fill" style="width: {share(p.memoryBytes, usage.memoryBytes) ?? 0}%"></span>
            </div>
            {#if what(p)}<div class="part-sub">{what(p)}</div>{/if}
            {#if p.weightsBytes}
              <div class="part-sub">
                {`Model ${size(p.weightsBytes)}`}{p.contextBytes
                  ? `, context ${size(p.contextBytes)}${p.contextTokens ? ` (room for ${tokens(p.contextTokens)} tokens)` : ""}`
                  : ""}{p.device?.shared ? ` · ${size(p.device.bytes)} of its memory is used by ${p.device.name}` : ""}
              </div>
            {/if}
          </div>
        {/each}
      </div>
      <p class="faint">
        Memory is what is in RAM right now. Memory on a graphics card with its own memory is counted separately. Processor is the share of
        all cores, so 100% is every core flat out. Updated every 2 seconds while this is open.
      </p>
    </section>
  {/if}
</div>

<style>
  .usage-view {
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
  .faint {
    margin: 0 2px;
    font-size: 11px;
    line-height: 1.45;
    color: var(--text-faint);
  }
  .meters {
    display: flex;
    flex-direction: column;
    gap: 16px;
    padding: 14px 14px 15px;
  }
  .meter {
    display: grid;
    grid-template-columns: 1fr auto;
    gap: 3px 12px;
    align-items: end;
  }
  .meter-title {
    font-size: 13px;
    font-weight: 600;
    color: var(--text);
  }
  .meter-caption {
    margin-top: 2px;
    font-size: 11px;
    color: var(--text-faint);
    font-variant-numeric: tabular-nums;
  }
  .meter-figure {
    font-size: 12px;
    color: var(--text-dim);
    white-space: nowrap;
    font-variant-numeric: tabular-nums;
  }
  .bar {
    grid-column: 1 / -1;
    display: flex;
    height: 8px;
    margin-top: 6px;
    border-radius: 4px;
    overflow: hidden;
    background: var(--field-bg-hover);
  }
  .bar.thin {
    height: 4px;
    margin-top: 2px;
    border-radius: 2px;
  }
  .bar .fill {
    background: var(--accent);
    border-radius: inherit;
    transition: width 0.6s ease;
  }
  .bar .others {
    background: var(--text-faint);
    opacity: 0.45;
    transition: width 0.6s ease;
  }
  .legend {
    display: flex;
    align-items: center;
    gap: 5px;
  }
  .dot {
    display: inline-block;
    width: 7px;
    height: 7px;
    border-radius: 50%;
  }
  .dot.others {
    margin-left: 8px;
    background: var(--text-faint);
    opacity: 0.7;
  }
  .dot.mine {
    background: var(--accent);
  }
  .parts {
    display: flex;
    flex-direction: column;
    gap: 13px;
    padding: 12px 14px 13px;
  }
  .part {
    display: flex;
    flex-direction: column;
    gap: 3px;
  }
  .part-line {
    display: flex;
    gap: 8px;
    align-items: baseline;
    font-size: 12px;
  }
  .part-name {
    color: var(--text);
    font-weight: 500;
  }
  .part-figures {
    margin-left: auto;
    color: var(--text-dim);
    white-space: nowrap;
    font-variant-numeric: tabular-nums;
  }
  .part-sub {
    font-size: 11px;
    line-height: 1.4;
    color: var(--text-faint);
  }
</style>
