<script lang="ts">
  import { onMount } from "svelte";
  import { listModels, downloadModel, selectModel, setPictureModel } from "../api";
  import type { ModelInfo } from "../types";
  import { formatBytes } from "../format";
  import Icon from "./Icon.svelte";

  let models = $state<ModelInfo[]>([]);
  let loading = $state(true);

  async function refresh() {
    models = await listModels();
  }

  onMount(async () => {
    await refresh();
    loading = false;
  });

  async function onDownload(m: ModelInfo) {
    await downloadModel(m.id, (p) => {
      const found = models.find((x) => x.id === m.id);
      if (found) found.downloadProgress = p;
    });
    await refresh();
  }
  async function onSelect(m: ModelInfo) {
    if (!m.installed) return;
    await selectModel(m.id);
    await refresh();
  }

  async function onPictures(id: string | null) {
    error = "";
    try {
      await setPictureModel(id);
    } catch (e) {
      error = String(e);
    }
    await refresh();
  }

  let error = $state("");
  const chat = $derived(models.filter((m) => m.type === "chat"));
  const embed = $derived(models.filter((m) => m.type === "embed"));
  const vision = $derived(models.filter((m) => m.type === "vision"));
</script>

<div class="models">
  {#if loading}
    <div class="muted">Loading…</div>
  {:else}
    <div class="section-label">Chat model</div>
    {#each chat as m (m.id)}
      {@render row(m)}
    {/each}

    <div class="section-label embed-label">Embedding model</div>
    <div class="hint">Used to search your files. Changing it re-indexes your folders.</div>
    {#each embed as m (m.id)}
      {@render row(m)}
    {/each}

    {#if vision.length}
      <div class="section-label embed-label">Picture descriptions <span class="optional">optional</span></div>
      <div class="hint">
        Describes each picture so it can be found by what it shows, not only by the text on it.
        Descriptions are short and approximate. Pictures are read again when this changes.
      </div>
      {#each vision as m (m.id)}
        <div class="model card" class:selected={m.selected}>
          <div class="model-main">
            <div class="name">{m.name}</div>
            <div class="meta">{m.params} · {m.quant} · {formatBytes(m.sizeBytes)}</div>
            {#if m.downloadProgress != null}
              <div class="progress"><div style="width:{m.downloadProgress * 100}%"></div></div>
            {/if}
          </div>
          <div class="model-action">
            {#if m.downloadProgress != null}
              <span class="pct">{Math.round(m.downloadProgress * 100)}%</span>
            {:else if !m.installed}
              <button class="dl" onclick={() => onDownload(m)}><Icon name="download" size={13} stroke={2.2} />Download</button>
            {:else if m.selected}
              <span class="active"><Icon name="check" size={13} stroke={2.6} />On</span>
              <button class="use off" onclick={() => onPictures(null)} title="Stop describing pictures; they stay findable by the text on them">Turn off</button>
            {:else}
              <button class="use" onclick={() => onPictures(m.id)} title="Describe pictures with this model when folders are indexed">Turn on</button>
            {/if}
          </div>
        </div>
      {/each}
      {#if error}<div class="hint error">{error}</div>{/if}
    {/if}
  {/if}
</div>

{#snippet row(m: ModelInfo)}
  <div class="model card" class:selected={m.selected}>
    <div class="model-main">
      <div class="name">{m.name}</div>
      <div class="meta">{m.params} · {m.quant} · {formatBytes(m.sizeBytes)} · {m.ctx.toLocaleString()} ctx</div>
      {#if m.downloadProgress != null}
        <div class="progress"><div style="width:{m.downloadProgress * 100}%"></div></div>
      {/if}
    </div>
    <div class="model-action">
      {#if m.downloadProgress != null}
        <span class="pct">{Math.round(m.downloadProgress * 100)}%</span>
      {:else if !m.installed}
        <button class="dl" onclick={() => onDownload(m)}><Icon name="download" size={13} stroke={2.2} />Download</button>
      {:else if m.selected}
        <span class="active"><Icon name="check" size={13} stroke={2.6} />Active</span>
      {:else}
        <button class="use" onclick={() => onSelect(m)}>Use</button>
      {/if}
    </div>
  </div>
{/snippet}

<style>
  .models {
    display: flex;
    flex-direction: column;
    gap: 8px;
  }
  .muted {
    color: var(--text-faint);
    font-size: 13px;
  }
  .section-label {
    margin: 4px 0 0 2px;
  }
  .embed-label {
    margin-top: 12px;
  }
  .hint {
    font-size: 11.5px;
    color: var(--text-faint);
    margin: -3px 0 1px 2px;
  }
  .optional {
    margin-left: 6px;
    font-size: 9.5px;
    letter-spacing: 0.6px;
    color: var(--text-faint);
    border: 1px solid var(--notch-border);
    border-radius: 5px;
    padding: 1px 5px;
  }
  .hint {
    line-height: 1.4;
  }
  .hint.error {
    color: var(--warn);
  }
  .model-action {
    display: flex;
    align-items: center;
    gap: 8px;
    flex: none;
  }
  .use.off {
    font-weight: 500;
    color: var(--text-dim);
  }
  .model {
    display: flex;
    align-items: center;
    gap: 10px;
    padding: 11px 12px 11px 14px;
    transition:
      background 0.15s ease,
      border-color 0.15s ease;
  }
  .model.selected {
    border-color: color-mix(in srgb, var(--accent) 55%, transparent);
    background: color-mix(in srgb, var(--accent) 11%, transparent);
  }
  .model-main {
    flex: 1;
    min-width: 0;
  }
  .name {
    font-size: 13.5px;
    font-weight: 600;
  }
  .meta {
    font-size: 11.5px;
    color: var(--text-faint);
    margin-top: 2px;
  }
  .progress {
    margin-top: 8px;
  }
  .dl,
  .use {
    display: inline-flex;
    align-items: center;
    gap: 6px;
    font-size: 12.5px;
    font-weight: 600;
    padding: 7px 12px;
    border-radius: 10px;
    background: var(--field-bg-hover);
    color: var(--text);
    transition:
      background 0.14s ease,
      color 0.14s ease,
      transform 0.12s ease;
  }
  .dl:hover {
    background: var(--accent-grad);
    color: white;
  }
  .use:hover {
    background: var(--accent);
    color: white;
  }
  .dl:active,
  .use:active {
    transform: scale(0.96);
  }
  .pct {
    font-size: 12px;
    font-variant-numeric: tabular-nums;
    color: var(--text-dim);
    min-width: 34px;
    text-align: right;
  }
  .active {
    display: inline-flex;
    align-items: center;
    gap: 5px;
    font-size: 12px;
    font-weight: 600;
    color: var(--accent);
    padding: 0 4px;
  }
</style>
