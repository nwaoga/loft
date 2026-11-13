// Installs the optional engines (CUDA, OpenVINO) from inside the real app and
// reports what each one actually does on this computer. Run it on purpose:
//
//   set LOFT_ENGINES_DIR=%TEMP%\loft-smoke-engines
//   scripts\win-smoke.bat smoke-optional-engines.js
//
// It DOWNLOADS about 700 MB from the llama.cpp release pinned in
// src-tauri/engines.json (skipping an engine that is already in that folder).
// With LOFT_ENGINES_DIR set as above the engines go to a test folder, not
// into your own Loft.
//
// Only what must always hold is asserted: the checks around an engine. Whether
// an optional or experimental engine can run the model here is REPORTED as it
// happened, never assumed: "it loaded" is taken from the engine's own log, and
// "it answered" from a real test question.
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
  const summary = (s) => `${s.powerSource}/${s.profile} | chat: ${line(s.chat)} | embed: ${line(s.embed)}`;
  const waitFor = async (check, ms, what) => {
    const t = Date.now();
    let last;
    while (Date.now() - t < ms) {
      last = await check();
      if (last) return last;
      await sleep(500);
    }
    fail(`timed out waiting for ${what}`);
  };
  const settled = (s) => !s.switching && !s.pendingSwitch && s.chat.state !== "loading" && s.embed.state !== "loading";
  const waitSettled = (ms = 180000) => waitFor(async () => ((s) => (settled(s) ? s : null))(await status()), ms, "the engines to settle");
  const backendOf = (o, name) => o.inventory.backends.find((b) => b.backend === name);
  const DEFAULTS = { pluggedIn: { mode: "automatic", target: null, embedTarget: null }, onBattery: { mode: "automatic", target: null, embedTarget: null }, autoSwitch: true, allowFallback: true };
  const apply = (settings) => invoke("set_inference_settings", { settings: { ...DEFAULTS, ...settings } });
  const mb = (bytes) => `${Math.round(bytes / 1e6)} MB`;

  // Every progress event of an install, so its course can be reported.
  const installs = {};
  await window.__TAURI__.event.listen("engine:install", (e) => {
    const p = e.payload;
    const seen = (installs[p.backend] ??= { stages: [], events: 0, most: 0, total: 0, error: null });
    seen.events += 1;
    if (p.stage && seen.stages[seen.stages.length - 1] !== p.stage) seen.stages.push(p.stage);
    seen.most = Math.max(seen.most, p.received);
    seen.total = p.total;
    if (p.error) seen.error = p.error;
  });

  /** Install `backend` by pressing `button`, and report what Loft then found. */
  const installWith = async (backend, button, minutes) => {
    const before = await overview();
    if (backendOf(before, backend).status !== "notInstalled") {
      await log(`I-${backend} already installed in the engines folder: not downloaded again`);
      return;
    }
    const t = Date.now();
    button().click();
    await sleep(1500);
    const bar = text($(".performance .install"));
    const done = await waitFor(
      async () => {
        const o = await overview();
        const note = text($(".performance > .note"));
        if (note) fail(`installing ${backend} failed: ${note}`);
        return backendOf(o, backend).status !== "notInstalled" && !$(".performance .install") ? o : null;
      },
      minutes * 60000,
      `the ${backend} engine to be installed`,
    );
    const seen = installs[backend] ?? { stages: [], events: 0, most: 0, total: 0 };
    if (seen.error) fail(`installing ${backend} reported: ${seen.error}`);
    if (seen.most !== seen.total || !seen.stages.includes("downloading") || !seen.stages.includes("unpacking")) fail(`the ${backend} install did not run its course: ${JSON.stringify(seen)}`);
    const info = backendOf(done, backend);
    const download = done.downloads.find((d) => d.backend === backend);
    if (!download?.installed) fail(`${backend} is not reported as installed in Loft's engines folder`);
    await log(
      `I-${backend} installed in ${Math.round((Date.now() - t) / 1000)} s | ${mb(seen.total)}, checksum verified, ${seen.events} progress events, stages: ${seen.stages.join(" > ")} | while installing the page said: "${bar}" | status: ${info.status}${info.detail ? ` (${info.detail})` : ""} | devices: ${info.devices.map((d) => `${d.runtimeId} = ${d.name} [${d.kind}${d.memoryMib ? `, ${d.memoryMib} MiB` : ""}]`).join("; ") || "none"}`,
    );
  };

  /** Try one target for real and say exactly what happened. */
  const tryTarget = async (option) => {
    const { trial } = await invoke("try_engine", { target: option.target });
    const s = await waitSettled();
    const saved = (await overview()).settings;
    if (JSON.stringify(saved) !== JSON.stringify(DEFAULTS)) fail(`trying ${option.label} changed the saved choices: ${JSON.stringify(saved)}`);
    if (trial.ok) {
      // "ok" must mean: running there, confirmed by the engine's log, and it answered.
      if (!trial.verified || !trial.answer || !(trial.answerTokens > 0) || JSON.stringify(s.chat.activeTarget) !== JSON.stringify(option.target) || s.chat.fallbackUsed) {
        fail(`${option.label} was reported as working without proof: ${JSON.stringify(trial)} | ${summary(s)}`);
      }
    } else if (!trial.problem) {
      fail(`${option.label} failed without an explanation: ${JSON.stringify(trial)}`);
    } else if (s.chat.state === "ready" && JSON.stringify(s.chat.activeTarget) === JSON.stringify(option.target) && !trial.problem.includes("could not answer")) {
      fail(`${option.label} is running although the trial says it failed: ${JSON.stringify(trial)}`);
    }
    await log(
      trial.ok
        ? `I-try ${option.label}: WORKS | ${trial.detail} | ${trial.tokensPerSecond?.toFixed(1)} tok/s, first token ${trial.firstTokenMs} ms, ${trial.answerTokens} tokens | answer: "${(trial.answer ?? "").slice(0, 140)}" | embeddings: ${line(s.embed)}${s.embed.notes.length ? ` (${s.embed.notes[0]})` : ""}${trial.note ? ` | note: ${trial.note}` : ""}`
        : `I-try ${option.label}: DOES NOT WORK | ${trial.problem} | hint: ${trial.hint ?? "none"} | now: ${summary(s)}${trial.note ? ` | note: ${trial.note}` : ""}`,
    );
    return trial;
  };

  try {
    const first = await overview();
    await log(`I1 before | engines: ${first.inventory.backends.map((b) => `${b.backend} ${b.status}`).join(", ")} | optional: ${first.downloads.map((d) => `${d.backend} ${mb(d.sizeBytes)}${d.installed ? " (installed)" : ""}`).join(", ")}`);
    await waitSettled();
    await apply({});
    await waitSettled();
    $(".pill").click();
    await sleep(1200);
    $('[aria-label="Settings"]').click();
    await sleep(1500);

    // ---- I2: OpenVINO, installed from the "Try NPU now" card
    await installWith("openvino", () => $$(".performance .npu button").find((b) => text(b).startsWith("Install the OpenVINO engine")) ?? fail("the NPU card does not offer to install OpenVINO"), 20);
    await sleep(800);
    await log(`I2 the NPU card now says: ${text($(".performance .npu")).slice(0, 300)}`);

    // ---- I3: CUDA, installed from the engines list under Advanced
    $(".performance .advanced").open = true;
    await sleep(300);
    await installWith("cuda", () => $$(".performance .backend").find((b) => text(b).startsWith("CUDA")) ?.querySelector("button") ?? fail("the engines list does not offer to install CUDA"), 45);
    await waitSettled();
    let s = await status();
    await log(`I3 after installing, with Automatic | ${summary(s)} | ${s.chat.detail ?? ""}`);

    // ---- I4: try every device of the new engines, one at a time
    const options = (await overview()).options.filter((o) => o.available && (o.target.backend === "cuda" || o.target.backend === "openvino"));
    if (!options.length) fail("neither new engine offers a device to try");
    const outcome = {};
    for (const option of options) {
      outcome[option.label] = await tryTarget(option);
      await invoke("set_engine_override", { target: null });
      await waitSettled();
    }
    const failed = options.filter((o) => !outcome[o.label].ok);
    const worked = options.filter((o) => outcome[o.label].ok);

    // ---- I5: no fallback: an engine that cannot run the model stops, and says why
    if (failed.length) {
      const target = failed[0].target;
      await apply({ pluggedIn: { mode: "manual", target, embedTarget: null }, allowFallback: false });
      s = await waitSettled();
      const procs = await engines();
      if (s.chat.state === "ready" || s.chat.active || procs.chat !== null) fail(`with fallback off, something still runs in place of ${failed[0].label}: ${summary(s)}`);
      if (!s.chat.error) fail(`the stop is not explained: ${summary(s)}`);
      const kept = (await overview()).settings.pluggedIn.target;
      if (JSON.stringify(kept) !== JSON.stringify(target)) fail("the failed choice was not kept as the saved choice");
      await log(`I5 ${failed[0].label} chosen with fallback off: stopped, choice kept | ${s.chat.error} | hint: ${s.chat.hint ?? "none"}`);
      await apply({});
      await waitSettled();
    } else {
      await log("I5 every new device ran the model, so the no-fallback stop has nothing to show here");
    }

    // ---- I6: an engine in use cannot be removed from under the model
    if (worked.length) {
      const inUse = worked[0];
      await invoke("set_engine_override", { target: inUse.target });
      await waitSettled();
      const refused = await invoke("remove_engine", { backend: inUse.target.backend }).then(() => "removed").catch((e) => String(e));
      if (refused === "removed") fail(`the ${inUse.target.backend} engine was removed while the chat model was running on it`);
      await log(`I6 removing ${inUse.target.backend} while in use: ${refused}`);
      await invoke("set_engine_override", { target: null });
      await waitSettled();
    }

    // ---- I7: measure everything together
    const report = await invoke("run_benchmark", { targets: null });
    s = await waitSettled();
    for (const r of report.rows) {
      await log(
        r.ok
          ? `I7 ${r.label}: ${r.tokensPerSecond.toFixed(1)} tok/s | first token ${r.firstTokenMs} ms | answer ${r.totalMs} ms | load ${r.loadMs} ms | peak ${r.peakMemoryMib} MiB${r.deviceMemoryMib ? ` | ${r.deviceMemoryMib} MiB on device` : ""} | ${r.actual}`
          : `I7 ${r.label}: FAILED | ${r.error}`,
      );
    }
    const pick = report.advice.pluggedIn;
    if (pick && JSON.stringify(s.chat.activeTarget) !== JSON.stringify(pick.target)) fail(`Automatic is not on the fastest measured engine: ${summary(s)} vs ${pick.label}`);
    const procs = await engines();
    await log(`I7 advice: ${pick ? `${pick.label} — ${pick.reason}` : "none"} | Automatic now runs: ${summary(s)} | not measured: ${report.unmeasured.join(", ") || "nothing"} | llama-server processes: ${procs.all.length}`);
    if (procs.all.length !== 2) fail(`expected 2 llama-server processes at the end, found ${procs.all.length}`);

    await log("I-DONE");
    await sleep(300);
    await invoke("quit_app");
  } catch (e) {
    await log("FAILED: " + (e && e.message ? e.message : e));
    await sleep(500);
    await invoke("quit_app");
  }
})();
