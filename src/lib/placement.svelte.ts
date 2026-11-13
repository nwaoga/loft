// Which screen edge the notch hangs from. The backend owns the setting (it
// has to place the window before the UI exists) and saves it; the UI mirrors
// it as `data-placement` on <html>, which the layout in App.svelte keys off.
//
// A floating window (see window-style.svelte.ts) hangs from no edge: it is
// laid out like the top placement whatever edge is saved for the notch.

import { getPlacement, setPlacement } from "./api";
import { ui } from "./state.svelte";

export type Placement = "top" | "right";

export const PLACEMENTS: { id: Placement; name: string; hint: string }[] = [
  { id: "top", name: "Top", hint: "Centre of the top edge" },
  { id: "right", name: "Right", hint: "Middle of the right edge" },
];

export const placement = $state({ value: "top" as Placement });

let floating = false;

function apply(value: Placement) {
  placement.value = value;
  document.documentElement.dataset.placement = floating ? "top" : value;
}

/** The window style changed: lay out for the saved edge, or for no edge. */
export function syncPlacement(isFloating: boolean) {
  floating = isFloating;
  apply(placement.value);
}

/** Read the saved placement. Called once, before the UI is mounted. */
export async function initPlacement() {
  try {
    apply(await getPlacement());
  } catch {
    apply("top");
  }
}

/** Move the notch to another edge and remember it. */
export async function choosePlacement(value: Placement) {
  if (value === placement.value) return;
  await setPlacement(value, ui.expanded); // moves the window first
  apply(value);
}
