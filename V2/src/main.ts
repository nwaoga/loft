import { mount } from "svelte";
import "./app.css";
import App from "./App.svelte";
import { initTheme } from "./lib/theme.svelte";
import { initPlacement } from "./lib/placement.svelte";

// Both are applied before the first paint, so the notch never flashes the
// default theme or appears at the wrong edge of its (transparent) window.
initTheme();
initPlacement().finally(() => {
  mount(App, { target: document.getElementById("app")! });
});
