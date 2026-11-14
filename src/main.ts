import { mount } from "svelte";
import "./app.css";
import App from "./App.svelte";
import { initTheme } from "./lib/theme.svelte";
import { initPlacement } from "./lib/placement.svelte";
import { initWindowStyle } from "./lib/window-style.svelte";

// All three are applied before the first paint, so Loft never flashes the
// default theme, appears at the wrong edge of its (transparent) window, or
// shows a notch where a floating window was chosen.
// Midnight is a step lighter on macOS (see app.css). Windows and Linux keep
// the shared colours. The attribute is set before the notch is mounted.
if (/mac/i.test(`${navigator.platform} ${navigator.userAgent}`)) {
  document.documentElement.dataset.platform = "macos";
}
initTheme();
Promise.allSettled([initPlacement(), initWindowStyle()]).then(() => {
  mount(App, { target: document.getElementById("app")! });
});
