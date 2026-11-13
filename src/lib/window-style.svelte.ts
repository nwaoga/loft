// How Loft appears: a notch docked at a screen edge, or a floating window
// that stays where it is dragged; and whether it lives in the tray (nothing
// on screen until it is opened from the tray icon or with the hotkey).
//
// The backend owns both settings (it places and shows the window before the
// UI exists). The UI mirrors them as `data-style` and `data-tray` on <html>,
// which the layout in App.svelte keys off.

import { getWindowSettings, onPanelSize, rememberPosition, setTrayOnly, setWindowStyle } from "./api";
import { syncPlacement } from "./placement.svelte";
import { applyPanelSize, ui } from "./state.svelte";

export type WindowStyle = "notch" | "floating";

export const STYLES: { id: WindowStyle; name: string; hint: string }[] = [
  { id: "notch", name: "Notch", hint: "Docked at a screen edge" },
  { id: "floating", name: "Floating", hint: "A window you can put anywhere" },
];

export const windowStyle = $state({
  value: "notch" as WindowStyle,
  /** No pill on screen: Loft is opened from its tray icon or the hotkey. */
  trayOnly: false,
  /** This desktop gave Loft a tray (menu bar) icon. */
  trayAvailable: true,
});

function apply() {
  document.documentElement.dataset.style = windowStyle.value;
  document.documentElement.dataset.tray = windowStyle.trayOnly ? "on" : "off";
  syncPlacement(windowStyle.value === "floating");
}

/** Read the saved style. Called once, before the UI is mounted. */
export async function initWindowStyle() {
  try {
    const saved = await getWindowSettings();
    windowStyle.value = saved.style;
    windowStyle.trayOnly = saved.trayOnly;
    windowStyle.trayAvailable = saved.trayAvailable;
    ui.large = saved.large;
  } catch {
    /* keep the defaults */
  }
  // The backend says how large the panel is whenever it places the window.
  onPanelSize((size) => {
    if (ui.expanded) applyPanelSize(size);
  }).catch(() => {});
  apply();
}

/** Switch between the notch and the floating window, and remember it. */
export async function chooseStyle(value: WindowStyle) {
  if (value === windowStyle.value) return;
  await setWindowStyle(value, ui.expanded); // moves the window first
  windowStyle.value = value;
  apply();
}

/** Live in the tray, or stay on screen. */
export async function chooseTrayOnly(on: boolean) {
  windowStyle.trayOnly = await setTrayOnly(on, ui.expanded);
  apply();
}

/** The floating window was dragged: remember where it was left. */
export function windowMoved() {
  if (windowStyle.value === "floating") rememberPosition().catch(() => {});
}
