// Colour themes. Each theme is a set of CSS variables under
// `[data-theme="<id>"]` in app.css; choosing one sets that attribute on <html>.
// The choice is remembered in the webview's local storage.

export type ThemeId = "midnight" | "graphite" | "ember" | "light";

export const THEMES: { id: ThemeId; name: string }[] = [
  { id: "midnight", name: "Midnight" },
  { id: "graphite", name: "Graphite" },
  { id: "ember", name: "Ember" },
  { id: "light", name: "Light" },
];

const KEY = "loft.theme";
const DEFAULT: ThemeId = "midnight";

function stored(): ThemeId {
  try {
    const saved = localStorage.getItem(KEY);
    if (THEMES.some((t) => t.id === saved)) return saved as ThemeId;
  } catch {
    /* storage unavailable: use the default */
  }
  return DEFAULT;
}

export const theme = $state({ id: stored() });

/** Apply the remembered theme. Called once, before the UI is mounted. */
export function initTheme() {
  document.documentElement.dataset.theme = theme.id;
}

export function setTheme(id: ThemeId) {
  theme.id = id;
  document.documentElement.dataset.theme = id;
  try {
    localStorage.setItem(KEY, id);
  } catch {
    /* not remembered, but still applied for this run */
  }
}
