<script lang="ts">
  import { onMount } from "svelte";
  import {
    listFolders,
    addFolder,
    reindexFolder,
    deleteIndex,
    removeFolder,
    onFoldersChanged,
  } from "../api";
  import type { FolderInfo } from "../types";
  import { basename } from "../format";
  import Icon from "./Icon.svelte";

  let folders = $state<FolderInfo[]>([]);
  let loading = $state(true);
  let actionError = $state("");

  async function refresh() {
    try {
      folders = await listFolders();
    } catch (e) {
      actionError = String(e);
    }
  }

  onMount(() => {
    let unsub = () => {};
    refresh().then(() => (loading = false));
    // Live status + progress pushed from the indexer.
    onFoldersChanged(refresh).then((un) => (unsub = un));
    return () => unsub();
  });

  async function run(action: () => Promise<unknown>) {
    actionError = "";
    try {
      await action();
    } catch (e) {
      actionError = String(e);
    }
    await refresh();
  }

  const onAdd = () => run(addFolder);
  const onReindex = (id: string) => run(() => reindexFolder(id));
  const onDeleteIndex = (id: string) => run(() => deleteIndex(id));
  const onRemove = (id: string) => run(() => removeFolder(id));

  function statusLabel(f: FolderInfo): string {
    switch (f.status) {
      case "indexing":
        return f.progress && f.progress.total > 0
          ? `Indexing ${f.progress.done}/${f.progress.total}`
          : "Scanning…";
      case "ready":
        return `${f.fileCount} files · ${f.chunkCount} chunks · watching`;
      case "pending":
        return "Waiting to index…";
      case "error":
        return f.error ?? "Error";
      default:
        return "Not indexed";
    }
  }
</script>

