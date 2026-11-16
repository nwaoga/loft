<script lang="ts">
  import { onMount } from "svelte";
  import { ui, drop, expand, collapse, toggle, toggleLarge } from "./lib/state.svelte";
  import {
    isTauri,
    getRuntimeStatus,
    onRuntimeStatus,
    onToggle,
    listFolders,
    onFoldersChanged,
    movedByLoft,
    onWindowMoved,
    startWindowDrag,
    onFileDrop,
    addDocuments,
  } from "./lib/api";
  import type { AddDocumentsResult } from "./lib/types";
  import { windowStyle, windowMoved } from "./lib/window-style.svelte";
  import type { FolderInfo, RuntimeStatus } from "./lib/types";
  import type { Tab } from "./lib/state.svelte";
  import Ask from "./lib/components/Ask.svelte";
  import Folders from "./lib/components/Folders.svelte";
  import Settings from "./lib/components/Settings.svelte";
  import Usage from "./lib/components/Usage.svelte";
  import Icon from "./lib/components/Icon.svelte";

  let runtime = $state<RuntimeStatus | null>(null);
  let folders = $state<FolderInfo[] | null>(null);

  // Settings (the gear) and Usage are views of their own, opened from the
  // header; leaving one returns to the tab that was open before.
  let tabBefore: Tab = "ask";
  const isView = (tab: Tab) => tab === "settings" || tab === "usage";
  function toggleView(view: "settings" | "usage") {
    if (ui.tab === view) {
      ui.tab = tabBefore;
    } else {
      if (!isView(ui.tab)) tabBefore = ui.tab;
      ui.tab = view;
    }
  }

  async function refreshFolders() {
    try {
      folders = await listFolders();
    } catch {
      /* the Folders tab reports errors; the pill just keeps its last state */
    }
  }

  // In a plain browser, paint a desktop-like backdrop so the notch is visible.
  if (!isTauri()) {
    document.documentElement.classList.add("preview");
  }

  onMount(() => {
    const cleanups: Array<() => void> = [];

    getRuntimeStatus().then((s) => (runtime = s));

    // Live runtime status + hotkey toggle from the Rust backend.
    onRuntimeStatus((s) => (runtime = s)).then((un) => cleanups.push(un));
    onToggle(() => toggle()).then((un) => cleanups.push(un));

    // Folder state drives the pill's "Indexing…" hint and the Ask tab's guidance.
    refreshFolders();
    onFoldersChanged(refreshFolders).then((un) => cleanups.push(un));

    // Files dragged onto the window: copy them into the Loft library and index
    // them. The chat window is the drop surface, so a drop opens the Ask view.
    onFileDrop({
      onOver() {
        drop.dragging = true;
        if (!ui.expanded) expand("ask");
      },
      onLeave() {
        drop.dragging = false;
      },
      onDrop(paths) {
        drop.dragging = false;
        handleDrop(paths);
      },
    }).then((un) => cleanups.push(un));

    // Collapse when clicking outside the notch (preview + Tauri both benefit)
    const onDown = (e: MouseEvent) => {
      const el = e.target as HTMLElement;
      if (ui.expanded && !el.closest(".notch")) collapse();
    };
    window.addEventListener("mousedown", onDown);
    cleanups.push(() => window.removeEventListener("mousedown", onDown));

    // Floating: once a drag has settled, remember where the window was left.
    let settle: ReturnType<typeof setTimeout> | undefined;
    onWindowMoved(() => {
      clearTimeout(settle);
      settle = setTimeout(() => {
        if (!movedByLoft()) windowMoved();
      }, 350);
    }).then((un) => cleanups.push(un));

    const onKey = (e: KeyboardEvent) => {
      if (e.key !== "Escape") return;
      if (isView(ui.tab)) ui.tab = tabBefore; // Esc backs out of Settings or Usage first
      else collapse();
    };
    window.addEventListener("keydown", onKey);
    cleanups.push(() => window.removeEventListener("keydown", onKey));

    return () => cleanups.forEach((fn) => fn());
  });

  // Floating: the pill and the top of the panel are handles to drag the
  // window by. A press that moves is a drag; one that does not is a click.
  let pressAt: { x: number; y: number } | null = null;
  let dragged = false;
  function pillDown(e: PointerEvent) {
    dragged = false;
    pressAt = windowStyle.value === "floating" && e.button === 0 ? { x: e.screenX, y: e.screenY } : null;
  }
  function pillMove(e: PointerEvent) {
    if (!pressAt || Math.hypot(e.screenX - pressAt.x, e.screenY - pressAt.y) < 4) return;
    pressAt = null;
    dragged = true;
    startWindowDrag().catch(() => {});
  }
  function pillClick() {
    if (dragged) return; // that press moved the window
    if (ui.expanded) collapse();
    else expand("ask");
  }
  function headerDown(e: PointerEvent) {
    if (windowStyle.value !== "floating" || e.button !== 0) return;
    if ((e.target as HTMLElement).closest("button")) return; // the tabs and buttons still work
    startWindowDrag().catch(() => {});
  }

  // A drop lands here: show the Ask view, add the files, then report the result
  // in the chat view. Indexing itself runs in the background (the Folders
  // state and the status bar show its progress).
  async function handleDrop(paths: string[]) {
    if (!paths.length) return;
    expand("ask");
    drop.note = { text: paths.length === 1 ? "Adding…" : `Adding ${paths.length} items…`, tone: "ok" };
    try {
      drop.note = summariseDrop(await addDocuments(paths));
    } catch (e) {
      drop.note = { text: `Could not add that: ${e}`, tone: "warn" };
    }
    refreshFolders();
  }

  function summariseDrop(r: AddDocumentsResult): { text: string; tone: "ok" | "warn" } {
    const n = (list: string[], one: (name: string) => string, many: (count: number) => string) =>
      list.length === 1 ? one(list[0]) : many(list.length);
    const parts: string[] = [];
    if (r.added.length) parts.push(n(r.added, (f) => `Added ${f} — ask away.`, (c) => `Added ${c} files — ask away.`));
    if (r.foldersAdded.length)
      parts.push(n(r.foldersAdded, (f) => `Added folder ${f}.`, (c) => `Added ${c} folders.`));
    if (r.storedOnly.length)
      parts.push(n(r.storedOnly, (f) => `Kept ${f} (that type can't be indexed).`, (c) => `Kept ${c} files that can't be indexed.`));
    if (r.duplicates.length)
      parts.push(n(r.duplicates, (f) => `${f} was already in Loft.`, (c) => `${c} were already in Loft.`));
    if (r.failed.length) parts.push(n(r.failed, () => `One item couldn't be added.`, (c) => `${c} items couldn't be added.`));
    const nothingAdded = !r.added.length && !r.foldersAdded.length && !r.storedOnly.length;
    const tone: "ok" | "warn" = r.failed.length && nothingAdded ? "warn" : "ok";
    return { text: parts.join(" ") || "Nothing to add.", tone };
  }

  const ready = $derived(
    runtime ? runtime.chat === "ready" && runtime.embed === "ready" : false,
  );
  const statusClass = $derived(
    !runtime
      ? "loading"
      : runtime.chat === "error" || runtime.embed === "error"
        ? "bad"
        : ready
          ? "good"
          : runtime.chat === "loading" || runtime.embed === "loading"
            ? "loading"
            : "idle",
  );

  const indexing = $derived(
    (folders ?? []).some((f) => f.status === "indexing" || f.status === "pending"),
  );

  // One-line summary for the panel's status bar.
  const statusText = $derived.by(() => {
    if (!runtime) return "Starting…";
    if (runtime.chat === "error" || runtime.embed === "error") return "A model failed to start";
    if (!ready) {
      return runtime.chat === "loading" || runtime.embed === "loading"
        ? "Loading models…"
        : "No models selected";
    }
    const list = folders ?? [];
    const active = list.find((f) => f.status === "indexing" && f.progress && f.progress.total > 0);
    if (active?.progress) return `Indexing ${active.progress.done} of ${active.progress.total} files`;
    if (indexing) return "Indexing…";
    if (list.length === 0) return "Ready · no folders yet";
    const files = list.reduce((n, f) => n + f.fileCount, 0);
    return `Ready · ${files.toLocaleString()} ${files === 1 ? "file" : "files"} in ${list.length} ${list.length === 1 ? "folder" : "folders"}`;
  });

  // What stands between the user and an answer, if anything, and where to fix it.
  const askHint = $derived.by((): { text: string; tab?: Tab } | null => {
    if (!runtime) return { text: "Starting…" };
    if (runtime.chat === "error" || runtime.embed === "error") {
      return { text: runtime.message ?? "A model failed to start. Select it again in Settings.", tab: "settings" };
    }
    if (runtime.chat === "stopped" || runtime.embed === "stopped") {
      return {
        text: runtime.message ?? "Download and select a chat model and an embedding model to get started.",
        tab: "settings",
      };
    }
    if (!ready) return { text: "Loading the models…" };
    if (folders && folders.length === 0) {
      return { text: "No folders yet. Add one and Loft will index it.", tab: "folders" };
    }
    if (folders && folders.every((f) => f.chunkCount === 0)) {
      return indexing
        ? { text: "Indexing your folders. You can ask as soon as the first files are in." }
        : { text: "Nothing is indexed. Rebuild a folder's index in Folders.", tab: "folders" };
    }
    return null;
  });
