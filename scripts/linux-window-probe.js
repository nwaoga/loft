// Headless check for the Linux runner: no models, no answers.
// Drives the same window commands as scripts/smoke-ui.js (expand, larger
// window, right edge) and checks the native window actually changed size.
// Also checks the tray icon was created and the CPU engine is listed.
//
// Run via scripts/smoke.sh linux-window-probe.js. A pass ends with
// "[dev] WINDOW-OK". A failure ends with "[dev] FAILED: ...".
(async () => {
  const invoke = window.__TAURI__.core.invoke;
  const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
  const log = (msg) => invoke("dev_log", { msg: String(msg) });
  const fail = (why) => {
    throw new Error(why);
  };

  // dev_log returns [width, height, x, y] in physical pixels. GTK applies a
  // size on a later frame, so poll.
  const waitGeo = async (pred, what) => {
    let geo;
    for (let i = 0; i < 25; i++) {
      geo = await log(`${what} ${i}`);
      if (pred(geo)) return geo;
      await sleep(200);
    }
    fail(`${what}: window stayed ${geo}`);
  };

  try {
    const dpr = window.devicePixelRatio || 1;
    const settings = await invoke("get_window_settings");
    const monitors = await invoke("list_monitors");
    const mon = monitors.find((m) => m.selected) || monitors[0];
    // The webview's devicePixelRatio can disagree with GTK's scale (it stays
    // 1 under GDK_SCALE=2). The monitor size from the backend is physical.
    const scale = mon && screen.width ? mon.width / screen.width : dpr;
    await log(
      `probe start | trayAvailable=${settings.trayAvailable} trayOnly=${settings.trayOnly} dpr=${dpr} scale=${scale} screen=${screen.width}x${screen.height} monitor=${mon ? mon.width + "x" + mon.height : "?"} availTop=${screen.availTop}`,
    );
    if (!settings.trayAvailable) fail("the tray icon was not created (trayAvailable is false)");

    const px = (logical) => Math.round(logical * scale);
    const near = (geo, lw, lh) => Math.abs(geo[0] - px(lw)) <= 12 && Math.abs(geo[1] - px(lh)) <= 12;

    const collapsed = await waitGeo((g) => near(g, 320, 76), "probe collapsed");
    await invoke("set_expanded", { expanded: true });
    const expanded = await waitGeo((g) => near(g, 540, 580), "probe expanded");
    if (expanded[0] < collapsed[0] + 100 || expanded[1] < collapsed[1] + 100) {
      fail(`expanded window did not grow: ${collapsed} -> ${expanded}`);
    }
    await log("probe expanded OK");
    await sleep(1500);

    await invoke("set_large", { large: true, expanded: true });
    const large = await waitGeo(
      (g) => g[0] >= expanded[0] + 20 && g[1] >= expanded[1] + 20,
      "probe large",
    );
    await log(`probe large OK | ${expanded[0]}x${expanded[1]} -> ${large[0]}x${large[1]}`);
    await invoke("set_large", { large: false, expanded: true });
    await waitGeo((g) => near(g, 540, 580), "probe large off");

    await invoke("set_placement", { placement: "right", expanded: true });
    const right = await waitGeo((g) => near(g, 520, 600), "probe right");
    const screenW = mon ? mon.width : px(screen.width);
    const screenH = mon ? mon.height : px(screen.height);
    const rightEdge = right[2] + right[0];
    const vmid = right[3] + right[1] / 2;
    if (Math.abs(rightEdge - screenW) > 16 || Math.abs(vmid - screenH / 2) > 40) {
      fail(`right edge: window ${right} is not centred on the right edge of ${screenW}x${screenH}`);
    }
    await log("probe right OK");

    await invoke("set_placement", { placement: "top", expanded: false });
    await waitGeo((g) => near(g, 320, 76), "probe collapsed again");

    // The shell presses Ctrl+Shift+Space while this line is in the log.
    await log("HOTKEY-WAIT");
    await sleep(2000);
    const afterKey = await log("probe after hotkey");
    const grew = afterKey[0] > collapsed[0] + 100 && afterKey[1] > collapsed[1] + 100;
    await log(`probe hotkey ${grew ? "toggled the panel" : "did not toggle the panel"} | ${afterKey}`);

    const inf = await invoke("get_inference");
    const options = (inf.options || []).map((o) => `${o.label}${o.available ? "" : " [unavailable]"}`);
    await log(`probe engines | ${options.join("; ")}`);
    const cpu = (inf.options || []).find((o) => o.available && o.target && o.target.backend === "cpu");
    if (!cpu) fail(`CPU is not an available engine: ${options.join("; ")}`);

    const procs = await invoke("dev_engines");
    await log(`probe dev_engines | ${JSON.stringify(procs)}`);

    await log("WINDOW-OK");
    await invoke("quit_app");
  } catch (e) {
    try {
      await log("FAILED: " + (e && e.message ? e.message : e));
    } catch {
      /* the app may already be gone */
    }
    try {
      await invoke("quit_app");
    } catch {
      /* ignore */
    }
  }
})();