<div class="folders">
  <button class="add" onclick={onAdd}>
    <span class="add-icon"><Icon name="plus" size={15} stroke={2.2} /></span>
    Add a folder
  </button>
  {#if actionError}<div class="note note-error">{actionError}</div>{/if}

  {#if loading}
    <div class="muted">Loading…</div>
  {:else if folders.length === 0}
    <div class="empty">
      <span class="empty-icon"><Icon name="folder" size={22} stroke={1.6} /></span>
      No folders yet. Add one and Loft will index it and keep it up to date.
    </div>
  {:else}
    {#each folders as f (f.id)}
      <div class="folder card">
        <span class="folder-icon status-{f.status}"><Icon name="folder" size={17} stroke={1.7} /></span>
        <div class="folder-main">
          <div class="name" title={f.path}>{basename(f.path)}</div>
          <div class="path">{f.path}</div>
          <div class="status status-{f.status}">
            {#if f.status === "indexing"}<span class="spin"></span>{/if}
            {statusLabel(f)}
          </div>
          {#if f.status === "indexing" && f.progress}
            <div class="progress"><div style="width:{(f.progress.done / Math.max(f.progress.total, 1)) * 100}%"></div></div>
            {#if f.progress.current}
              <div class="current">
                {f.progress.current}{#if (f.progress.chunksTotal ?? 0) > 100}<span class="chunks">
                    &nbsp;· {(f.progress.chunksDone ?? 0).toLocaleString()} of {(f.progress.chunksTotal ?? 0).toLocaleString()} chunks</span
                  >{/if}
              </div>
            {/if}
          {/if}
          {#if f.error && f.status !== "error"}<div class="note">{f.error}</div>{/if}
        </div>
        <div class="actions">
          <button class="icon-btn" title="Rebuild this folder's index" aria-label="Rebuild index" onclick={() => onReindex(f.id)}>
            <Icon name="refresh" size={14} />
          </button>
          <button class="icon-btn" title="Delete indexed data (keeps the folder in the list)" aria-label="Delete indexed data" onclick={() => onDeleteIndex(f.id)}>
            <Icon name="trash" size={14} />
          </button>
          <button class="icon-btn danger" title="Remove folder from Loft (your files are not touched)" aria-label="Remove folder" onclick={() => onRemove(f.id)}>
            <Icon name="x" size={15} />
          </button>
        </div>
      </div>
    {/each}
  {/if}
</div>

<style>
  .folders {
    display: flex;
    flex-direction: column;
    gap: 9px;
  }
  .add {
    display: flex;
    align-items: center;
    justify-content: center;
    gap: 8px;
    font-size: 13px;
    font-weight: 600;
    color: var(--text-dim);
    border: 1px dashed var(--border-strong);
    border-radius: 14px;
    padding: 11px 13px;
    transition:
      background 0.14s ease,
      color 0.14s ease,
      border-color 0.14s ease;
  }
  .add:hover {
    color: var(--text);
    background: var(--field-bg);
    border-color: color-mix(in srgb, var(--accent) 60%, transparent);
  }
  .add-icon {
    display: grid;
    color: var(--accent);
  }
  .muted {
    color: var(--text-faint);
    font-size: 13px;
    padding: 8px 2px;
  }
  .empty {
    display: flex;
    flex-direction: column;
    align-items: center;
    gap: 10px;
    text-align: center;
    color: var(--text-faint);
    font-size: 13px;
    line-height: 1.5;
    padding: 34px 40px;
  }
  .empty-icon {
    display: grid;
    place-items: center;
    width: 46px;
    height: 46px;
    border-radius: 14px;
    background: var(--field-bg);
    color: var(--text-dim);
  }
  .folder {
    display: flex;
    gap: 11px;
    align-items: flex-start;
    padding: 12px 8px 12px 12px;
  }
  .folder-icon {
    flex: none;
    display: grid;
    place-items: center;
    width: 32px;
    height: 32px;
    border-radius: 10px;
    background: var(--field-bg);
    color: var(--text-dim);
  }
  .folder-icon.status-ready {
    color: var(--accent);
    background: color-mix(in srgb, var(--accent) 14%, transparent);
  }
  .folder-main {
    flex: 1;
    min-width: 0;
  }
  .name {
    font-size: 13.5px;
    font-weight: 600;
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
  }
  .path {
    font-size: 11.5px;
    color: var(--text-faint);
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
    margin: 1px 0 6px;
  }
  .status {
    display: inline-flex;
    align-items: center;
    gap: 6px;
    font-size: 12px;
    color: var(--text-dim);
  }
  .status::before {
    content: "";
    width: 6px;
    height: 6px;
    border-radius: 50%;
    background: var(--text-faint);
  }
  .status.status-ready::before {
    background: var(--good);
  }
  .status.status-error {
    color: var(--bad);
  }
  .status.status-error::before {
    background: var(--bad);
  }
  .status.status-indexing,
  .status.status-pending {
    color: var(--warn);
  }
  .status.status-pending::before {
    background: var(--warn);
  }
  .status.status-indexing::before {
    display: none; /* the spinner takes its place */
  }
  .current {
    font-size: 11px;
    color: var(--text-faint);
    margin-top: 4px;
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
  }
  .note {
    font-size: 11.5px;
    line-height: 1.4;
    color: var(--warn);
    margin-top: 4px;
  }
  .note-error {
    color: var(--bad);
  }
  .progress {
    margin-top: 8px;
  }
  .actions {
    display: flex;
    gap: 1px;
    opacity: 0.55;
    transition: opacity 0.15s ease;
  }
  .folder:hover .actions,
  .actions:focus-within {
    opacity: 1;
  }
  .actions .danger:hover {
    color: var(--bad);
  }
  .spin {
    width: 10px;
    height: 10px;
    border: 2px solid var(--warn);
    border-top-color: transparent;
    border-radius: 50%;
    animation: spin 0.7s linear infinite;
  }
  @keyframes spin {
    to { transform: rotate(360deg); }
  }
</style>