</script>

<div class="stage">
  <div class="notch" class:expanded={ui.expanded}>
    <!-- Concave corners where the notch meets the top edge of the screen -->
    <span class="ear left"></span>
    <span class="ear right"></span>

    <div class="clip">
      <!-- Collapsed pill -->
      <button
        class="pill"
        onclick={pillClick}
        onpointerdown={pillDown}
        onpointermove={pillMove}
        aria-label="Open Loft"
        tabindex={ui.expanded ? -1 : 0}
      >
        <span class="dot {indexing && ready ? 'loading' : statusClass}"></span>
        <span class="brand">Loft</span>
        <span class="hint">{indexing ? "Indexing…" : "Ask your files"}</span>
      </button>

      <!-- Expanded panel -->
      <div class="panel" inert={!ui.expanded}>
        <!-- svelte-ignore a11y_no_static_element_interactions -->
        <header class="tabs" onpointerdown={headerDown}>
          <div class="segmented">
            <button
              class:active={ui.tab === "ask"}
              onclick={() => (ui.tab = "ask")}
              title={ui.asking ? "An answer is being written" : undefined}
            >Ask{#if ui.asking}<span class="answering" aria-hidden="true"></span>{/if}</button>
            <button class:active={ui.tab === "folders"} onclick={() => (ui.tab = "folders")}>Folders</button>
          </div>
          <span class="spacer"></span>
          <button
            class="icon-btn"
            class:on={ui.large}
            onclick={toggleLarge}
            aria-label="Larger window"
            aria-pressed={ui.large}
            title={ui.large ? "Back to the normal size" : "Twice as large, to see more"}
          >
            <Icon name={ui.large ? "minimize" : "maximize"} size={14} />
          </button>
          <button
            class="icon-btn"
            class:on={ui.tab === "usage"}
            onclick={() => toggleView("usage")}
            aria-label="Usage"
            aria-pressed={ui.tab === "usage"}
            title={ui.tab === "usage" ? "Close usage" : "Usage: memory, graphics card and processor"}
          >
            <Icon name="activity" size={15} />
          </button>
          <button
            class="icon-btn"
            class:on={ui.tab === "settings"}
            onclick={() => toggleView("settings")}
            aria-label="Settings"
            aria-pressed={ui.tab === "settings"}
            title={ui.tab === "settings" ? "Close settings" : "Settings: theme, models, launch at login"}
          >
            <Icon name="settings" size={15} />
          </button>
          <button class="icon-btn collapse" onclick={collapse} aria-label="Collapse" title="Collapse (Esc)">
            <Icon name="chevron-up" size={17} />
          </button>
        </header>

        <div class="body scroll">
          <!-- All three stay mounted, so an answer survives a look at another tab. -->
          <div class="tab-content fill" class:shown={ui.tab === "ask"}><Ask {ready} hint={askHint} /></div>
          <div class="tab-content" class:shown={ui.tab === "folders"}><Folders /></div>
          <div class="tab-content" class:shown={ui.tab === "settings"}><Settings /></div>
          <div class="tab-content" class:shown={ui.tab === "usage"}><Usage /></div>
        </div>

        <footer class="statusbar">
          <span class="dot {indexing && ready ? 'loading' : statusClass}"></span>
          <span class="status-text">{statusText}</span>
          <span class="accel" title={runtime?.message ?? "What the models run on"}>{runtime?.accelerator ?? "…"}</span>
        </footer>

        {#if drop.dragging}
          <div class="dropzone" aria-hidden="true">
            <Icon name="download" size={28} />
            <p class="drop-title">Drop to add to Loft</p>
            <p class="drop-sub">Files are kept in your Loft library and indexed so you can ask about them.</p>
          </div>
        {/if}
      </div>
    </div>
  </div>
</div>

<style>
  .stage {
    width: 100%;
    display: flex;
    justify-content: center;
    align-items: flex-start;
  }

  /* The notch hangs from the top-centre of the screen. Solid black, with
     concave "ears" joining it to the screen edge like the hardware notch. */
  .notch {
    position: relative;
    width: 236px;
    height: 36px;
    background: var(--notch);
    border-radius: 0 0 var(--radius-pill) var(--radius-pill);
    box-shadow: var(--pill-shadow);
    transform-origin: top center;
    transition:
      width 0.46s var(--spring),
      height 0.46s var(--spring),
      border-radius 0.46s var(--ease),
      box-shadow 0.3s ease,
      transform 0.25s var(--spring);
  }
  .notch:not(.expanded):hover {
    transform: scale(1.045);
  }
  /* The panel's size: 460x520, or larger (the switch in the header) as the
     backend says; it sizes the window to match (PANEL_SIZE in lib.rs). */
  .notch.expanded {
    width: var(--panel-w, 460px);
    height: var(--panel-h, 520px);
    border-radius: 0 0 var(--radius-panel) var(--radius-panel);
    box-shadow: var(--notch-shadow);
  }

  .ear {
    position: absolute;
    top: 0;
    width: var(--ear);
    height: var(--ear);
    pointer-events: none;
  }
  .ear.left {
    left: calc(-1 * var(--ear));
    background: radial-gradient(circle at 0 100%, transparent calc(var(--ear) - 0.5px), var(--notch) var(--ear));
  }
  .ear.right {
    right: calc(-1 * var(--ear));
    background: radial-gradient(circle at 100% 100%, transparent calc(var(--ear) - 0.5px), var(--notch) var(--ear));
  }

  /* Docked on the right edge (Settings → Placement): the notch hangs from the
     right, centred vertically. The pill stands upright and the panel opens
     leftwards; the ears sit above and below, against the screen edge. Window
     sizes for this are COLLAPSED_SIZE_RIGHT / EXPANDED_SIZE_RIGHT in lib.rs. */
  :global(html[data-placement="right"]) .stage {
    height: 100%;
    justify-content: flex-end;
    align-items: center;
  }
  :global(html[data-placement="right"]) .notch {
    width: 36px;
    height: 190px;
    border-radius: var(--radius-pill) 0 0 var(--radius-pill);
    transform-origin: right center;
  }
  :global(html[data-placement="right"]) .notch.expanded {
    width: var(--panel-w, 460px);
    height: var(--panel-h, 520px);
    border-radius: var(--radius-panel) 0 0 var(--radius-panel);
  }
  :global(html[data-placement="right"]) .ear.left {
    left: auto;
    right: 0;
    top: calc(-1 * var(--ear));
    background: radial-gradient(circle at 0 0, transparent calc(var(--ear) - 0.5px), var(--notch) var(--ear));
  }
  :global(html[data-placement="right"]) .ear.right {
    right: 0;
    top: auto;
    bottom: calc(-1 * var(--ear));
    background: radial-gradient(circle at 0 100%, transparent calc(var(--ear) - 0.5px), var(--notch) var(--ear));
  }
  :global(html[data-placement="right"]) .pill {
    top: 50%;
    left: auto;
    right: 0;
    transform: translateY(-50%);
    flex-direction: column;
    width: 36px;
    height: 190px;
    padding: 15px 0 14px 1px;
  }
  :global(html[data-placement="right"]) .pill .brand,
  :global(html[data-placement="right"]) .pill .hint {
    writing-mode: vertical-rl;
  }
  :global(html[data-placement="right"]) .pill .hint {
    margin-left: 0;
    margin-top: auto;
  }
  :global(html[data-placement="right"]) .panel {
    top: 50%;
    left: auto;
    right: 0;
    transform: translateY(-50%);
  }
  /* The collapse arrow points at the edge the panel folds back into. */
  :global(html[data-placement="right"]) .collapse :global(svg) {
    transform: rotate(90deg);
  }

  /* Floating (Settings → Window): not attached to any edge. A capsule, and
     a card with all four corners round, a little inside the window so the
     shadow has room above as well. Dragged by the pill or the panel's top. */
  :global(html[data-style="floating"]) .stage {
    padding-top: 14px;
  }
  :global(html[data-style="floating"]) .notch {
    border-radius: 18px;
  }
  :global(html[data-style="floating"]) .notch.expanded {
    border-radius: var(--radius-panel);
  }
  :global(html[data-style="floating"]) .ear {
    display: none;
  }
  :global(html[data-style="floating"]) .pill,
  :global(html[data-style="floating"]) .tabs {
    cursor: grab;
  }
  :global(html[data-style="floating"]) .tabs :global(button) {
    cursor: pointer;
  }

  /* Living in the tray: there is no pill. The window is hidden while Loft
     is closed; this covers the moments either side of that. */
  :global(html[data-tray="on"]) .notch:not(.expanded) {
    opacity: 0;
    pointer-events: none;
  }

  /* Clips the pill/panel while the notch animates between its two sizes. */
  .clip {
    position: absolute;
    inset: 0;
    overflow: hidden;
    border-radius: inherit;
  }

  /* Pill (collapsed affordance) */
  .pill {
    position: absolute;
    top: 0;
    left: 50%;
    transform: translateX(-50%);
    display: flex;
    align-items: center;
    gap: 9px;
    width: 236px;
    height: 36px;
    padding: 0 15px 1px;
    white-space: nowrap;
    transition: opacity 0.2s ease 0.12s;
  }
  .notch.expanded .pill {
    opacity: 0;
    pointer-events: none;
    transition: opacity 0.1s ease;
  }
  .brand {
    font-weight: 650;
    font-size: 13px;
    letter-spacing: 0.2px;
  }
  .hint {
    color: var(--text-faint);
    font-size: 12px;
    margin-left: auto;
  }

  .dot {
    width: 7px;
    height: 7px;
    border-radius: 50%;
    flex: none;
    box-shadow: 0 0 0 0 currentColor;
  }
  .dot.good {
    background: var(--good);
    color: var(--good);
    animation: pulse 2.6s infinite;
  }
  .dot.loading {
    background: var(--warn);
    color: var(--warn);
    animation: pulse 1.1s infinite;
  }
  .dot.bad {
    background: var(--bad);
    color: var(--bad);
  }
  .dot.idle {
    background: var(--text-faint);
    color: var(--text-faint);
  }
  @keyframes pulse {
    0% { box-shadow: 0 0 0 0 color-mix(in srgb, currentColor 55%, transparent); }
    70% { box-shadow: 0 0 0 6px transparent; }
    100% { box-shadow: 0 0 0 0 transparent; }
  }

  /* Panel (expanded) */
  .panel {
    position: absolute;
    top: 0;
    left: 50%;
    transform: translateX(-50%);
    display: flex;
    flex-direction: column;
    width: var(--panel-w, 460px);
    height: var(--panel-h, 520px);
    opacity: 0;
    pointer-events: none;
    transition: opacity 0.12s ease;
  }
  .notch.expanded .panel {
    opacity: 1;
    pointer-events: auto;
    transition: opacity 0.28s ease 0.14s;
  }

  /* Shown over the whole panel while files are dragged onto the window. */
  .dropzone {
    position: absolute;
    inset: 8px;
    z-index: 5;
    display: flex;
    flex-direction: column;
    align-items: center;
    justify-content: center;
    gap: 8px;
    text-align: center;
    padding: 24px;
    border-radius: var(--radius-panel, 18px);
    border: 2px dashed var(--accent, #6aa3ff);
    background: color-mix(in srgb, var(--notch, #111) 86%, var(--accent, #6aa3ff));
    color: var(--text, #fff);
    pointer-events: none;
  }
  .drop-title {
    margin: 0;
    font-size: 15px;
    font-weight: 650;
  }
  .drop-sub {
    margin: 0;
    max-width: 280px;
    font-size: 12px;
    line-height: 1.4;
    color: var(--text-dim, #aaa);
  }

  .tabs {
    display: flex;
    align-items: center;
    gap: 4px;
    padding: 12px 12px 10px 14px;
  }
  .segmented {
    display: flex;
    gap: 2px;
    padding: 3px;
    background: var(--field-bg);
    border-radius: 11px;
  }
  .segmented button {
    font-size: 12.5px;
    font-weight: 550;
    color: var(--text-dim);
    padding: 5px 13px;
    border-radius: 8px;
    transition:
      background 0.15s ease,
      color 0.15s ease;
  }
  .segmented button {
    position: relative;
  }
  .segmented button:hover {
    color: var(--text);
  }
  /* Marks the Ask tab while an answer is being written, so it shows from any tab. */
  .answering {
    position: absolute;
    top: 4px;
    right: 4px;
    width: 6px;
    height: 6px;
    border-radius: 50%;
    background: var(--accent);
    color: var(--accent);
    animation: pulse 1.1s infinite;
  }
  .segmented button.active {
    color: var(--text);
    background: var(--raised);
    box-shadow: 0 1px 2px rgba(0, 0, 0, 0.3);
  }
  .tabs .spacer {
    flex: 1;
  }
  .icon-btn.on {
    background: var(--field-bg-hover);
    color: var(--text);
  }

  .body {
    flex: 1;
    padding: 4px 14px 12px 16px;
    min-height: 0;
  }
  .tab-content {
    display: none;
  }
  .tab-content.shown {
    display: block;
    animation: tab-in 0.22s var(--ease);
  }
  /* The Ask tab fills the panel and scrolls its own conversation. */
  .tab-content.fill.shown {
    height: 100%;
  }
  @keyframes tab-in {
    from { opacity: 0; transform: translateY(4px); }
  }

  .statusbar {
    display: flex;
    align-items: center;
    gap: 8px;
    height: 34px;
    padding: 0 16px 2px;
    border-top: 1px solid var(--notch-border);
    font-size: 11.5px;
    color: var(--text-faint);
  }
  .status-text {
    flex: 1;
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
  }
  .accel {
    font-size: 9.5px;
    font-weight: 600;
    text-transform: uppercase;
    letter-spacing: 0.7px;
    color: var(--text-faint);
    border: 1px solid var(--notch-border);
    border-radius: 6px;
    padding: 2px 6px;
  }
</style>
