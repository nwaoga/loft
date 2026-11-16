// Promo captures for the macOS runner. No test banner and no debug copy:
// the window should look like Loft in ordinary use, on the sample folder.
//
// The saved placement is left alone (top, centred). Just before each shot
// the window is moved to the top-right so the crop can show the notch under
// the menu bar, near the tray icons. That move is not saved.
//
// Signals the shell with "SHOT <name>" (dev_log also prints the window
// geometry) and ends with "PROMO-OK". Needs the chat and embedding models
// selected, and sample-docs seeded by scripts/smoke.sh.
(async () => {
  const invoke = window.__TAURI__.core.invoke;
  const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
  const log = (msg) => invoke("dev_log", { msg: String(msg) });
  const $ = (s, root = document) => root.querySelector(s);
  const text = (el) => (el ? el.innerText.replace(/\s+/g, " ").trim() : "");
  const fail = (why) => {
    throw new Error(why);
  };
  const waitFor = async (fn, ms, what) => {
    const t = Date.now();
    let last = "";
    while (Date.now() - t < ms) {
      try {
        const v = await fn();
        if (v) return v;
      } catch (e) {
        const msg = e && e.message ? e.message : String(e);
        if (msg.startsWith("a model failed")) throw e;
        last = msg;
      }
      await sleep(400);
    }
    fail(`timed out waiting for ${what}${last ? ` (${last})` : ""}`);
  };

  try {
    const dpr = window.devicePixelRatio || 1;
    await log(`promo display dpr=${dpr} screen=${screen.width}x${screen.height} availTop=${screen.availTop}`);

    const status = await waitFor(async () => {
      const s = await invoke("get_runtime_status");
      if (s.chat === "error" || s.embed === "error") {
        const inf = await invoke("get_inference");
        const chatLog = await invoke("get_engine_log", { engine: "chat" });
        const embedLog = await invoke("get_engine_log", { engine: "embed" });
        const why = [inf.status && inf.status.chat && inf.status.chat.error, inf.status && inf.status.embed && inf.status.embed.error]
          .filter(Boolean)
          .join(" | ");
        await log(`promo model error | ${why || s.message || ""}`);
        await log(`promo chat log | ${(chatLog || []).slice(-6).join(" || ")}`);
        await log(`promo embed log | ${(embedLog || []).slice(-6).join(" || ")}`);
        fail(`a model failed to start: ${why || s.chat + "/" + s.embed}`);
      }
      return s.chat === "ready" && s.embed === "ready" ? s : null;
    }, 240000, "the chat and embedding models");
    await log(`promo models ready | ${status.accelerator} | ${status.message || ""}`);

    const folder = await waitFor(async () => {
      const folders = await invoke("list_folders");
      const sample = folders.find((f) => String(f.path).endsWith("sample-docs"));
      if (sample && sample.status === "error") fail(`indexing failed: ${sample.error || sample.status}`);
      return sample && sample.status === "ready" && sample.chunkCount > 0 ? sample : null;
    }, 240000, "sample-docs to finish indexing");
    await log(`promo indexed | ${folder.fileCount} files, ${folder.chunkCount} chunks`);

    // Slide the window under the right-hand end of the menu bar. Docking
    // (opening the panel) puts it back in the centre, so this is called
    // again after every resize, and only then is SHOT logged.
    const placeTopRight = async () => {
      const api = window.__TAURI__.window;
      if (!api || !api.getCurrentWindow) fail("window.__TAURI__.window is missing");
      const win = api.getCurrentWindow();
      const factor = await win.scaleFactor();
      const size = await win.outerSize();
      const logicalW = size.width / factor;
      const x = Math.max(0, screen.width - logicalW - 10);
      const y = screen.availTop || 0;
      await win.setPosition({ type: "Logical", x, y });
      const wantX = Math.round(x * factor);
      const wantY = Math.round(y * factor);
      const geo = await waitFor(async () => {
        const g = await log("promo park");
        return Math.abs(g[2] - wantX) <= 16 && Math.abs(g[3] - wantY) <= 16 ? g : null;
      }, 5000, "the window to sit at the top right");
      await log(`promo parked | target ${Math.round(x)},${Math.round(y)} | at ${geo[2]},${geo[3]}`);
    };
    const shot = async (name) => {
      await placeTopRight();
      await log("SHOT " + name);
      await sleep(2500);
    };

    // Collapsed notch, then the open panel with nothing asked yet.
    await shot("notch");
    $(".pill").click();
    await waitFor(() => document.querySelector(".notch.expanded"), 5000, "the panel to open");
    await sleep(600);
    await shot("idle");

    const ask = async (q) => {
      const ta = $("textarea");
      ta.value = q;
      ta.dispatchEvent(new Event("input", { bubbles: true }));
      await sleep(150);
      $(".send").click();
      await waitFor(() => !$(".send.stop") && $(".turn.last .answer p:not(.waiting)"), 180000, "the answer to finish");
      await sleep(500);
    };
    await ask("Who owns the supplier contract with BeanWorks?");
    const answer = text($(".turn.last .answer"));
    const sources = $(".turn.last .sources-toggle");
    await log(`promo answer | ${answer.slice(0, 240)}`);
    if (answer.length < 20 || /couldn't find that/i.test(answer)) fail(`no usable answer: ${answer.slice(0, 240)}`);
    if (!sources) fail("the answer has no sources");

    // The reply with the cited excerpts open, then Settings.
    if (sources && !$(".turn.last .sources.open")) sources.click();
    await waitFor(() => $(".turn.last .sources.open"), 5000, "the sources to open");
    await sleep(400);
    await shot("sources");
    $('[aria-label="Settings"]').click();
    await waitFor(() => document.querySelector(".tab-content.shown .settings"), 5000, "Settings to open");
    await sleep(500);
    await shot("settings");

    await log("PROMO-OK");
    await invoke("quit_app");
  } catch (e) {
    try {
      await log("FAILED: " + (e && e.message ? e.message : e));
    } catch {
      /* already gone */
    }
    try {
      await invoke("quit_app");
    } catch {
      /* ignore */
    }
  }
})();
