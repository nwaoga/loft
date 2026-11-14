// Promo captures for the Linux runner. No test banner and no debug copy:
// the window should look like Loft in ordinary use, on the sample folder.
//
// Signals the shell with "SHOT <name>" (dev_log also prints the window
// geometry, which the shell crops to) and ends with "PROMO-OK". Needs the
// chat and embedding models selected, and sample-docs seeded by smoke.sh.
(async () => {
  const invoke = window.__TAURI__.core.invoke;
  const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
  const log = (msg) => invoke("dev_log", { msg: String(msg) });
  const $ = (s, root = document) => root.querySelector(s);
  const $$ = (s, root = document) => [...root.querySelectorAll(s)];
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
      await sleep(200);
    }
    fail(`timed out waiting for ${what}${last ? ` (${last})` : ""}`);
  };
  const shot = async (name) => {
    await log(`SHOT ${name}`);
    await sleep(1200);
  };

  try {
    const dpr = window.devicePixelRatio || 1;
    await log(`promo display dpr=${dpr} screen=${screen.width}x${screen.height} availTop=${screen.availTop}`);

    const status = await waitFor(async () => {
      const s = await invoke("get_runtime_status");
      if (s.chat === "error" || s.embed === "error") {
        const inf = await invoke("get_inference");
        const why = [inf.status && inf.status.chat && inf.status.chat.error, inf.status && inf.status.embed && inf.status.embed.error]
          .filter(Boolean)
          .join(" | ");
        await log(`promo model error | ${why || s.message || ""}`);
        fail(`a model failed to start: ${why || s.chat + "/" + s.embed}`);
      }
      return s.chat === "ready" && s.embed === "ready" ? s : null;
    }, 240000, "the chat and embedding models");
    await log(`promo models ready | ${status.accelerator} | ${status.message || ""}`);

    // Open on the sample folder while a rebuild is in progress, before any
    // question is asked. sample-docs is small, so this has to be the first
    // thing we paint once the embedding model is up.
    $(".pill").click();
    await sleep(500);
    if (screen.height >= 900) {
      $('[aria-label="Larger window"]').click();
      await sleep(400);
    }
    $$("header.tabs button").find((b) => text(b) === "Folders").click();
    await sleep(300);
    const rebuild = $('[aria-label="Rebuild index"]');
    if (!rebuild) fail("no folder to rebuild");
    rebuild.click();
    await waitFor(async () => {
      const folders = await invoke("list_folders");
      const sample = folders.find((f) => String(f.path).endsWith("sample-docs"));
      return sample && (sample.status === "indexing" || sample.status === "pending") ? sample : null;
    }, 20000, "sample-docs to start indexing");
    await shot("folders");

    const folder = await waitFor(async () => {
      const folders = await invoke("list_folders");
      const sample = folders.find((f) => String(f.path).endsWith("sample-docs"));
      if (sample && sample.status === "error") fail(`indexing failed: ${sample.error || sample.status}`);
      return sample && sample.status === "ready" && sample.chunkCount > 0 ? sample : null;
    }, 240000, "sample-docs to finish indexing");
    await log(`promo indexed | ${folder.fileCount} files, ${folder.chunkCount} chunks`);

    $$("header.tabs button").find((b) => text(b) === "Ask").click();
    await sleep(200);
    $('[aria-label="Collapse"]').click();
    // The window shrinks only after the collapse animation (about half a second).
    await waitFor(async () => {
      const g = await log("promo collapsing");
      return g[0] <= 400 && g[1] <= 120 ? g : null;
    }, 5000, "the notch to shrink");
    await shot("notch");

    $(".pill").click();
    await sleep(600);
    await shot("expanded");

    const ask = async (q) => {
      const ta = $("textarea");
      ta.value = q;
      ta.dispatchEvent(new Event("input", { bubbles: true }));
      await sleep(150);
      $(".send").click();
      await waitFor(() => !$(".send.stop") && $(".turn.last .answer p:not(.waiting)"), 180000, "the answer to finish");
      await sleep(400);
    };
    await ask("Who owns the supplier contract with BeanWorks?");
    const answer = text($(".turn.last .answer"));
    const sources = $(".turn.last .sources-toggle");
    await log(`promo answer | ${answer.slice(0, 240)}`);
    if (answer.length < 20 || /couldn't find that/i.test(answer)) fail(`no usable answer: ${answer.slice(0, 240)}`);
    if (!sources) fail("the answer has no sources");
    if (!$(".turn.last .sources.open")) sources.click();
    await sleep(450);
    await shot("answer");

    $('[aria-label="Settings"]').click();
    await sleep(500);
    await shot("settings");

    // Leave the panel up. The shell crops the indicator out of the panel.
    await shot("tray");
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
