// Exercises "Performance & Battery" in the real app (run via scripts\win-smoke.bat
// smoke-engines.js): choosing engines and devices, switching on a (simulated)
// change of power source, waiting for work in progress, fallback with and
// without permission, a temporary override, and crash recovery.
//
// Every claim is checked against the engine status, which is built from what
// the model server itself logged about where it loaded the model. The power
// changes are simulated (nobody pulls the charger); everything else is real:
// real servers, real models, real devices.
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
  const text = (el) => (el ? el.innerText.replace(/\s+/g, " ").trim() : "");
  const fail = (why) => {
    throw new Error(why);
  };

  const overview = () => invoke("get_inference");
  const status = async () => (await overview()).status;
  const engines = () => invoke("dev_engines");
  const line = (e) =>
    `${e.active ?? `[${e.state}]`}${e.verified ? " ✓" : ""}${e.pending ? " (pending)" : ""}${e.fallbackUsed ? " (fallback)" : ""}${e.error ? ` ERROR: ${e.error}` : ""}`;
  const summary = (s) => `${s.powerSource}/${s.profile}${s.pendingSwitch ? " pending" : ""} | chat: ${line(s.chat)} | embed: ${line(s.embed)}`;
  const waitStatus = async (pred, ms, what) => {
    const t = Date.now();
    let last;
    while (Date.now() - t < ms) {
      last = await status();
      if (pred(last)) return last;
      await sleep(300);
    }
    fail(`timed out waiting for ${what}; last: ${summary(last)}`);
  };
  const running = (e, label) => e.state === "ready" && e.verified && (e.active ?? "").includes(label) && !e.pending;
  const both = (label) => (s) => running(s.chat, label) && running(s.embed, label) && !s.switching;

  const auto = () => ({ mode: "automatic", target: null, embedTarget: null });
  const manual = (target, embedTarget = null) => ({ mode: "manual", target, embedTarget });
  const DEFAULTS = { pluggedIn: auto(), onBattery: auto(), autoSwitch: true, allowFallback: true };
  const apply = (settings) => invoke("set_inference_settings", { settings: { ...DEFAULTS, ...settings } });
  const power = (source) => invoke("dev_set_power", { source });
  const SETTLE = 3600; // a little over the 3 s a new power state must hold

  // An answer through the real pipeline; rejects if the model is not running.
  const ask = (question, options = {}) => invoke("ask", { question, history: [], options });
  const expectAnswer = async (what) => {
    const done = await ask("Who owns the supplier contract with BeanWorks?");
    if (!done.sources.length || !done.stats) fail(`${what}: no sourced answer came back`);
    return `${done.stats.answerTokens} tokens at ${Math.round(done.stats.tokensPerSecond ?? 0)}/s`;
  };

  try {
    // ---- E1: what is here, and what started
    const first = await overview();
    const option = (needle) => first.options.find((o) => o.available && o.label.includes(needle)) ?? fail(`no available option matching "${needle}"`);
    const gpus = first.options.filter((o) => o.available && o.target.backend !== "cpu");
    await log(`E1 options: ${first.options.map((o) => `${o.label}${o.available ? "" : " [unavailable]"}`).join("; ")}`);
    if (gpus.length < 2) {
      await log("E-SKIP this test needs two GPU targets (it was written on a laptop with NVIDIA and Intel graphics)");
      await invoke("quit_app");
      return;
    }
    const DISCRETE = option("NVIDIA").target;
    const INTEGRATED = option("Intel").target;
    const CPU = option("CPU only").target;
    await waitStatus(both("NVIDIA"), 120000, "both engines to start on the default (NVIDIA)");
    let s = await status();
    if (s.profile !== "pluggedIn" || s.chat.requested !== "Automatic (performance)") fail(`unexpected start: ${summary(s)}`);
    let procs = await engines();
    if (procs.all.length !== 2) fail(`expected 2 llama-server processes, found ${procs.all.length}`);
    await log(`E1 started | ${summary(s)} | ${s.chat.detail} | answer: ${await expectAnswer("default")}`);

    // ---- E2: a repeated request restarts nothing
    await apply({});
    await apply({});
    await invoke("retry_engines");
    let again = await engines();
    if (again.chat !== procs.chat || again.embed !== procs.embed) fail("re-applying the same settings restarted a server");
    await log("E2 same settings applied twice, then Retry: both servers kept running untouched");

    // ---- E3: choose the integrated GPU, through the Settings UI
    $(".pill").click();
    await sleep(1200);
    $('[aria-label="Settings"]').click();
    await sleep(1500);
    const pickInUi = async (id, needle) => {
      const select = $(id);
      const choice = [...select.options].find((o) => o.text.includes(needle)) ?? fail(`"${needle}" is not offered in ${id}`);
      select.value = choice.value;
      select.dispatchEvent(new Event("change", { bubbles: true }));
    };
    await pickInUi("#perf-plugged", "Intel");
    s = await waitStatus(both("Intel"), 120000, "both engines to move to the Intel GPU");
    again = await engines();
    if (again.chat === procs.chat || again.embed === procs.embed) fail("the servers were not restarted for the new device");
    if (again.all.length !== 2 || again.all.includes(procs.chat) || again.all.includes(procs.embed)) fail(`old servers left behind: ${JSON.stringify(again)}`);
    await sleep(600);
    const shown = text($(".performance .status"));
    if (!shown.includes("Intel") || !shown.includes("Confirmed from the engine's own log")) fail(`the Settings status does not show it: ${shown}`);
    await log(`E3 Vulkan · Intel chosen in Settings | ${summary(s)} | ${s.chat.detail} | answer: ${await expectAnswer("integrated GPU")}`);

    // ---- E4: CPU only really is CPU only
    await apply({ pluggedIn: manual(CPU) });
    s = await waitStatus(both("CPU only"), 120000, "both engines on the CPU");
    if (!s.chat.detail.includes("nothing was offloaded")) fail(`CPU only offloaded something: ${s.chat.detail}`);
    await log(`E4 CPU only | ${s.chat.detail} | answer: ${await expectAnswer("CPU")}`);

    // ---- E5: a different target for the embeddings
    await apply({ pluggedIn: manual(DISCRETE, CPU) });
    s = await waitStatus((x) => running(x.chat, "NVIDIA") && running(x.embed, "CPU only"), 120000, "chat on NVIDIA with embeddings on the CPU");
    const index = (await invoke("list_folders")).map((f) => `${f.fileCount} files/${f.chunkCount} chunks`).join(", ");
    await log(`E5 separate embeddings target | ${summary(s)} | index unchanged: ${index} | answer: ${await expectAnswer("split targets")}`);

    // ---- E6: the power source selects the profile
    await apply({ pluggedIn: manual(DISCRETE), onBattery: manual(CPU) });
    await waitStatus(both("NVIDIA"), 120000, "the plugged-in profile");
    procs = await engines();
    await power("battery");
    s = await waitStatus((x) => x.powerSource === "battery" && x.profile === "onBattery" && both("CPU only")(x), 120000, "the battery profile");
    await log(`E6 unplugged | ${summary(s)} | answer: ${await expectAnswer("on battery")}`);
    procs = await engines();
    for (let i = 0; i < 4; i++) await power("battery"); // duplicate notifications
    await sleep(SETTLE);
    for (const flap of ["ac", "battery", "ac", "battery"]) {
      await power(flap); // a loose charger, ending where it began
      await sleep(250);
    }
    await sleep(SETTLE);
    again = await engines();
    if (again.chat !== procs.chat || again.embed !== procs.embed) fail("duplicate or flapping power notifications restarted a server");
    await power("ac");
    s = await waitStatus((x) => x.powerSource === "ac" && both("NVIDIA")(x), 120000, "the plugged-in profile again");
    await log(`E6 duplicates and a loose charger changed nothing; plugged in again | ${summary(s)}`);

    // ---- E7: auto-switch off keeps the plugged-in choice on battery
    await apply({ pluggedIn: manual(DISCRETE), onBattery: manual(CPU), autoSwitch: false });
    procs = await engines();
    await power("battery");
    await sleep(SETTLE + 1500);
    s = await status();
    again = await engines();
    if (s.powerSource !== "battery" || s.profile !== "pluggedIn" || !running(s.chat, "NVIDIA") || again.chat !== procs.chat) fail(`auto-switch off still switched: ${summary(s)}`);
    await log(`E7 auto-switch off, on battery | ${summary(s)}`);
    await apply({ pluggedIn: manual(DISCRETE), onBattery: manual(CPU) }); // on again: it applies at once
    await waitStatus(both("CPU only"), 120000, "the battery profile once auto-switch is back on");
    await power("ac");
    await waitStatus(both("NVIDIA"), 120000, "the plugged-in profile");

    // ---- E8: a switch waits for an answer in progress, then happens
    // (A question typed into this window, or its Stop button, ends the answer
    // the test is waiting for: that is how Loft is meant to behave, not a
    // failure. It is recognised by the count of questions and stops, said in
    // the log, and the scenario is run again.)
    let answer = "";
    let finished = null;
    let sawPending = null;
    for (let attempt = 1; ; attempt++) {
      await apply({ pluggedIn: manual(CPU), onBattery: manual(DISCRETE) }); // the CPU is slow enough to watch
      await waitStatus(both("CPU only"), 120000, "the CPU for the slow answer");
      procs = await engines();
      answer = "";
      finished = null;
      sawPending = null;
      const unlisten = await window.__TAURI__.event.listen("ask:token", (e) => (answer += e.payload.text));
      const answering = ask("Describe everything in the project plan, the FAQ and the kickoff notes in as much detail as you can.", { length: "detailed", maxTokens: 512 })
        .then((done) => (finished = done))
        .catch((e) => (finished = { error: String(e) }));
      await sleep(700);
      await power("battery");
      while (!finished) {
        const now = await status();
        const pids = await engines();
        if (pids.chat !== procs.chat) fail("the chat server was restarted in the middle of an answer");
        if (now.pendingSwitch && now.chat.pending && !sawPending) sawPending = `${summary(now)} | "${now.message}" (after ${answer.length} characters)`;
        await sleep(250);
      }
      await answering;
      unlisten();
      if (finished.error) fail(`the answer in progress failed: ${finished.error}`);
      const after = await engines();
      const others = after.asks - procs.asks - 1; // questions or stops that were not the test's own
      if (!finished.stats && others > 0 && attempt < 3) {
        await log(`E8 attempt ${attempt}: the answer was stopped after ${answer.length} characters by ${others} other question(s) or stop(s) from this window, not by the switch; running the scenario again`);
        await power("ac");
        await sleep(SETTLE + 500);
        continue;
      }
      if (!sawPending) fail(`the answer ended before the switch could be seen waiting (${answer.length} characters): use a slower target`);
      if (answer.length < 200 || !finished.stats) {
        fail(`the answer was cut short: ${answer.length} characters | stats: ${JSON.stringify(finished.stats)} | other questions or stops: ${others} | chat pid ${procs.chat} -> ${after.chat} | ${summary(await status())} | ends: ${JSON.stringify(answer.slice(-80))}`);
      }
      break;
    }
    s = await waitStatus((x) => both("NVIDIA")(x) && !x.pendingSwitch, 120000, "the pending switch to be carried out");
    await log(`E8 switch waited for the answer | while answering: ${sawPending} | answer complete: ${answer.length} characters, ${finished.stats.answerTokens} tokens | then: ${summary(s)}`);
    await power("ac");
    await waitStatus(both("CPU only"), 120000, "the plugged-in profile");

    // ---- E9: a change of embeddings device during indexing does not disturb it
    const before = (await invoke("list_folders"))[0];
    await invoke("reindex_folder", { id: before.id });
    await sleep(150);
    const during = await apply({ pluggedIn: manual(CPU, INTEGRATED) });
    const waited = during.status.embed.pending;
    await waitStatus((x) => running(x.embed, "Intel") && running(x.chat, "CPU only"), 180000, "the embeddings to move after indexing");
    const t9 = Date.now();
    let after;
    do {
      after = (await invoke("list_folders"))[0];
      await sleep(400);
    } while (after.status !== "ready" && Date.now() - t9 < 120000);
    if (after.status !== "ready" || after.chunkCount !== before.chunkCount) fail(`indexing was disturbed: ${JSON.stringify(after)}`);
    await log(`E9 embeddings moved to Intel during a re-index | switch waited for the run: ${waited} | index rebuilt intact: ${after.fileCount} files, ${after.chunkCount} chunks | answer: ${await expectAnswer("after re-index")}`);

    // ---- E10: fallback allowed: the request is kept, something else runs
    await invoke("dev_fail_target", { target: INTEGRATED });
    await apply({ pluggedIn: manual(INTEGRATED) });
    s = await waitStatus((x) => running(x.chat, "NVIDIA") && x.chat.fallbackUsed, 120000, "a fallback from the failing target");
    const saved = (await overview()).settings.pluggedIn.target;
    if (JSON.stringify(saved) !== JSON.stringify(INTEGRATED) || !s.chat.requested.includes("Intel")) fail("the fallback overwrote the saved choice");
    procs = await engines();
    await sleep(7000); // nothing may loop
    again = await engines();
    if (again.chat !== procs.chat || again.embed !== procs.embed) fail("the failed target was retried in a loop");
    await log(`E10 chosen target failed, fallback on | requested: ${s.chat.requested} | ${summary(s)} | reason: ${s.chat.fallbackReason} | "${s.message}" | answer: ${await expectAnswer("fallback")}`);

    // ---- E11: fallback not allowed: stop, say why, keep the choice
    await apply({ pluggedIn: manual(INTEGRATED), allowFallback: false });
    s = await waitStatus((x) => x.chat.state === "error" && !x.switching, 120000, "the engine to stop");
    again = await engines();
    if (s.chat.active || again.chat !== null) fail(`something is still running without permission to fall back: ${summary(s)}`);
    if (!s.chat.error || !s.chat.requested.includes("Intel")) fail(`the stop is not explained: ${summary(s)}`);
    const refused = await ask("Who owns the supplier contract?").then(() => "answered").catch((e) => String(e));
    if (refused === "answered") fail("a question was answered although the chosen engine failed and fallback is off");
    if (!refused.includes("could not be started")) fail(`the refusal does not say the engine failed: ${refused}`);
    await log(`E11 chosen target failed, fallback off | ${summary(s)} | asking: ${refused} | llama-server processes: ${again.all.length}`);
    await invoke("dev_fail_target", { target: null });
    await invoke("retry_engines");
    s = await waitStatus(both("Intel"), 120000, "Retry to bring the chosen engine up");
    await log(`E11 Retry once the failure is gone | ${summary(s)}`);

    // ---- E12: a temporary engine, through the Settings UI, outlives power changes
    await apply({ pluggedIn: manual(DISCRETE), onBattery: manual(INTEGRATED) });
    await waitStatus(both("NVIDIA"), 120000, "the plugged-in profile");
    await sleep(800);
    const trySelect = $(".performance .try select");
    trySelect.value = ([...trySelect.options].find((o) => o.text.includes("CPU only")) ?? fail("CPU only is not offered to try")).value;
    trySelect.dispatchEvent(new Event("change", { bubbles: true }));
    await sleep(200);
    $$(".performance .try button").find((b) => text(b) === "Try").click();
    s = await waitStatus((x) => x.profile === "override" && running(x.chat, "CPU only"), 120000, "the temporary engine");
    // Trying an engine also asks it a short question and shows what came back.
    const t12 = Date.now();
    while (!text($(".performance .trial")).includes("answered a test question") && Date.now() - t12 < 90000) await sleep(400);
    const tried = text($(".performance .trial"));
    if (!tried.includes("CPU only answered a test question") || !tried.includes("nothing was offloaded") || !/[\d.]+ tokens\/s/.test(tried)) fail(`trying an engine did not report a verified test answer: ${tried}`);
    await log(`E12 tried CPU only | ${tried.slice(0, 330)}`);
    procs = await engines();
    await power("battery");
    await sleep(SETTLE + 1500);
    s = await status();
    again = await engines();
    if (s.profile !== "override" || !running(s.chat, "CPU only") || again.chat !== procs.chat) fail(`a power change ended the temporary engine: ${summary(s)}`);
    const kept = (await overview()).settings;
    if (JSON.stringify(kept.pluggedIn.target) !== JSON.stringify(DISCRETE) || JSON.stringify(kept.onBattery.target) !== JSON.stringify(INTEGRATED)) fail("the temporary engine changed the saved choices");
    await sleep(500);
    const card = text($(".performance .try"));
    await log(`E12 temporary engine, then unplugged | ${summary(s)} | Settings says: ${card.slice(0, 120)}`);
    $$(".performance .try button").find((b) => text(b).startsWith("Restore")).click();
    s = await waitStatus((x) => x.profile === "onBattery" && running(x.chat, "Intel"), 120000, "the battery profile after the temporary engine ends");
    await log(`E12 restored | ${summary(s)}`);
    await power("ac");
    await waitStatus(both("NVIDIA"), 120000, "the plugged-in profile");

    // ---- E13: crash recovery goes through the same coordinator
    procs = await engines();
    const killed = await invoke("dev_kill_engine", { engine: "chat" });
    // A killed process takes a moment to go; the status must notice by
    // itself, before the 3-second crash check gets to it.
    const t13 = Date.now();
    do {
      await sleep(100);
      s = await status();
    } while (s.chat.state === "ready" && Date.now() - t13 < 2500);
    const noticed = Date.now() - t13;
    if (s.chat.state === "ready") fail(`a dead chat server is still reported as running: ${summary(s)}`);
    do {
      await sleep(400);
      again = await engines();
      s = await status();
    } while ((again.chat === killed || again.chat === null || !running(s.chat, "NVIDIA")) && Date.now() - t13 < 60000);
    if (again.chat === killed || again.chat === null || !running(s.chat, "NVIDIA") || again.embed !== procs.embed || again.all.length !== 2) fail(`crash recovery went wrong: ${JSON.stringify(again)} | ${summary(s)}`);
    await log(`E13 chat server killed (pid ${killed}): reported as not running after ${noticed} ms, restarted (pid ${again.chat}) on the same target; embeddings untouched | answer: ${await expectAnswer("after a crash")}`);

    // ---- E14: the NPU is offered (not hidden), and says what it needs here
    const first14 = await overview();
    const openvino = first14.inventory.backends.find((b) => b.backend === "openvino");
    const npuCard = text($(".performance .npu"));
    if (!npuCard.startsWith("Try NPU now")) fail(`the NPU is not offered: ${npuCard}`);
    const npuDevice = (openvino?.devices ?? []).find((d) => d.kind === "npu");
    if (openvino?.status === "notInstalled") {
      const offer = $$(".performance .npu button").find((b) => text(b).startsWith("Install the OpenVINO engine"));
      if (!offer || !/\(\d+ MB\)/.test(text(offer))) fail(`installing OpenVINO is not offered: ${npuCard}`);
      await log(`E14 Try NPU now, OpenVINO not installed: setup is offered, nothing is downloaded by this test | ${npuCard.slice(0, 260)}`);
    } else if (npuDevice) {
      // A computer with an NPU and the engine: really try it. Whatever
      // happens is reported as it happened; nothing here assumes success.
      $$(".performance .npu button").find((b) => text(b) === "Try NPU now").click();
      const t14 = Date.now();
      while (!$(".performance .trial .trial-sub") && Date.now() - t14 < 240000) await sleep(500);
      await sleep(500);
      s = await status();
      await log(`E14 Try NPU now on ${npuDevice.name} | result: ${text($(".performance .trial")).slice(0, 400)} | ${summary(s)}`);
      $$(".performance .try button").find((b) => text(b).startsWith("Restore"))?.click();
      await waitStatus((x) => x.profile !== "override" && !x.switching && x.chat.state === "ready", 120000, "the power profile after the NPU trial");
    } else {
      await log(`E14 Try NPU now, OpenVINO installed but no NPU found | ${npuCard.slice(0, 260)}`);
    }

    // ---- E15: optional engines are listed with their size; guards hold
    $(".performance .advanced").open = true;
    await sleep(300);
    const rows15 = $$(".performance .backend").map((b) => text(b));
    const downloads = first14.downloads.map((d) => `${d.backend} ${Math.round(d.sizeBytes / 1e6)} MB${d.installed ? " (installed)" : ""}${d.blocked ? " (blocked)" : ""}`);
    if (!first14.downloads.some((d) => d.backend === "cuda") || !first14.downloads.some((d) => d.backend === "openvino")) fail(`the optional engines are not listed: ${downloads}`);
    const again15 = await invoke("install_engine", { backend: "vulkan" }).then(() => "installed again").catch((e) => String(e));
    const part15 = await invoke("remove_engine", { backend: "vulkan" }).then(() => "removed").catch((e) => String(e));
    if (again15 === "installed again" || part15 === "removed") fail(`an engine that is part of Loft was reinstalled or removed: ${again15} / ${part15}`);
    await log(`E15 optional engines: ${downloads.join(", ")} | ${rows15.filter((r) => /Install|Remove/.test(r)).map((r) => r.slice(0, 110)).join(" || ")} | reinstalling Vulkan: ${again15} | removing Vulkan: ${part15}`);

    // ---- E16: Test performance, through the Settings UI
    await apply({});
    await waitStatus(both("NVIDIA"), 120000, "the defaults before the performance test");
    procs = await engines();
    $$(".performance .bench button").find((b) => text(b).startsWith("Test")).click();
    await sleep(2500);
    s = await status();
    const during16 = await ask("Who owns the supplier contract?").then(() => "answered").catch((e) => String(e));
    const seen16 = text($(".performance .bench .running"));
    const panel16 = await invoke("get_runtime_status");
    if (panel16.chat !== "loading" || !(panel16.message ?? "").startsWith("Testing engine performance")) fail(`the panel does not say the chat model is on its way: ${JSON.stringify(panel16)}`);
    if (during16 === "answered" || !during16.includes("testing engine performance")) fail(`a question during the performance test was not turned away clearly: ${during16}`);
    if (s.message !== "Testing performance…") fail(`the status does not say a test is running: ${s.message}`);
    const t16 = Date.now();
    while ($(".performance .bench .running") && Date.now() - t16 < 420000) await sleep(1000);
    if ($(".performance .bench .running")) fail("the performance test did not finish");
    const report = await invoke("get_benchmarks");
    const usable = first14.options.filter((o) => o.available).length;
    const good = report.rows.filter((r) => r.ok && r.verified && !r.stale && r.tokensPerSecond > 0 && r.firstTokenMs > 0 && r.loadMs > 0 && r.peakMemoryMib > 0);
    const failed = report.rows.filter((r) => !good.includes(r));
    if (report.rows.length !== usable || failed.some((r) => r.target.backend !== "openvino" || !r.error)) {
      fail(`a standard engine failed or a result is missing: ${JSON.stringify(report.rows.map((r) => [r.label, r.ok, r.verified, r.error, r.stale]))}`);
    }
    const nvidia16 = report.rows.find((r) => r.label.includes("NVIDIA"));
    const cpu16 = report.rows.find((r) => r.label === "CPU only");
    if (!(nvidia16.deviceMemoryMib > 1000) || cpu16.deviceMemoryMib != null) fail(`device memory is not reported as it should be: ${nvidia16.deviceMemoryMib} / ${cpu16.deviceMemoryMib}`);
    if (report.rows.some((r) => r.drainWatts != null || r.joulesPerToken != null)) fail("an energy figure was reported while plugged in");
    if (!report.advice.pluggedIn || report.advice.onBattery || !report.advice.batteryNote.startsWith("Power efficiency is unmeasured")) fail(`unexpected advice: ${JSON.stringify(report.advice)}`);
    s = await waitStatus(both("NVIDIA"), 120000, "the chat model to come back after the test");
    again = await engines();
    if (again.embed !== procs.embed || again.chat === procs.chat || again.all.length !== 2) fail(`the engines after the test are not as expected: ${JSON.stringify(again)}`);
    const shown16 = $$(".performance .bench .row").length;
    const pick16 = text($(".performance .bench .pick"));
    if (shown16 !== usable || !pick16.startsWith("Recommended when plugged in")) fail(`the results are not shown: ${shown16} rows, "${pick16}"`);
    await log(`E16 performance test | while running: "${seen16}", asking: ${during16} | ${report.rows.map((r) => r.ok ? `${r.label}: ${r.tokensPerSecond.toFixed(1)} tok/s, first token ${r.firstTokenMs} ms, load ${r.loadMs} ms, peak ${r.peakMemoryMib} MiB${r.deviceMemoryMib ? `, ${r.deviceMemoryMib} MiB on device` : ""}, ${r.actual}` : `${r.label}: failed — ${r.error}`).join(" || ")}`);
    await log(`E16 advice | ${pick16} | ${report.advice.batteryNote.slice(0, 90)} | afterwards: ${summary(s)} | llama-server processes: ${again.all.length} | answer: ${await expectAnswer("after the performance test")}`);
    // The recommendation is applied only when its button is pressed.
    const before16 = JSON.stringify((await overview()).settings);
    $$(".performance .bench .pick button")[0].click();
    await sleep(1500);
    const after16 = (await overview()).settings;
    if (before16 === JSON.stringify(after16) || after16.pluggedIn.mode !== "manual" || JSON.stringify(after16.pluggedIn.target) !== JSON.stringify(report.advice.pluggedIn.target)) fail("accepting the recommendation did not save it");
    await log(`E16 "Use it" saved the recommended engine as the plugged-in choice: ${JSON.stringify(after16.pluggedIn.target)}`);

    // ---- done: leave the profile as it ships
    await apply({});
    await waitStatus(both("NVIDIA"), 120000, "the defaults");
    const end = await engines();
    await log(`E-DONE defaults restored | llama-server processes: ${end.all.length}`);
    await sleep(300);
    await invoke("quit_app");
  } catch (e) {
    await log("FAILED: " + (e && e.message ? e.message : e));
    await sleep(500);
    await invoke("quit_app"); // so a failed run ends instead of waiting for someone to close Loft
  }
})();
