// Global UI state using Svelte 5 runes (valid in a .svelte.ts module).

import { setExpanded } from "./api";

/** "settings" is opened from the gear button, not the tab strip. */
export type Tab = "ask" | "folders" | "settings";

export const ui = $state({
  expanded: false,
  tab: "ask" as Tab,
  /** An answer is being written; shown on the Ask tab so it is visible from the other tabs. */
  asking: false,
});

// The native window is only as large as what is drawn, so it grows before the
// panel opens and shrinks once the collapse animation has finished.
const COLLAPSE_ANIMATION_MS = 520;
let shrinkTimer: ReturnType<typeof setTimeout> | undefined;

export async function expand(tab?: Tab) {
  if (tab) ui.tab = tab;
  clearTimeout(shrinkTimer);
  try {
    await setExpanded(true);
  } catch {
    /* keep going: the panel is still usable in a too-small window's place */
  }
  ui.expanded = true;
}

export function collapse() {
  if (!ui.expanded) return;
  ui.expanded = false;
  clearTimeout(shrinkTimer);
  shrinkTimer = setTimeout(() => {
    if (!ui.expanded) setExpanded(false).catch(() => {});
  }, COLLAPSE_ANIMATION_MS);
}

export function toggle() {
  if (ui.expanded) collapse();
  else expand();
}
