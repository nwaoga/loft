# Running Loft

What to do once it is open. Build it from [Building](building.md). The first launch on macOS or Windows is [First open](first-open.md).

Start a desktop build from [Building](building.md). To look at the notch UI in a browser,
with mock data and no Rust:

```bash
npm install
npm run dev      # http://localhost:1420
```

In the app: **Settings** (the gear) → download and select a chat model and an embedding
model → **Folders** tab → add a folder → **Ask**. Ctrl/Cmd+Shift+Space summons the notch.

Settings also holds:

- **Theme**: Midnight, Graphite, Ember or Light (remembered in the webview's local storage).
- **Window style**: *Notch* (docked at a screen edge) or *Floating* (a window of its own:
  drag it by its pill or by the top of its panel, and it stays where you leave it, on any
  display). Remembered in `selection.json`, next to the model selection.
- **Placement** (notch only): *Top* is the centre of the top edge; *Right* is the middle of the
  right edge, where the pill stands upright and the panel opens leftwards.
- **Display** (notch only): which monitor the notch is on (the main display by default). If
  the chosen display is unplugged, Loft falls back to the main one.
- **Live in the tray**: nothing stays on screen. Loft is opened and closed from its tray icon
  (the notification area on Windows, the menu bar on macOS, an app indicator on Linux) or
  with Ctrl/Cmd+Shift+Space, and works with either style. On macOS it also drops the Dock
  icon, from the next launch. Where the desktop gives Loft no tray icon the option is
  disabled, since Loft would be unreachable.
- **Performance & battery**: which engine and device run the models when plugged in and on
  battery, trying another engine (or the NPU) on the spot, and a performance test. See
  [Engines, performance and battery](engines.md).
- **Launch at login** and **Quit** (Quit is on the tray icon too).

## Icons

Every icon (app, installer, taskbar, tray) comes from `app-icon.svg`, a notch over the lines
of a file. They are checked in under `src-tauri/icons/`; after changing the design run
`npm run icons` to make them again (it leaves out Tauri's Android and iOS sets). Keep the
shapes bold: the same picture is used at 16 pixels in the tray.
