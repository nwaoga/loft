// Global UI state using Svelte 5 runes (valid in a .svelte.ts module).

import { setExpanded, setLarge, type PanelSize } from "./api";

/** "settings" and "usage" are opened from header buttons, not the tab strip. */
export type Tab = "ask" | "folders" | "settings" | "usage";

export const ui = $state({
  expanded: false,
  tab: "ask" as Tab,
  /** An answer is being written; shown on the Ask tab so it is visible from the other tabs. */
  asking: false,
  /** The panel opens twice as large (remembered by the backend). */
  large: false,
});

/** Size the panel; the layout reads these. */
export function applyPanelSize(size: PanelSize) {
  const root = document.documentElement.style;
  root.setProperty("--panel-w", `${size.width}px`);
  root.setProperty("--panel-h", `${size.height}px`);
}

// The native window is only as large as what is drawn, so it grows before the
// panel opens and shrinks once the collapse animation has finished.
const COLLAPSE_ANIMATION_MS = 520;
let shrinkTimer: ReturnType<typeof setTimeout> | undefined;

export async function expand(tab?: Tab) {
  if (tab) ui.tab = tab;
  clearTimeout(shrinkTimer);
  try {
    applyPanelSize(await setExpanded(true));
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

/** Twice as large, or back to normal. The window grows before the panel
 * does, and shrinks only once the panel has finished shrinking. */
let resizing = 0;
export async function toggleLarge() {
  const large = !ui.large;
  const mine = ++resizing;
  ui.large = large;
  if (!ui.expanded) {
    await setLarge(large, false).catch(() => {});
    return;
  }
  if (large) {
    try {
      applyPanelSize(await setLarge(true, true));
    } catch {
      ui.large = false;
    }
    return;
  }
  applyPanelSize({ width: 460, height: 520 });
  await new Promise((done) => setTimeout(done, COLLAPSE_ANIMATION_MS));
  if (mine !== resizing) return; // switched again meanwhile
  try {
    applyPanelSize(await setLarge(false, ui.expanded));
  } catch {
    /* the panel is already at its normal size */
  }
}

export function toggle() {
  if (ui.expanded) collapse();
  else expand();
}
