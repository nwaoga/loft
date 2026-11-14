// Promo captures for the macOS runner. No test banner and no debug copy:
// the window should look like Loft in ordinary use, on the sample folder.
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
        last = e && e.message ? e.message : String(e);
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
      if (s.chat === "error" || s.embed === "error") fail(`a model failed to start: ${s.message || s.chat + "/" + s.embed}`);
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

    // Collapsed notch, then the open panel with nothing asked yet.
    await log("SHOT notch");
    await sleep(2500);
    $(".pill").click();
    await sleep(700);
    if (screen.height >= 900) {
      $('[aria-label="Larger window"]').click();
      await sleep(800);
    }
    await log("SHOT idle");
    await sleep(2500);

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

    // The reply on its own, then the same reply with the cited excerpts open.
    if ($(".turn.last .sources.open")) sources.click();
    await sleep(400);
    await log("SHOT answer");
    await sleep(2500);
    if (!$(".turn.last .sources.open")) $(".turn.last .sources-toggle").click();
    await sleep(500);
    await log("SHOT citations");
    await sleep(2500);

    // Leave the panel up so the shell can grab the menu bar and the tray menu.
    await log("SHOT hold");
    await sleep(8000);
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
