<script lang="ts">
  import { onMount, tick } from "svelte";
  import { deleteConversation, listConversations, renameConversation } from "../api";
  import type { Conversation } from "../types";
  import Icon from "./Icon.svelte";

  // The saved conversations, laid over the conversation area (the panel does
  // not change size). Open one to carry on with it; rename or delete from
  // its menu. Deleting a conversation never touches a file or its index.
  let {
    activeId = null,
    busy = false,
    saving = true,
    onopen,
    onnew,
    onclose,
    onrenamed,
    ondeleted,
  }: {
    /** The conversation that is open now. */
    activeId?: string | null;
    /** An answer is being written: switching waits until it has ended. */
    busy?: boolean;
    /** New conversations are being saved (the "Chat history" option). */
    saving?: boolean;
    onopen: (id: string) => void;
    onnew: () => void;
    onclose: () => void;
    onrenamed: (conversation: Conversation) => void;
    ondeleted: (id: string) => void;
  } = $props();

  let query = $state("");
  let list = $state<Conversation[]>([]);
  let total = $state(0);
  let loaded = $state(false);
  let error = $state("");
  /** The row whose menu, rename box or delete question is showing. */
  let menuFor = $state<string | null>(null);
  let renaming = $state<string | null>(null);
  let renameText = $state("");
  let confirming = $state<string | null>(null);
  let panelEl: HTMLDivElement | undefined = $state();
  let searchEl: HTMLInputElement | undefined = $state();

  let latest = 0;
  async function load() {
    const mine = ++latest;
    try {
      const found = await listConversations(query);
      if (mine !== latest) return; // a newer search has been started
      list = found.conversations;
      total = found.total;
      error = "";
    } catch (e) {
      if (mine === latest) error = String(e);
    } finally {
      if (mine === latest) loaded = true;
    }
  }

  onMount(() => {
    load().then(async () => {
      await tick();
      // Start on the open conversation, or on the search box.
      (rowButtons().find((b) => b.dataset.id === activeId) ?? searchEl)?.focus();
    });
  });

  // Search as the user types, without asking the database on every key.
  let searchTimer: ReturnType<typeof setTimeout> | undefined;
  function onSearch() {
    clearTimeout(searchTimer);
    searchTimer = setTimeout(load, 180);
  }

  /** "Today", "Yesterday", "Previous 7 days", "Previous 30 days", then by month. */
  function groupOf(at: number, now: Date): string {
    const startOfToday = new Date(now.getFullYear(), now.getMonth(), now.getDate()).getTime();
    const day = 86_400_000;
    if (at >= startOfToday) return "Today";
    if (at >= startOfToday - day) return "Yesterday";
    if (at >= startOfToday - 7 * day) return "Previous 7 days";
    if (at >= startOfToday - 30 * day) return "Previous 30 days";
    const d = new Date(at);
    return d.toLocaleDateString(undefined, { month: "long", year: d.getFullYear() === now.getFullYear() ? undefined : "numeric" });
  }
  const groups = $derived.by(() => {
    const now = new Date();
    const out: { name: string; items: Conversation[] }[] = [];
    for (const c of list) {
      const name = groupOf(c.updatedAt, now);
      if (out[out.length - 1]?.name !== name) out.push({ name, items: [] });
      out[out.length - 1].items.push(c);
    }
    return out;
  });

  const rowButtons = () => [...(panelEl?.querySelectorAll<HTMLButtonElement>(".conv-open") ?? [])];

  function closeMenus() {
    menuFor = null;
    renaming = null;
    confirming = null;
  }

  // Keys: Up and Down move through the list (from the search box too), Enter
  // opens, Esc backs out of whatever is open and then closes the panel.
  // Caught on the way down so the panel's own Esc handling never sees it.
  $effect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape") {
        e.stopPropagation();
        if (menuFor || renaming || confirming) closeMenus();
        else onclose();
        return;
      }
      if (e.key !== "ArrowDown" && e.key !== "ArrowUp") return;
      if (renaming) return; // the keys belong to the name being typed
      const rows = rowButtons();
      if (!rows.length) return;
      const at = rows.indexOf(document.activeElement as HTMLButtonElement);
      const next = e.key === "ArrowDown" ? Math.min(at + 1, rows.length - 1) : at <= 0 ? -1 : at - 1;
      e.preventDefault();
      if (next < 0) searchEl?.focus();
      else rows[next].focus();
    };
    window.addEventListener("keydown", onKey, true);
    return () => window.removeEventListener("keydown", onKey, true);
  });

  async function startRename(c: Conversation) {
    closeMenus();
    renaming = c.id;
    renameText = c.title;
    await tick();
    const box = panelEl?.querySelector<HTMLInputElement>(".rename-box");
    box?.focus();
    box?.select();
  }

  async function saveRename(c: Conversation) {
    const title = renameText.trim();
    if (!title || title === c.title) {
      renaming = null;
      return;
    }
    try {
      const renamed = await renameConversation(c.id, title);
      list = list.map((x) => (x.id === c.id ? renamed : x));
      onrenamed(renamed);
      error = "";
    } catch (e) {
      error = String(e);
    }
    renaming = null;
  }

  async function remove(c: Conversation) {
    try {
      await deleteConversation(c.id);
      ondeleted(c.id);
      closeMenus();
      await load();
    } catch (e) {
      error = String(e);
    }
  }

  const time = (at: number) => new Date(at).toLocaleTimeString(undefined, { hour: "numeric", minute: "2-digit" });
  const footer = $derived(
    !loaded
      ? ""
      : `${total} ${total === 1 ? "conversation" : "conversations"} · Stored locally${saving ? "" : " · new chats are not being saved"}`,
  );
