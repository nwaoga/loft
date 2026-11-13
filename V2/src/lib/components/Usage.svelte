<script lang="ts">
  import { getUsage } from "../api";
  import { ui } from "../state.svelte";
  import type { Usage, UsagePart } from "../types";

  // "What Loft is using": memory, graphics memory and processor for each part
  // of Loft, read every couple of seconds while Settings is open.
  const EVERY_MS = 2000;

  let usage = $state<Usage | null>(null);
  let failed = $state("");
  let open = $state<Record<string, boolean>>({});

  $effect(() => {
    if (ui.tab !== "settings") return;
    let stopped = false;
    let timer: ReturnType<typeof setTimeout> | undefined;
    async function read() {
      if (stopped) return;
      // A hidden window does not need the numbers.
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
  const cpu = (percent?: number | null) => (percent == null ? "…" : `${percent < 10 ? percent.toFixed(1) : percent.toFixed(0)}%`);
  const tokens = (n: number) => n.toLocaleString();


  const share = $derived(
    usage?.systemTotalBytes ? Math.min(100, (usage.memoryBytes / usage.systemTotalBytes) * 100) : null,
  );
  const inUse = $derived(
    usage?.systemTotalBytes != null && usage.systemAvailableBytes != null ? usage.systemTotalBytes - usage.systemAvailableBytes : null,
  );
  /** The model whose context takes the most memory, when that is most of its memory. */
  const bigContext = $derived(
    usage?.parts.find(
      (p) => p.kind === "chat" && p.contextBytes && p.weightsBytes && p.contextBytes > 0.5 * p.weightsBytes && p.contextBytes > 512 * 1024 ** 2,
    ),
  );
</script>

<div class="usage card">
  <div class="head">
    <span class="title">What Loft is using</span>
    {#if usage}
      <span class="totals">
        {size(usage.memoryBytes)} memory{#if usage.deviceBytes > 0}&nbsp;· {size(usage.deviceBytes)} graphics card{/if}&nbsp;· processor {cpu(usage.cpuPercent)}
      </span>
    {/if}
  </div>

  {#if !usage}
    <div class="faint">{failed || "Measuring…"}</div>
  {:else}
    {#if share != null}
      <div class="meter" title="Loft's share of this computer's memory">
        <span class="mine" style="width: {share}%"></span>
        {#if inUse != null && usage.systemTotalBytes}
          <span class="others" style="width: {Math.max(0, Math.min(100 - share, ((inUse - usage.memoryBytes) / usage.systemTotalBytes) * 100))}%"></span>
        {/if}
      </div>
      <div class="faint legend">
        <span><i class="dot mine"></i>Loft {size(usage.memoryBytes)}</span>
        {#if inUse != null}<span><i class="dot others"></i>everything else {size(Math.max(0, inUse - usage.memoryBytes))}</span>{/if}
        <span>of {size(usage.systemTotalBytes)}{#if usage.systemAvailableBytes != null}, {size(usage.systemAvailableBytes)} free{/if}</span>
      </div>
    {/if}

    {#each usage.parts as p (p.kind)}
      <button class="part" aria-expanded={!!open[p.kind]} onclick={() => (open[p.kind] = !open[p.kind])}>
        <span class="part-line">
          <span class="part-name">{p.label}</span>
          <span class="part-figures">
            {size(p.memoryBytes)}{#if p.device && !p.device.shared}&nbsp;+ {size(p.device.bytes)} on the card{/if}&nbsp;· {cpu(p.cpuPercent)}
          </span>
        </span>
        {#if p.model || p.runsOn}
          <span class="part-sub">{[p.model, p.runsOn].filter(Boolean).join(" · ")}</span>
        {/if}
        {#if open[p.kind]}
          <span class="detail">
            {#if p.weightsBytes}
              <span>
                {`Model ${size(p.weightsBytes)}`}{p.contextBytes
                  ? `, context ${size(p.contextBytes)}${p.contextTokens ? ` (room for ${tokens(p.contextTokens)} tokens)` : ""}`
                  : ""}
              </span>
            {/if}
            {#if p.device}
              <span>
                {#if p.device.shared}
                  {size(p.device.bytes)} of its memory is used by {p.device.name}, which has no memory of its own
                {:else}
                  {size(p.device.bytes)} on {p.device.name}
                {/if}
              </span>
            {/if}
            <span>{size(p.memoryBytes)} of system memory in use · {p.processes} {p.processes === 1 ? "process" : "processes"}</span>
          </span>
        {/if}
      </button>
    {/each}

    {#if bigContext}
      <p class="faint">
        The chat model's context takes {size(bigContext.contextBytes)}: room for {tokens(bigContext.contextTokens ?? 0)} tokens of conversation
        and excerpts, set aside in advance whether or not it is used.
      </p>
    {/if}
    <p class="faint">
      Processor is the share of all {usage.cores} cores, so 100% is every core flat out. Memory on a graphics card does not count against
      system memory.
    </p>
  {/if}
</div>

<style>
  .usage {
    display: flex;
    flex-direction: column;
    gap: 7px;
    padding: 10px 12px 11px 14px;
  }
  .head {
    display: flex;
    align-items: baseline;
    gap: 8px;
    flex-wrap: wrap;
  }
  .title {
    font-size: 12px;
    font-weight: 600;
    color: var(--text);
  }
  .totals {
    margin-left: auto;
    font-size: 11.5px;
    color: var(--text-dim);
    font-variant-numeric: tabular-nums;
  }
  .faint {
    margin: 0;
    font-size: 11px;
    line-height: 1.4;
    color: var(--text-faint);
  }
  .meter {
    display: flex;
    height: 6px;
    border-radius: 3px;
    overflow: hidden;
    background: var(--field-bg-hover);
  }
  .meter .mine {
    background: var(--accent);
  }
  .meter .others {
    background: var(--text-faint);
    opacity: 0.6;
  }
  .legend {
    display: flex;
    flex-wrap: wrap;
    gap: 4px 12px;
    font-variant-numeric: tabular-nums;
  }
  .dot {
    display: inline-block;
    width: 7px;
    height: 7px;
    margin-right: 5px;
    border-radius: 50%;
    vertical-align: 0;
  }
  .dot.mine {
    background: var(--accent);
  }
  .dot.others {
    background: var(--text-faint);
  }
  .part {
    display: flex;
    flex-direction: column;
    gap: 2px;
    width: 100%;
    padding: 6px 8px;
    margin: 0 -8px;
    box-sizing: content-box;
    border: 0;
    border-radius: 8px;
    background: none;
    color: inherit;
    font: inherit;
    text-align: left;
    cursor: pointer;
  }
  .part:hover {
    background: var(--field-bg-hover);
  }
  .part-line {
    display: flex;
    gap: 8px;
    align-items: baseline;
    font-size: 12px;
  }
  .part-name {
    color: var(--text);
  }
  .part-figures {
    margin-left: auto;
    color: var(--text-dim);
    white-space: nowrap;
    font-variant-numeric: tabular-nums;
  }
  .part-sub,
  .detail {
    font-size: 11px;
    line-height: 1.4;
    color: var(--text-faint);
  }
  .detail {
    display: flex;
    flex-direction: column;
    gap: 1px;
    margin-top: 2px;
    color: var(--text-dim);
  }
</style>
