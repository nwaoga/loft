// Drives the real Loft UI inside the notch's webview (run via LOFT_DEV_EVAL).
// Every step reports through the dev_log command, which also prints the
// native window's size and position.
(async () => {
  const invoke = window.__TAURI__.core.invoke;
  const log = (msg) => invoke("dev_log", { msg: String(msg) });
  const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
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
  const tab = (name) => $$("header.tabs button").find((b) => text(b) === name).click();
  const type = (q) => {
    const ta = $("textarea");
    ta.value = q;
    ta.dispatchEvent(new Event("input", { bubbles: true }));
  };
  // Ask and wait for the answer to finish. Returns timings in ms:
  // "sources/first word/done". (An instant answer never shows Stop, so don't
  // wait for the button; give the UI a moment to clear the previous answer.)
  const askAndWait = async (q) => {
    type(q);
    await sleep(100);
    const t0 = performance.now();
    $(".send").click();
    await sleep(80);
    let tSources = null;
    let tFirst = null;
    await waitFor(
      () => {
        const now = Math.round(performance.now() - t0);
        if (tSources === null && $(".turn.last .sources-toggle")) tSources = now;
        if (tFirst === null && $(".turn.last .answer p:not(.waiting)")) tFirst = now;
        return !$(".send.stop");
      },
      180000,
      "the answer to finish",
    );
    return `${tSources ?? "-"}/${tFirst ?? "-"}/${Math.round(performance.now() - t0)}`;
  };
  // The "Chat & Answers" options sheet (opened from the button beside Send).
  const options = {
    open: async () => {
      if (!$(".chat-options")) {
        $(".options-btn").click();
        await waitFor(() => $(".chat-options"), 3000, "the chat options to open");
        await sleep(250);
      }
    },
    close: async () => {
      if ($(".chat-options")) {
        $('[aria-label="Close chat options"]').click();
        await sleep(250);
      }
    },
    radio: async (name) => {
      await options.open();
      $$(".chat-options .radio")
        .find((b) => text(b).startsWith(name))
        .click();
      await sleep(80);
    },
    length: async (name) => {
      await options.open();
      $$(".chat-options .segments button")
        .find((b) => text(b) === name)
        .click();
      await sleep(80);
    },
    toggle: async (title, on) => {
      await options.open();
      const t = $$(".chat-options .toggle").find((b) => text($(".toggle-title", b)) === title);
      if (t.classList.contains("on") !== on) t.click();
      await sleep(80);
    },
    reset: async () => {
      await options.open();
      $(".chat-options .reset").click();
      await sleep(150);
    },
    saved: () => JSON.parse(localStorage.getItem("loft.chatOptions") || "{}"),
  };
  const words = (t) => t.split(/\s+/).filter(Boolean).length;
  const folderLine = () =>
    $$(".folder").map((f) => `${text($(".name", f))} [${text($(".status", f))}]`).join("; ") || "(no folders)";
  // The folder actions below (rebuild, delete index) are only ever applied to
  // the bundled sample folder, never to a folder the user added themselves.
  const SAMPLE = "sample-docs";
  const sampleRow = () => {
    const row = $$(".folder").find((f) => text($(".name", f)) === SAMPLE);
    if (!row) throw new Error(`the ${SAMPLE} folder is not in the list; add it before running this test`);
    return row;
  };
  const sampleStatus = () => text($(".status", sampleRow()));

  try {
    // (Someone may have opened the panel already, by a click or the hotkey.)
    await log(`1 start | panel open: ${!!$(".notch.expanded")} | pill: ${text($(".pill"))}`);

    if (!$(".notch.expanded")) $(".pill").click();
    await waitFor(() => $(".notch.expanded"), 5000, "the panel to open");
    await sleep(400);
    await log(`2 expanded | hint: ${text($(".hint-card"))} | accel: ${text($(".accel"))} (${$(".accel").title})`);
    await options.reset();
    await options.radio("Collapsed by default");
    await options.close();
    const act = (title) => $(`button[title^="${title}"]`, sampleRow()).click();

    // (A previous run may have left the sample folder un-indexed.)
    if (text($(".hint-card")).includes("Nothing is indexed")) {
      $(".hint-card button").click();
      await waitFor(() => $(".folder"), 5000, "the folder list");
      act("Rebuild");
      await waitFor(() => sampleStatus().includes("watching"), 120000, "indexing to finish");
      await log(`2b indexed via the hint card | ${folderLine()}`);
      tab("Ask");
      await sleep(300);
    }

    // --- Ask: a question the sample folder answers (first one after launch, then again)
    let ms = await askAndWait("Who owns the supplier contract?");
    await log(`3 first answer, sources/first word/done = ${ms} ms | ${text($(".turn.last .answer"))}`);

    // --- Ask: sources are folded away under the answer until opened
    if ($(".turn.last .source")) throw new Error("the sources should start folded away");
    const drawer = $(".turn.last .sources-toggle");
    const drawerLabel = text(drawer);
    drawer.click();
    await waitFor(() => $(".turn.last .source"), 3000, "the sources drawer to open");
    const opened = $$(".turn.last .source-file").map(text).join("; ");
    drawer.click();
    await sleep(200);
    if ($(".turn.last .source")) throw new Error("the sources drawer did not close again");
    await log(`3a sources drawer | closed: "${drawerLabel}" | opened: ${opened} | closed again`);
    ms = await askAndWait("When does the pilot start?");
    await log(`3b second answer, sources/first word/done = ${ms} ms | ${text($(".turn.last .answer"))}`);

    // --- Ask: follow-ups that only make sense with the conversation so far
    ms = await askAndWait("Where?");
    let reply = text($(".turn.last .answer"));
    if (!reply.includes("Quayside")) throw new Error(`the follow-up "Where?" was not understood: ${reply}`);
    await log(`3c follow-up "Where?" in ${ms} ms | ${reply}`);
    ms = await askAndWait("And who owns that contract we talked about first?");
    reply = text($(".turn.last .answer"));
    if (!reply.includes("Priya")) throw new Error(`the follow-up about the contract was not understood: ${reply}`);
    const meterText = text($(".meter"));
    if (!/Context \d+% · [\d,]+ of [\d,]+ tokens/.test(meterText) || !meterText.includes("remembered")) {
      throw new Error(`the context meter is missing or incomplete: "${meterText}"`);
    }
    if ($$(".turn .turn-stats").length !== $$(".turn").length) {
      throw new Error(`expected token counts under every answer: ${$$(".turn .turn-stats").map(text).join(" | ")}`);
    }
    await log(`3d2 context stats | meter: ${meterText} | last answer: ${text($(".turn.last .turn-stats"))} | detail: ${$(".meter").title.replace(/\n/g, " / ")}`);
    await log(
      `3d follow-up in ${ms} ms | ${reply} | turns: ${$$(".turn").length}, earlier turns with a sources drawer: ${$$(".turn:not(.last) .sources-toggle").length} | box: "${$("textarea").placeholder}"`,
    );

    // --- Ask: New chat forgets the conversation
    $(".new-chat").click();
    await sleep(300);
    if ($$(".turn").length || $(".new-chat")) throw new Error("New chat did not clear the conversation");
    ms = await askAndWait("Where?");
    await log(`3e after New chat, "Where?" alone in ${ms} ms | ${text($(".turn.last .answer"))} | turns: ${$$(".turn").length}`);
    $(".new-chat").click();
    await sleep(300);

    // --- Ask: answers that exist only as pictures of text (a PNG and a scanned
    // PDF in sample-docs/scans, read by OCR when the folder is indexed)
    const scanCheck = async (step, question, mustSay, mustCite) => {
      ms = await askAndWait(question);
      reply = text($(".turn.last .answer"));
      if (!mustSay.every((w) => reply.includes(w))) throw new Error(`${step}: expected ${mustSay} in: ${reply}`);
      $(".turn.last .sources-toggle").click();
      await waitFor(() => $(".turn.last .source"), 3000, "the sources drawer to open");
      const cited = $$(".turn.last .source").map((el) => `${text($(".tag", el))} ${text($(".source-file", el))}`);
      if (!cited.some((c) => c.includes(mustCite))) throw new Error(`${step}: ${mustCite} is not among the sources: ${cited.join("; ")}`);
      await log(`${step} in ${ms} ms | ${reply} | cites: ${cited.find((c) => c.includes(mustCite))}`);
      $(".new-chat").click();
      await sleep(300);
    };
    await scanCheck("3f picture of a delivery note", "Which days does BeanWorks deliver to Harbour Cafe?", ["Tuesday", "Friday"], "delivery-note.png");
    await scanCheck("3g scanned PDF", "How much written notice is needed to end the Quayside lease?", ["90"], "lease-letter-scan.pdf");

    // --- Chat & Answers options: each one changes what it says it changes
    await options.open();
    await log(
      `3h options sheet | ${text($(".chat-options h2"))} | sections: ${$$(".chat-options .section-label").map(text).join("; ")} | saved: ${JSON.stringify(options.saved())}`,
    );
    const everyone = "Who is responsible for what in the Harbour Cafe loyalty pilot?";
    await options.length("Concise");
    await options.close();
    await askAndWait(everyone);
    const concise = text($(".turn.last .answer"));
    $(".new-chat").click();
    await sleep(300);
    await options.length("Detailed");
    await options.close();
    await askAndWait(everyone);
    const detailed = text($(".turn.last .answer"));
    $(".new-chat").click();
    await sleep(300);
    if (!concise.includes("Priya") || !detailed.includes("Priya")) throw new Error(`answer length changed the facts: ${concise} / ${detailed}`);
    if (words(concise) >= words(detailed) || words(concise) > 30) {
      throw new Error(`Concise (${words(concise)} words) should be short and shorter than Detailed (${words(detailed)} words)`);
    }
    await log(`3i answer length | concise ${words(concise)} words: ${concise} | detailed ${words(detailed)} words: ${detailed.slice(0, 140)}`);
    await options.length("Balanced");

    // Tone: still the right fact, still with sources.
    await options.radio("Technical");
    await options.close();
    await askAndWait("Who owns the till integration?");
    reply = text($(".turn.last .answer"));
    if (!reply.includes("Tomasz") || !$(".turn.last .sources-toggle")) throw new Error(`technical tone: ${reply}`);
    await log(`3j technical tone | ${reply}`);
    $(".new-chat").click();
    await sleep(300);
    await options.radio("Natural");

    // Answer from my files only, off: general knowledge is allowed and marked.
    await options.toggle("Answer from my files only", false);
    await options.close();
    await askAndWait("What is the capital of Australia?");
    reply = text($(".turn.last .answer"));
    if (!reply.includes("Canberra") || !reply.startsWith("Your indexed files don")) throw new Error(`general knowledge answer: ${reply}`);
    if ($(".turn.last .sources-toggle")) throw new Error("a general-knowledge answer must not list sources");
    await askAndWait("When does the pilot start?");
    const fromFiles = text($(".turn.last .answer"));
    if (!fromFiles.includes("February") || !$(".turn.last .sources-toggle")) throw new Error(`files should still come first: ${fromFiles}`);
    await log(`3k files-only off | ${reply} | and from the files: ${fromFiles}`);
    $(".new-chat").click();
    await sleep(300);
    await options.toggle("Answer from my files only", true);

    // Source panel and citations.
    await options.radio("Auto");
    await options.close();
    await askAndWait("Who owns the supplier contract?");
    await askAndWait("And who signs off the wording?");
    const auto = `latest open: ${$$(".turn.last .source").length > 0}, earlier open: ${$$(".turn:not(.last) .source").length > 0}`;
    await options.radio("Always expanded");
    await options.close();
    const expanded = `every drawer open: ${$$(".turn").every((t) => $(".source", t))}`;
    await options.radio("Collapsed by default");
    await options.close();
    const collapsed = `open drawers: ${$$(".source").length}`;
    await options.toggle("Show citations", false);
    await options.close();
    const hidden = `drawers shown: ${$$(".sources-toggle").length}`;
    await options.toggle("Show citations", true);
    await options.close();
    if (auto !== "latest open: true, earlier open: false" || expanded !== "every drawer open: true" || collapsed !== "open drawers: 0" || hidden !== "drawers shown: 0") {
      throw new Error(`source panel options: auto ${auto}; expanded ${expanded}; collapsed ${collapsed}; citations off ${hidden}`);
    }
    await log(`3l source panel | auto: ${auto} | always expanded: ${expanded} | collapsed: ${collapsed} | citations off: ${hidden}`);

    // Remember conversation context, off: the follow-up is not sent with the turns before it.
    await options.toggle("Remember conversation context", false);
    await options.close();
    await askAndWait("When is the review meeting with finance?");
    const meterOff = text($(".meter"));
    if (meterOff.includes("remembered")) throw new Error(`with context off nothing should be remembered: ${meterOff}`);
    await options.toggle("Remember conversation context", true);
    await options.close();
    await askAndWait("And who is it with?");
    const meterOn = text($(".meter"));
    if (!meterOn.includes("remembered")) throw new Error(`with context on, earlier turns should be sent: ${meterOn}`);
    await log(`3m remember context | off: ${meterOff} | on: ${meterOn}`);

    // Stream responses, off: nothing of the answer shows until it is complete.
    await options.toggle("Stream responses", false);
    await options.close();
    type("Summarise the project plan.");
    await sleep(100);
    $(".send").click();
    await sleep(80);
    let sawPartial = false;
    while ($(".send.stop")) {
      if ($(".turn.last .answer p:not(.waiting)")) sawPartial = true;
      await sleep(20);
    }
    await sleep(150);
    reply = text($(".turn.last .answer"));
    if (sawPartial || reply.length < 20) throw new Error(`with streaming off the answer should arrive whole (partial seen: ${sawPartial}): ${reply}`);
    await log(`3n streaming off | arrived whole: ${reply.slice(0, 100)}`);
    await options.toggle("Stream responses", true);

    // Chat history: saved locally, then not kept at all.
    await options.radio("Save locally");
    await options.close();
    await sleep(300);
    const kept = JSON.parse(localStorage.getItem("loft.chat") || "[]").length;
    if (kept !== $$(".turn").length || kept === 0) throw new Error(`Save locally kept ${kept} of ${$$(".turn").length} turns`);
    await options.radio("Current session only");
    await options.close();
    await sleep(200);
    if (localStorage.getItem("loft.chat") !== null) throw new Error("the saved conversation should be removed when history is no longer saved");
    await log(`3o chat history | saved locally: ${kept} turns | removed again when switched back`);

    // Advanced, then Reset puts everything back.
    await options.open();
    const slider = $("#opt-temperature");
    slider.value = "0.6";
    slider.dispatchEvent(new Event("input", { bubbles: true }));
    const select = $("#opt-max-tokens");
    select.value = "512";
    select.dispatchEvent(new Event("change", { bubbles: true }));
    await sleep(150);
    const advanced = `${options.saved().temperature} / ${options.saved().maxTokens} / button marked: ${$(".options-btn").classList.contains("changed")}`;
    await options.reset();
    const afterReset = JSON.stringify(options.saved());
    if (advanced !== "0.6 / 512 / button marked: true" || $(".options-btn").classList.contains("changed")) {
      throw new Error(`advanced options or reset: ${advanced}; after reset ${afterReset}`);
    }
    await log(`3p advanced and reset | temperature / max tokens: ${advanced} | after Reset: ${afterReset}`);
    await options.radio("Collapsed by default"); // the steps below open drawers themselves
    await options.close();
    $(".new-chat").click();
    await sleep(300);

    // --- Ask: stop mid-answer
    type("Summarise everything decided at the kickoff meeting in detail.");
    await sleep(100);
    $(".send").click();
    await waitFor(() => $(".turn.last .answer p:not(.waiting)"), 120000, "the first words of the answer");
    $(".send.stop").click();
    await waitFor(() => !$(".send.stop"), 5000, "Stop to take effect");
    await log(`4 stopped | partial answer: ${text($(".turn.last .answer"))}`);

    // --- Ask: a request keeps running while other tabs are looked at
    $(".new-chat").click();
    await sleep(300);
    type("In the Harbour Cafe loyalty pilot, who owns what, and what happens on each date in the timeline?");
    await sleep(100);
    $(".send").click();
    await sleep(40);
    const lengths = [];
    for (const name of ["Folders", "Settings", "Folders"]) {
      if (name === "Settings") $('[aria-label="Settings"]').click();
      else tab(name);
      await sleep(120);
      lengths.push(
        `${name} ${text($(".turn.last .answer p:not(.waiting)")).length}${$(".send.stop") ? " running" : " done"}${$(".segmented .answering") ? " marked" : ""}`,
      );
      if (!!$(".send.stop") !== !!$(".segmented .answering")) {
        throw new Error(`the Ask tab's marker does not match the request (${lengths.join(", ")})`);
      }
    }
    await waitFor(() => !$(".send.stop"), 120000, "the answer to finish on another tab");
    await sleep(50);
    if ($(".segmented .answering")) throw new Error("the Ask tab is still marked after the answer finished");
    const hiddenAnswer = $(".turn.last .answer p:not(.waiting)");
    if (!hiddenAnswer || hiddenAnswer.textContent.trim().length < 40 || $(".turn.last .error")) {
      throw new Error(`the answer did not survive switching tabs (${lengths.join(", ")})`);
    }
    tab("Ask");
    await sleep(300);
    await log(`4b answered while on other tabs | characters seen: ${lengths.join(", ")} | ${text($(".turn.last .answer")).slice(0, 120)}`);

    // --- Ask: something the files don't cover (asked as a follow-up to the above)
    ms = await askAndWait("What is the capital of Australia?");
    await log(`5 unanswerable in ${ms} ms | ${text($(".turn.last .answer"))} | sources offered: ${$$(".turn.last .sources-toggle").length}`);

    // --- Folders: rebuild, delete index, rebuild
    tab("Folders");
    await waitFor(() => $(".folder"), 5000, "the folder list");
    await log(`6 folders | ${folderLine()}`);

    // Rebuild: record every distinct status the row shows for 3 s.
    const seen = [];
    const watch = setInterval(() => {
      const s = sampleStatus();
      if (seen[seen.length - 1] !== s) seen.push(s);
    }, 15);
    act("Rebuild");
    await sleep(3000);
    clearInterval(watch);
    await log(`7 statuses during Rebuild | ${seen.join(" -> ")}`);
    await waitFor(() => sampleStatus().includes("watching"), 120000, "the rebuild to finish");
    await log(`8 rebuilt | ${folderLine()}`);

    act("Delete indexed data");
    await waitFor(() => sampleStatus().includes("Not indexed"), 20000, "the index to be deleted");
    await log(`9 index deleted | ${folderLine()}`);
    tab("Ask");
    await sleep(300);
    await log(`10 Ask tab with nothing indexed | hint: ${text($(".hint-card"))}`);
    if ($(".new-chat")) $(".new-chat").click(); // otherwise the conversation so far could answer it
    await sleep(300);
    ms = await askAndWait("Who owns the supplier contract?");
    await log(`10b asked anyway | ${text($(".turn.last .answer"))}`);
    tab("Folders");
    await sleep(300);
    act("Rebuild");
    await waitFor(() => sampleStatus().includes("watching"), 120000, "re-indexing to finish");
    await log(`11 re-indexed | ${folderLine()}`);

    // --- Settings: opened from the gear, holds themes, models and general options
    if ($$(".segmented button").map(text).join(",") !== "Ask,Folders") {
      throw new Error(`unexpected tabs: ${$$(".segmented button").map(text).join(",")}`);
    }
    $('[aria-label="Settings"]').click();
    await waitFor(() => $(".tab-content.shown .settings"), 3000, "Settings to open");
    await sleep(500);
    await log(`12 settings | ${text($(".tab-content.shown")).slice(0, 330)}`);

    // Themes: each one recolours the notch; the original is put back afterwards.
    const original = document.documentElement.dataset.theme;
    const colours = [];
    for (const button of $$(".settings .theme")) {
      button.click();
      await sleep(250);
      if (button.getAttribute("aria-checked") !== "true") throw new Error(`theme ${text(button)} was not selected`);
      colours.push(`${text(button)} ${document.documentElement.dataset.theme} ${getComputedStyle($(".notch")).backgroundColor}`);
    }
    if (new Set(colours.map((c) => c.split(" ").slice(2).join(" "))).size !== colours.length) {
      throw new Error(`themes did not each recolour the notch: ${colours.join("; ")}`);
    }
    const remembered = localStorage.getItem("loft.theme");
    $$(".settings .theme")
      .find((b) => text(b).toLowerCase() === original)
      .click();
    await sleep(250);
    await log(`13 themes | ${colours.join("; ")} | remembered: ${remembered} | restored: ${document.documentElement.dataset.theme}`);

    // Placement: docking on the right edge moves and reshapes the window; the
    // original placement is put back afterwards.
    const place = async (name) => {
      $$(".settings .placement")
        .find((b) => text($(".place-name", b)) === name)
        .click();
      await sleep(700);
    };
    // The window must sit flush with the right edge and centred vertically.
    const checkRight = (geo, what) => {
      const [w, h, x, y] = geo;
      const dpr = window.devicePixelRatio;
      const off = [x + w - screen.width * dpr, y + h / 2 - (screen.height * dpr) / 2];
      if (Math.abs(off[0]) > 2 || Math.abs(off[1]) > 2) {
        throw new Error(`${what}: window ${w}x${h} at ${x},${y} is not centred on the right edge (off by ${off})`);
      }
    };
    // Displays: the checks below measure against the main display, so the
    // notch is moved there first; the original display is put back at the end.
    const displays = () => $$(".settings .display");
    const displayName = (b) => text($(".display-name", b));
    const pickDisplay = async (name) => {
      displays()
        .find((b) => displayName(b) === name)
        .click();
      await sleep(900);
    };
    const startDisplay = displayName(displays().find((b) => b.classList.contains("selected")));
    const mainDisplay = displayName(displays().find((b) => $(".main", b)));
    if (startDisplay !== mainDisplay) await pickDisplay(mainDisplay);
    const mainSize = [screen.width * window.devicePixelRatio, screen.height * window.devicePixelRatio];
    const onMain = ([w, h, x, y]) => x >= 0 && y >= 0 && x + w <= mainSize[0] && y + h <= mainSize[1];

    const startedAt = document.documentElement.dataset.placement;
    if (startedAt !== "top") await place("Top");
    await place("Right");
    if (document.documentElement.dataset.placement !== "right") throw new Error("placement did not switch to right");
    let geo = await log(`13b placement right, panel open | notch ${$(".notch").offsetWidth}x${$(".notch").offsetHeight}`);
    checkRight(geo, "right, panel open");
    $('[aria-label="Collapse"]').click();
    await sleep(1100);
    geo = await log(`13c placement right, collapsed | pill ${$(".notch").offsetWidth}x${$(".notch").offsetHeight} | pill text: ${text($(".pill"))}`);
    checkRight(geo, "right, collapsed");
    if ($(".notch").offsetWidth >= $(".notch").offsetHeight) throw new Error("the pill should stand upright on the right edge");
    $(".pill").click();
    await waitFor(() => $(".notch.expanded"), 5000, "the panel to reopen on the right");
    await sleep(600);
    // (Growing at the screen edge once sent the window to the next display.)
    geo = await log(`13c2 placement right, reopened | notch ${$(".notch").offsetWidth}x${$(".notch").offsetHeight}`);
    checkRight(geo, "right, reopened");
    if (!$(".tab-content.shown .settings")) $('[aria-label="Settings"]').click();
    await sleep(300);
    await place("Top");
    geo = await log(`13d placement top again | ${document.documentElement.dataset.placement} | notch ${$(".notch").offsetWidth}x${$(".notch").offsetHeight}`);
    if (geo[3] !== 0 || Math.abs(geo[2] + geo[0] / 2 - (screen.width * window.devicePixelRatio) / 2) > 2) {
      throw new Error(`top: window ${geo} is not centred on the top edge`);
    }

    // Another display: the notch moves there and stays there through
    // collapsing, reopening and docking on the right.
    const others = displays().map(displayName).filter((n) => n !== mainDisplay);
    if (others.length) {
      const other = others[0];
      await pickDisplay(other);
      const steps = [];
      const offMain = async (what) => {
        const g = await log(`13e ${other}: ${what}`);
        if (onMain(g)) throw new Error(`${what}: the window ${g} is on the main display, not on ${other}`);
        steps.push(what);
      };
      await offMain("moved, top, panel open");
      await place("Right");
      await offMain("right, panel open");
      $('[aria-label="Collapse"]').click();
      await sleep(1100);
      await offMain("right, collapsed");
      $(".pill").click();
      await waitFor(() => $(".notch.expanded"), 5000, `the panel to reopen on ${other}`);
      await sleep(600);
      await offMain("right, reopened");
      if (!$(".tab-content.shown .settings")) $('[aria-label="Settings"]').click();
      await sleep(400);
      await place("Top");
      await offMain("top again");
      await pickDisplay(mainDisplay);
      const back = await log(`13f back on ${mainDisplay} | displays: ${displays().map((b) => text(b) + (b.classList.contains("selected") ? " *" : "")).join("; ")}`);
      if (!onMain(back)) throw new Error(`the window ${back} did not return to ${mainDisplay}`);
    } else {
      await log("13e only one display attached: display switching not exercised");
    }
    if (startDisplay !== mainDisplay) await pickDisplay(startDisplay);
    if (startedAt === "right") await place("Right");

    // Picture descriptions: with the picture model on, a picture with no text
    // on it (sample-docs/scans/sales-chart.png) can be found by what it shows.
    // The setting is put back afterwards.
    const pictureRow = () => $$(".settings .model").find((m) => text($(".name", m)).includes("SmolVLM"));
    if (pictureRow() && !$(".dl", pictureRow())) {
      const wasOn = !!$(".active", pictureRow());
      const setPictures = async (on) => {
        const button = $$("button", pictureRow()).find((b) => text(b) === (on ? "Turn on" : "Turn off"));
        if (!button) return;
        button.click();
        // Every folder reads its pictures again; wait for that to start and finish.
        await sleep(2500);
        await waitFor(
          () => $$(".folder").every((f) => /watching|Not indexed/.test(text($(".status", f)))),
          300000,
          "the folders to finish reading their pictures again",
        );
      };
      const chartIsCited = async () => {
        if (!$(".tab-content.shown .ask")) tab("Ask");
        await sleep(300);
        if ($(".new-chat")) $(".new-chat").click();
        await sleep(300);
        ms = await askAndWait("Which picture shows a bar chart?");
        const answer = text($(".turn.last .answer"));
        let cited = [];
        if ($(".turn.last .sources-toggle")) {
          $(".turn.last .sources-toggle").click();
          await waitFor(() => $(".turn.last .source"), 3000, "the sources drawer to open");
          cited = $$(".turn.last .source").map((el) => `${text($(".source-file", el))}: ${text($(".snippet", el)).slice(0, 110)}`);
        }
        $(".new-chat").click();
        await sleep(300);
        return { answer, chart: cited.find((c) => c.startsWith("sales-chart.png")) };
      };
      const backToSettings = async () => {
        if (!$(".tab-content.shown .settings")) $('[aria-label="Settings"]').click();
        await sleep(400);
      };

      await setPictures(true);
      const on = await chartIsCited();
      if (!on.chart) throw new Error(`with picture descriptions on, sales-chart.png was not cited: ${on.answer}`);
      await log(`13g picture descriptions on | ${on.answer} | cites ${on.chart}`);
      await backToSettings();
      if (!wasOn) {
        await setPictures(false);
        const off = await chartIsCited();
        if (off.chart) throw new Error("with picture descriptions off, the chart should no longer be found");
        await log(`13h picture descriptions off again | ${off.answer}`);
        await backToSettings();
      }
    } else {
      await log("13g picture model not downloaded: picture descriptions not exercised");
    }

    $('.settings [role="switch"]').click();
    await sleep(600);
    await log(`14 launch-at-login in a dev build | on: ${$('.settings [role="switch"]').getAttribute("aria-checked")} | ${text($(".settings .note"))}`);
    document.dispatchEvent(new KeyboardEvent("keydown", { key: "Escape", bubbles: true }));
    await sleep(300);
    await log(
      `15 after Escape | settings open: ${!!$(".tab-content.shown .settings")} | back on: ${text($(".segmented button.active"))} | panel open: ${!!$(".notch.expanded")}`,
    );

    // --- Collapse (the window should shrink after the animation)
    $('[aria-label="Collapse"]').click();
    await sleep(1000);
    await log(`16 collapsed again | panel open: ${!!$(".notch.expanded")}`);

    // --- Quit from Settings
    $(".pill").click();
    await waitFor(() => $(".notch.expanded"), 5000, "the panel to reopen");
    $('[aria-label="Settings"]').click();
    await waitFor(() => $(".tab-content.shown .settings .quit"), 3000, "the Quit button");
    if ($(".tab-content.shown .settings")) $('[aria-label="Settings"]').click();
    await sleep(300);
    await options.reset(); // leave the chat options as they ship
    await options.close();
    $('[aria-label="Settings"]').click();
    await waitFor(() => $(".tab-content.shown .settings .quit"), 3000, "Settings to reopen");
    await log("17 DONE - clicking Quit");
    await sleep(300);
    $(".settings .quit").click();
  } catch (e) {
    await log("FAILED: " + (e && e.message ? e.message : e));
    await sleep(500);
    await invoke("quit_app"); // so a failed run ends instead of waiting for someone to close Loft
  }
})();
