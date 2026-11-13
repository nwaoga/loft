// Run after smoke-ui.js, on the profile it left behind:
//
//   set LOFT_SMOKE_KEEP=1
//   scripts\win-smoke.bat smoke-restart.js
//
// Loft has been closed and started again in between. This checks that the
// chat history is still there, as it was saved: the conversation smoke-ui.js
// named "Kept for the restart check" with its messages in order and their
// sources, an answer that was stopped still marked as stopped, and that the
// conversation can be opened and continued.
(async () => {
  const invoke = window.__TAURI__.core.invoke;
  const log = (msg) => invoke("dev_log", { msg: String(msg) });
  const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
  // This window is driven by a test. Say so on it: it sits where the user's
  // own Loft would be, and a question typed into it both gets a useless
  // answer (it only knows the sample folder) and stops the test's own.
  document.head.appendChild(
    Object.assign(document.createElement("style"), {
      textContent: `
        .pill .hint { display: none; }
        .pill .brand::after { content: " · TEST RUN, not your Loft"; font-weight: 600; color: #ffb648; }
        .panel::before {
          content: "Automated test in progress. This is not your Loft: please don't type here.";
          flex: none; padding: 5px 14px; font-size: 11px; font-weight: 600; text-align: center;
          color: #1a1205; background: #ffb648;
        }`,
    }),
  );
  const $ = (s, root = document) => root.querySelector(s);
  const $$ = (s, root = document) => [...root.querySelectorAll(s)];
  const text = (el) => (el ? el.innerText.replace(/\s+/g, " ").trim() : "(none)");
  const waitFor = async (fn, ms, what) => {
    const t = Date.now();
    while (Date.now() - t < ms) {
      const v = fn();
      if (v) return v;
      await sleep(150);
    }
    throw new Error("timed out waiting for " + what);
  };
  const KEPT = "Kept for the restart check";

  try {
    // ---- what the database holds, before anything is asked
    const listed = await invoke("list_conversations", { query: null });
    const kept = listed.conversations.find((c) => c.title === KEPT);
    if (!kept) throw new Error(`"${KEPT}" is not in the history after a restart (run smoke-ui.js first): ${listed.conversations.map((c) => c.title).join(" | ")}`);
    const view = await invoke("get_conversation", { id: kept.id });
    const roles = view.messages.map((m) => m.role[0]).join("");
    const answers = view.messages.filter((m) => m.role === "assistant");
    if (!/^(ua)+$/.test(roles) || view.messages[0].content !== "When does the pilot start?" || view.messages.length < 6) {
      throw new Error(`the messages did not come back in order: ${roles}, first "${view.messages[0]?.content}"`);
    }
    if (!kept.titleManual || answers.some((m) => m.status !== "complete" || !m.sources.length || !m.stats)) {
      throw new Error(`the saved answers lost something: ${answers.map((m) => `${m.status}/${m.sources.length} sources/${m.stats ? "stats" : "no stats"}`).join(", ")}`);
    }
    await log(`R1 after a restart | ${listed.total} conversations | "${KEPT}": ${view.messages.length} messages in order (${roles}), every answer complete with its sources (${answers.map((m) => m.sources.length).join(", ")}) and usage figures | name kept as renamed`);

    // An answer stopped before the restart is still a stopped answer; none is left "pending".
    let stopped = 0;
    let pending = 0;
    for (const c of listed.conversations) {
      for (const m of (await invoke("get_conversation", { id: c.id })).messages) {
        if (m.status === "stopped") stopped += 1;
        if (m.status === "pending") pending += 1;
      }
    }
    if (!stopped || pending) throw new Error(`after a restart: ${stopped} stopped answers (expected at least one), ${pending} still pending (expected none)`);
    await log(`R2 unfinished answers | ${stopped} still marked as stopped | ${pending} left pending`);

    // ---- through the UI: open it from the history and carry on
    if (!$(".notch.expanded")) $(".pill").click();
    await waitFor(() => $(".notch.expanded"), 5000, "the panel to open");
    await waitFor(() => !$("textarea").disabled, 180000, "the models to be ready");
    await sleep(500);
    $(".bar-btn").click();
    await waitFor(() => $(".history .list"), 3000, "the conversations panel");
    await sleep(400);
    const row = $$(".history .conv").find((r) => text($(".conv-title", r)) === KEPT);
    if (!row) throw new Error(`"${KEPT}" is not listed in the panel`);
    $(".conv-open", row).click();
    await waitFor(() => !$(".history"), 4000, "the conversation to open");
    await sleep(400);
    const shown = $$(".turn .question").map(text);
    if (shown[0] !== "When does the pilot start?" || shown.length !== view.messages.length / 2 || $$(".turn .sources-toggle").length !== shown.length || text($(".chat-title")) !== KEPT) {
      throw new Error(`the conversation was not shown as saved: ${shown.join(" | ")}, ${$$(".turn .sources-toggle").length} source drawers`);
    }
    const ta = $("textarea");
    ta.value = "Where?";
    ta.dispatchEvent(new Event("input", { bubbles: true }));
    await sleep(100);
    $(".send").click();
    await sleep(300);
    await waitFor(() => !$(".send.stop") && $(".turn.last .turn-stats"), 180000, "the follow-up to be answered");
    const reply = text($(".turn.last .answer"));
    const after = await invoke("get_conversation", { id: kept.id });
    if (!reply.includes("Quayside") || after.messages.length !== view.messages.length + 2) {
      throw new Error(`the follow-up after a restart: "${reply}", ${after.messages.length} messages saved (was ${view.messages.length})`);
    }
    await log(`R3 opened from the history and continued | shown: ${shown.length} turns | follow-up "Where?": ${reply.slice(0, 90)} | now ${after.messages.length} messages saved`);

    await log("R-DONE");
    await sleep(300);
    await invoke("quit_app");
  } catch (e) {
    await log("FAILED: " + (e && e.message ? e.message : e));
    await sleep(500);
    await invoke("quit_app");
  }
})();