</script>

<div class="history" role="dialog" aria-label="Conversations" bind:this={panelEl}>
  <header>
    <button class="icon-btn" onclick={onclose} aria-label="Back to the conversation" title="Back (Esc)">
      <Icon name="arrow-left" size={15} />
    </button>
    <h2>Conversations</h2>
    <button class="new" onclick={onnew} disabled={busy} title="Start a fresh conversation">
      <Icon name="plus" size={12} stroke={2.4} /> New
    </button>
  </header>

  <div class="search">
    <span class="search-icon"><Icon name="search" size={13} /></span>
    <input
      bind:this={searchEl}
      bind:value={query}
      oninput={onSearch}
      type="search"
      placeholder="Search conversations…"
      aria-label="Search conversations"
      spellcheck="false"
    />
  </div>

  <div class="list scroll">
    {#if error}<div class="note">{error}</div>{/if}
    {#if busy}<div class="note">An answer is being written. Stop it, or wait for it, to switch conversations.</div>{/if}

    {#if loaded && list.length === 0}
      <p class="empty">
        {#if query.trim()}
          Nothing matches “{query.trim()}”.
        {:else}
          No conversations yet. Ask a question and it is kept here{saving ? "" : " (once Chat history is set to Save locally)"}.
        {/if}
      </p>
    {/if}

    {#each groups as group (group.name)}
      <div class="group-label">{group.name}</div>
      {#each group.items as c (c.id)}
        <div class="conv" class:active={c.id === activeId} class:open={menuFor === c.id || renaming === c.id || confirming === c.id}>
          {#if renaming === c.id}
            <input
              class="rename-box"
              bind:value={renameText}
              aria-label="Conversation name"
              maxlength="120"
              onkeydown={(e) => {
                if (e.key === "Enter") saveRename(c);
              }}
              onblur={() => saveRename(c)}
            />
          {:else}
            <button
              class="conv-open"
              data-id={c.id}
              disabled={busy && c.id !== activeId}
              aria-current={c.id === activeId ? "true" : undefined}
              title={c.title}
              onclick={() => onopen(c.id)}
            >
              <span class="conv-title">{c.title}</span>
              <span class="conv-meta">{c.questions} {c.questions === 1 ? "question" : "questions"} · {time(c.updatedAt)}</span>
            </button>
            <button
              class="conv-more"
              aria-label="Actions for {c.title}"
              aria-expanded={menuFor === c.id}
              title="Rename or delete"
              onclick={() => {
                const open = menuFor !== c.id;
                closeMenus();
                if (open) menuFor = c.id;
              }}
            >⋯</button>
          {/if}
          {#if menuFor === c.id}
            <div class="conv-menu" role="menu">
              <button role="menuitem" onclick={() => startRename(c)}><Icon name="pencil" size={12} stroke={2.2} /> Rename</button>
              <button
                role="menuitem"
                class="danger"
                disabled={busy && c.id === activeId}
                onclick={() => {
                  menuFor = null;
                  confirming = c.id;
                }}
              ><Icon name="trash" size={12} stroke={2.2} /> Delete</button>
            </div>
          {/if}
          {#if confirming === c.id}
            <div class="conv-confirm" role="alertdialog" aria-label="Delete this conversation?">
              <span>Delete this conversation? Your files are not affected.</span>
              <button class="danger" onclick={() => remove(c)}>Delete</button>
              <button onclick={closeMenus}>Cancel</button>
            </div>
          {/if}
        </div>
      {/each}
    {/each}
  </div>

  <footer>{footer}</footer>
</div>

<style>
  /* Covers the conversation area; the panel keeps its size. */
  .history {
    position: absolute;
    inset: -4px -6px -4px -8px;
    z-index: 5;
    display: flex;
    flex-direction: column;
    gap: 8px;
    padding: 4px 6px 4px 8px;
    background: var(--notch);
    animation: history-in 0.16s var(--ease);
  }
  @keyframes history-in {
    from { opacity: 0; transform: translateY(4px); }
  }
  header {
    flex: none;
    display: flex;
    align-items: center;
    gap: 6px;
  }
  h2 {
    flex: 1;
    font-size: 13.5px;
    font-weight: 650;
  }
  .new {
    display: flex;
    align-items: center;
    gap: 4px;
    font-size: 12px;
    font-weight: 600;
    color: var(--text-dim);
    padding: 4px 9px;
    border-radius: 8px;
  }
  .new:hover:not(:disabled) {
    color: var(--text);
    background: var(--field-bg-hover);
  }
  .new:disabled {
    opacity: 0.45;
    cursor: default;
  }

  .search {
    flex: none;
    position: relative;
  }
  .search-icon {
    position: absolute;
    left: 11px;
    top: 50%;
    display: grid;
    color: var(--text-faint);
    transform: translateY(-50%);
    pointer-events: none;
  }
  .search input {
    width: 100%;
    padding: 8px 12px 8px 31px;
    border-radius: 11px;
    border: 1px solid var(--notch-border);
    background: var(--field-bg);
    color: var(--text);
    font: inherit;
    font-size: 12.5px;
  }
  .search input::placeholder {
    color: var(--text-faint);
  }
  .search input:focus-visible {
    outline: 2px solid color-mix(in srgb, var(--accent) 70%, transparent);
    outline-offset: 1px;
  }
  .search input::-webkit-search-cancel-button {
    -webkit-appearance: none;
  }

  .list {
    flex: 1;
    min-height: 0;
    display: flex;
    flex-direction: column;
    gap: 1px;
    margin-right: -6px;
    padding-right: 2px;
    scrollbar-gutter: stable;
  }
  .group-label {
    margin: 9px 0 3px 8px;
    font-size: 10px;
    font-weight: 650;
    letter-spacing: 0.7px;
    text-transform: uppercase;
    color: var(--text-faint);
  }
  .group-label:first-child {
    margin-top: 2px;
  }
  .conv {
    position: relative;
    display: flex;
    flex-wrap: wrap;
    align-items: center;
    border-radius: 10px;
    transition: background 0.12s ease;
  }
  .conv:hover,
  .conv.open,
  .conv:focus-within {
    background: var(--field-bg);
  }
  .conv.active {
    background: color-mix(in srgb, var(--accent) 13%, transparent);
  }
  .conv-open {
    flex: 1;
    min-width: 0;
    display: flex;
    flex-direction: column;
    gap: 1px;
    padding: 6px 4px 6px 9px;
    border-radius: 10px;
    text-align: left;
  }
  .conv-open:disabled {
    opacity: 0.5;
    cursor: default;
  }
  .conv-open:focus-visible {
    outline: 2px solid color-mix(in srgb, var(--accent) 70%, transparent);
    outline-offset: -2px;
  }
  .conv-title {
    font-size: 12.5px;
    font-weight: 550;
    color: var(--text);
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
  }
  .conv-meta {
    font-size: 10.5px;
    color: var(--text-faint);
  }
  .conv-more {
    flex: none;
    width: 26px;
    height: 26px;
    margin-right: 4px;
    border-radius: 7px;
    font-size: 15px;
    line-height: 1;
    color: var(--text-faint);
    opacity: 0;
  }
  .conv:hover .conv-more,
  .conv.open .conv-more,
  .conv-more:focus-visible {
    opacity: 1;
  }
  .conv-more:hover {
    color: var(--text);
    background: var(--field-bg-hover);
  }
  .conv-menu,
  .conv-confirm {
    flex-basis: 100%;
    display: flex;
    align-items: center;
    gap: 6px;
    padding: 0 8px 7px 9px;
    font-size: 11.5px;
    color: var(--text-dim);
  }
  .conv-confirm span {
    flex: 1;
    min-width: 0;
  }
  .conv-menu button,
  .conv-confirm button {
    display: inline-flex;
    align-items: center;
    gap: 5px;
    padding: 4px 9px;
    border-radius: 7px;
    font-size: 11.5px;
    font-weight: 600;
    color: var(--text);
    background: var(--field-bg-hover);
  }
  .conv-menu button:hover:not(:disabled),
  .conv-confirm button:hover {
    background: var(--raised-hover);
  }
  .conv-menu button:disabled {
    opacity: 0.45;
    cursor: default;
  }
  button.danger {
    color: var(--bad);
  }
  .rename-box {
    flex: 1;
    min-width: 0;
    margin: 4px 6px 4px 5px;
    padding: 5px 8px;
    border-radius: 8px;
    border: 1px solid color-mix(in srgb, var(--accent) 60%, transparent);
    background: var(--field-bg);
    color: var(--text);
    font: inherit;
    font-size: 12.5px;
  }
  .rename-box:focus {
    outline: none;
  }
  .empty,
  .note {
    padding: 10px 9px;
    font-size: 12px;
    line-height: 1.45;
    color: var(--text-faint);
  }
  .note {
    padding: 4px 9px;
    color: var(--warn);
  }
  footer {
    flex: none;
    padding: 6px 3px 0;
    border-top: 1px solid var(--notch-border);
    font-size: 11px;
    color: var(--text-faint);
  }
</style>
