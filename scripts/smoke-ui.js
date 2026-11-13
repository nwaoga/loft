// Drives the real Loft UI inside the notch's webview (run via LOFT_DEV_EVAL).
// Every step reports through the dev_log command, which also prints the
// native window's size and position.
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
  const waitForAsync = async (fn, ms, what) => {
    const t = Date.now();
    while (Date.now() - t < ms) {
      if (await fn()) return;
      await sleep(250);
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
  // Chat history: the saved conversations, and the panel that lists them.
  const chats = (query = null) => invoke("list_conversations", { query });
  const saved = (id) => invoke("get_conversation", { id });
  const convs = {
    open: async () => {
      if (!$(".history")) $(".bar-btn").click();
      await waitFor(() => $(".history .list"), 3000, "the conversations panel");
      await sleep(400);
    },
    close: async () => {
      if ($(".history")) $('.history [aria-label="Back to the conversation"]').click();
      await sleep(250);
    },
    titles: () => $$(".history .conv-title").map(text),
    row: (title) => $$(".history .conv").find((r) => text($(".conv-title", r)) === title) ?? (() => { throw new Error(`"${title}" is not in the conversations list: ${$$(".history .conv-title").map(text).join(" | ")}`); })(),
    search: async (q) => {
      const box = $(".history .search input");
      box.value = q;
      box.dispatchEvent(new Event("input", { bubbles: true }));
      await sleep(600);
    },
    resume: async (title) => {
      await convs.open();
      $(".conv-open", convs.row(title)).click();
      await waitFor(() => !$(".history"), 4000, `"${title}" to open`);
      await sleep(350);
    },
    menu: async (title, item) => {
      await convs.open();
      const row = convs.row(title);
      $(".conv-more", row).click();
      await sleep(150);
      $$(".conv-menu button", row).find((b) => text(b) === item).click();
      await sleep(200);
      return row;
    },
  };
  const key = (name) => document.activeElement.dispatchEvent(new KeyboardEvent("keydown", { key: name, bubbles: true }));
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
    await waitForAsync(
      async () => (await invoke("list_folders")).some((f) => f.path.endsWith(SAMPLE) && f.status === "ready" && f.chunkCount > 0),
      120000,
      "the sample folder to be searchable",
    );

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
    // (Text in pictures is read by Windows' own OCR engine; other systems index no text for these yet.)
    if (navigator.userAgent.includes("Windows")) {
      await scanCheck("3f picture of a delivery note", "Which days does BeanWorks deliver to Harbour Cafe?", ["Tuesday", "Friday"], "delivery-note.png");
      await scanCheck("3g scanned PDF", "How much written notice is needed to end the Quayside lease?", ["90"], "lease-letter-scan.pdf");
    } else {
      await log("3f, 3g skipped | reading text in pictures and scanned PDFs (OCR) is Windows-only for now");
    }

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

    // Small talk gets a friendly reply; anything else the files do not cover
    // is declined, naming the folders Loft is here for.
    await askAndWait("hi");
    const hello = text($(".turn.last .answer"));
    if (!hello || /couldn.t find|only really here/i.test(hello)) throw new Error(`a greeting should be answered as one: ${hello}`);
    if ($(".turn.last .sources-toggle")) throw new Error("a greeting must not list sources");
    await askAndWait("What is the capital of Australia?");
    reply = text($(".turn.last .answer"));
    if (!/couldn.t find that in your indexed files\. I.m only really here to talk about items in .*sample-docs/.test(reply)) {
      throw new Error(`out of scope should be declined naming the folder: ${reply}`);
    }
    if ($(".turn.last .sources-toggle")) throw new Error("a decline must not list sources");
    await log(`3k2 small talk and scope | hi: ${hello} | capital of Australia: ${reply}`);
    $(".new-chat").click();
    await sleep(300);

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

    // Chat history option: "Current session only" keeps a new conversation
    // out of the history; "Save locally" (the default) puts it there.
    const total3o = (await chats()).total;
    await options.radio("Current session only");
    await options.close();
    $(".new-chat").click();
    await sleep(300);
    await askAndWait("When does the pilot start?");
    const unsavedBar = text($(".chat-title"));
    const unsavedTotal = (await chats()).total;
    if (unsavedTotal !== total3o || unsavedBar !== "Not being saved") throw new Error(`"Current session only" still saved the conversation: ${unsavedTotal} vs ${total3o}, bar "${unsavedBar}"`);
    await options.radio("Save locally");
    await options.close();
    $(".new-chat").click();
    await sleep(300);
    await askAndWait("When does the pilot start?");
    const savedTotal = (await chats()).total;
    if (savedTotal !== total3o + 1 || text($(".chat-title")) !== "When does the pilot start") throw new Error(`"Save locally" did not save the conversation: ${savedTotal} vs ${total3o}, bar "${text($(".chat-title"))}"`);
    await log(`3o chat history option | current session only: not saved (bar says "${unsavedBar}") | save locally: saved as "${text($(".chat-title"))}"`);

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

    // --- Chat history: conversations are kept, listed, resumed, searched,
    // renamed and deleted, all inside the panel as it is.
    const dpr0 = window.devicePixelRatio;
    const totalBefore = (await chats()).total;
    if (text($(".chat-title")) !== "New conversation" || $(".new-chat")) throw new Error(`a fresh chat should have no name and no New chat button: "${text($(".chat-bar"))}"`);
    await askAndWait("When does the pilot start?");
    const titleA = text($(".chat-title"));
    await askAndWait("Where?");
    if (!text($(".turn.last .answer")).includes("Quayside")) throw new Error(`the follow-up was not understood: ${text($(".turn.last .answer"))}`);
    $(".new-chat").click();
    await sleep(300);
    await askAndWait("Who owns the supplier contract?");
    const titleB = text($(".chat-title"));
    if (titleA !== "When does the pilot start" || titleB !== "Who owns the supplier contract") throw new Error(`unexpected titles: "${titleA}", "${titleB}"`);
    if ((await chats()).total !== totalBefore + 2) throw new Error("two conversations should have been saved");

    $(".chat-title").click(); // the title opens the history too
    await waitFor(() => $(".history .list"), 3000, "the conversations panel");
    await sleep(400);
    let panel = await log(
      `3q history panel | groups: ${$$(".history .group-label").map(text).join(", ")} | first rows: ${convs.titles().slice(0, 3).join(" / ")} | open now: ${text($(".history .conv.active .conv-title"))} | ${text($(".history footer"))}`,
    );
    if (panel[0] !== Math.round(540 * dpr0) || panel[1] !== Math.round(580 * dpr0)) throw new Error(`the history changed the window's size: ${panel}`);
    if (convs.titles()[0] !== titleB || convs.titles()[1] !== titleA || text($(".history .conv.active .conv-title")) !== titleB) throw new Error(`the list is not newest-first with the open conversation marked: ${convs.titles().join(" | ")}`);
    if (!$$(".history .group-label").map(text).some((g) => g.toLowerCase() === "today")) throw new Error("conversations are not grouped by date");

    // Resume the first one: its messages and sources come back, in order,
    // scrolled to the latest, and a follow-up continues it.
    $(".conv-open", convs.row(titleA)).click();
    await waitFor(() => !$(".history"), 4000, "the conversation to open");
    await sleep(350);
    const thread = $(".thread");
    const resumed = $$(".turn .question").map(text).join(" | ");
    if (resumed !== "When does the pilot start? | Where?" || $$(".turn .sources-toggle").length !== 2 || text($(".chat-title")) !== titleA) {
      throw new Error(`the conversation was not restored: "${resumed}", ${$$(".turn .sources-toggle").length} source drawers, bar "${text($(".chat-title"))}"`);
    }
    if (thread.scrollHeight - thread.scrollTop - thread.clientHeight > 60) throw new Error("a resumed conversation should be scrolled to its latest message");
    await askAndWait("Where?");
    const resumedReply = text($(".turn.last .answer"));
    const memory = text($(".meter"));
    if (!resumedReply.includes("Quayside") || !memory.includes("remembered")) throw new Error(`the follow-up in a resumed conversation did not use it: ${resumedReply} | ${memory}`);
    // Nothing leaked between the two: each has its own messages and sources.
    let all = (await chats()).conversations;
    const idA = all.find((c) => c.title === titleA).id;
    const idB = all.find((c) => c.title === titleB).id;
    let viewA = await saved(idA);
    const viewB = await saved(idB);
    const shape = (v) => v.messages.map((m) => `${m.role[0]}:${m.status}:${m.sources.length}`).join(" ");
    if (viewA.messages.length !== 6 || viewB.messages.length !== 2 || viewB.messages[0].content !== "Who owns the supplier contract?") throw new Error(`messages are in the wrong conversation: A ${shape(viewA)} | B ${shape(viewB)}`);
    if (viewA.messages.some((m) => m.role === "assistant" && (m.status !== "complete" || !m.sources.length || m.sources.some((s) => s.state !== "same") || !m.stats || !m.model))) {
      throw new Error(`a saved answer is missing its status, sources, stats or model: ${shape(viewA)}`);
    }
    await log(`3r resumed "${titleA}" | questions: ${resumed} | follow-up: ${resumedReply.slice(0, 80)} | ${memory} | saved as: ${shape(viewA)} | the other conversation: ${shape(viewB)}`);

    // Search (titles and what was said), then the keyboard.
    // Every conversation listed for a search must really contain it (in its
    // title or in something that was said), and the list must be the same as
    // the one the store gives. Other steps have left conversations here too.
    const searched = async (term) => {
      await convs.search(term);
      const shown = convs.titles();
      const found = (await chats(term)).conversations;
      if (shown.join(" | ") !== found.map((c) => c.title).join(" | ")) throw new Error(`searching "${term}": the panel shows ${shown.join(" | ")}, the store found ${found.map((c) => c.title).join(" | ")}`);
      for (const c of found) {
        const view = await saved(c.id);
        const everything = [c.title, ...view.messages.map((m) => m.content)].join("\n").toLowerCase();
        if (!everything.includes(term.toLowerCase())) throw new Error(`searching "${term}" listed "${c.title}", which does not contain it`);
      }
      return found;
    };
    await convs.open();
    const inHistory = (await chats()).total;
    const byAnswer = await searched("Quayside"); // only ever in answers, never in a title
    const byTitle = await searched("SUPPLIER");
    await convs.search("no such words anywhere");
    const nothing = text($(".history .empty"));
    const noRows = convs.titles().length;
    await convs.search("");
    if (!byAnswer.some((c) => c.id === idA) || byAnswer.some((c) => c.title.includes("Quayside")) || !byTitle.some((c) => c.id === idB) || byAnswer.length >= inHistory || noRows || !nothing.includes("Nothing matches") || convs.titles().length !== inHistory) {
      throw new Error(`search: "Quayside" ${byAnswer.length} of ${inHistory}, "SUPPLIER" ${byTitle.length}, no match: ${noRows} rows "${nothing}", cleared: ${convs.titles().length}`);
    }
    $(".history .search input").focus();
    key("ArrowDown");
    const firstFocus = text($(".conv-title", document.activeElement.closest(".conv")));
    key("ArrowDown");
    const secondFocus = text($(".conv-title", document.activeElement.closest(".conv")));
    key("ArrowUp");
    key("ArrowUp");
    const backInSearch = document.activeElement === $(".history .search input");
    key("Escape");
    await sleep(250);
    if (!firstFocus || firstFocus === secondFocus || !backInSearch || $(".history") || !$(".notch.expanded")) {
      throw new Error(`keyboard: Down -> "${firstFocus}", Down -> "${secondFocus}", Up Up back in search: ${backInSearch}, Esc closed history: ${!$(".history")}, panel still open: ${!!$(".notch.expanded")}`);
    }
    await log(`3s search and keys | "Quayside" (said in answers): ${byAnswer.length} of ${inHistory} conversations, each containing it | "SUPPLIER": ${byTitle.length} | no match: ${nothing} | Down, Down: ${firstFocus} -> ${secondFocus} | Esc closes the history only`);

    // Rename: kept as typed, even a very long one fits the row.
    let row = await convs.menu(titleB, "Rename");
    let box = $(".history .rename-box");
    const longName = "Supplier contract: who owns it, when it renews, and everything else we know about BeanWorks and the beans";
    box.value = longName;
    box.dispatchEvent(new Event("input", { bubbles: true }));
    box.dispatchEvent(new KeyboardEvent("keydown", { key: "Enter", bubbles: true }));
    await sleep(500);
    const longRow = $(".conv-title", convs.row(longName));
    const fits = longRow.scrollWidth > longRow.clientWidth && $(".history").scrollWidth <= $(".history").clientWidth;
    const manual = (await chats()).conversations.find((c) => c.id === idB);
    if (!fits || manual.title !== longName || !manual.titleManual) throw new Error(`rename: title "${manual.title}", manual ${manual.titleManual}, long title cut to fit ${fits}`);
    await log(`3t renamed | "${manual.title.slice(0, 40)}…" kept as typed and shown cut to fit the row`);

    // Delete: asks first; Cancel keeps it; Delete removes it and nothing else.
    const indexBefore = (await invoke("list_folders")).map((f) => `${f.fileCount}/${f.chunkCount}`).join(",");
    row = await convs.menu(longName, "Delete");
    const question = text($(".conv-confirm", row));
    $$(".conv-confirm button", row).find((b) => text(b) === "Cancel").click();
    await sleep(250);
    const keptAfterCancel = convs.titles().includes(longName);
    row = await convs.menu(longName, "Delete");
    $$(".conv-confirm button", row).find((b) => text(b) === "Delete").click();
    await sleep(600);
    const goneFromList = !convs.titles().includes(longName);
    const goneFromStore = await saved(idB).then(() => false, () => true);
    const indexAfter = (await invoke("list_folders")).map((f) => `${f.fileCount}/${f.chunkCount}`).join(",");
    if (!question.startsWith("Delete this conversation?") || !keptAfterCancel || !goneFromList || !goneFromStore || indexBefore !== indexAfter) {
      throw new Error(`delete: asked "${question}", kept after Cancel ${keptAfterCancel}, gone ${goneFromList}/${goneFromStore}, index ${indexBefore} -> ${indexAfter}`);
    }
    // Deleting the conversation that is open leaves a fresh chat.
    await convs.close();
    $(".new-chat").click();
    await sleep(300);
    await askAndWait("When does the pilot start?");
    const openTitle = text($(".chat-title"));
    row = await convs.menu(openTitle, "Delete");
    // (two conversations share that title now: the open one is the newest, listed first)
    $$(".conv-confirm button", $(".history .conv.active")).find((b) => text(b) === "Delete").click();
    await sleep(600);
    await convs.close();
    if ($$(".turn").length || text($(".chat-title")) !== "New conversation") throw new Error(`deleting the open conversation should leave a fresh chat: ${$$(".turn").length} turns, bar "${text($(".chat-title"))}"`);
    await log(`3u deleted | asked: "${question}" | Cancel kept it, Delete removed it and its messages | indexed files untouched (${indexAfter}) | deleting the open one left a fresh chat`);

    // --- Ask: stop mid-answer
    type("Summarise everything decided at the kickoff meeting in detail.");
    await sleep(100);
    $(".send").click();
    await waitFor(() => $(".turn.last .answer p:not(.waiting)"), 120000, "the first words of the answer");
    $(".send.stop").click();
    await waitFor(() => !$(".send.stop"), 5000, "Stop to take effect");
    await log(`4 stopped | partial answer: ${text($(".turn.last .answer"))}`);
    // It is saved, but as what it is: reopened later it still says "stopped".
    await sleep(300);
    const stoppedTitle = text($(".chat-title"));
    const stoppedView = await saved((await chats()).conversations[0].id);
    const stoppedMessage = stoppedView.messages[stoppedView.messages.length - 1];
    $(".new-chat").click();
    await sleep(300);
    await convs.resume(stoppedTitle);
    const stoppedNote = text($(".turn.last .turn-note")) === "(none)" ? text($(".turn.last .stopped")) : text($(".turn.last .turn-note"));
    if (stoppedMessage.status !== "stopped" || !stoppedNote.startsWith("Stopped")) throw new Error(`a stopped answer was saved as "${stoppedMessage.status}" and reopens saying "${stoppedNote}"`);
    await log(`4a the stopped answer in the history | saved as: ${stoppedMessage.status} (${stoppedMessage.content.length} characters kept) | reopened it says: ${stoppedNote}`);

    // Starting a new chat while an answer is being written: that answer stays
    // with its own conversation (as stopped), and none of it reaches the next.
    $(".new-chat").click();
    await sleep(300);
    type("Summarise everything decided at the kickoff meeting in detail.");
    await sleep(100);
    $(".send").click();
    await waitFor(() => $(".turn.last .answer p:not(.waiting)"), 120000, "the first words of the answer");
    const abandonedStart = text($(".turn.last .answer")).slice(0, 12);
    $(".new-chat").click(); // stops it and starts afresh
    await waitFor(() => !$(".send.stop"), 5000, "the abandoned answer to stop");
    await sleep(200);
    if ($$(".turn").length || text($(".chat-title")) !== "New conversation") throw new Error("New chat during an answer should leave an empty, unnamed conversation");
    await askAndWait("Who owns the supplier contract?");
    const nextReply = text($(".turn.last .answer"));
    const newest = (await chats()).conversations;
    const nextView = await saved(newest[0].id);
    const abandoned = await saved(newest[1].id);
    const abandonedAnswer = abandoned.messages[abandoned.messages.length - 1];
    if ($$(".turn").length !== 1 || !nextReply.includes("Priya") || nextView.messages.length !== 2 || nextView.messages[1].status !== "complete") {
      throw new Error(`the conversation after New chat is not just its own question and answer: ${$$(".turn").length} turns, ${nextView.messages.length} saved, "${nextReply}"`);
    }
    if (abandonedAnswer.status !== "stopped" || abandoned.messages.length !== 2 || abandoned.messages[0].content !== "Summarise everything decided at the kickoff meeting in detail." || abandonedAnswer.content.includes("Priya")) {
      throw new Error(`the abandoned answer is not where it belongs: ${abandoned.messages.length} messages, last ${abandonedAnswer.status}: "${abandonedAnswer.content.slice(0, 60)}"`);
    }
    await log(`4a2 New chat during an answer | the abandoned answer ("${abandonedStart}…") stayed in its own conversation as ${abandonedAnswer.status}, ${abandonedAnswer.content.length} characters | the next conversation holds only its own question and answer: ${nextReply.slice(0, 70)}`);

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
    // An old conversation still shows what its answers were written from,
    // marked as no longer available: nothing is opened, nothing is re-matched.
    await convs.resume(titleA);
    $(".turn .sources-toggle").click();
    await waitFor(() => $(".turn .source"), 3000, "the sources of a saved answer");
    const goneCards = $$(".turn .source");
    const goneLabel = text($(".source-file", goneCards[0]));
    if (!goneCards.length || goneCards.some((c) => !c.disabled || !c.classList.contains("gone")) || !goneLabel.includes("no longer available")) {
      throw new Error(`with the index deleted, saved citations should be shown as unavailable: "${goneLabel}", disabled ${goneCards.map((c) => c.disabled)}`);
    }
    await log(`10a saved citations with the index deleted | ${goneCards.length} shown, none can be opened | "${goneLabel}" | snippet kept: ${text($(".snippet", goneCards[0])).slice(0, 60)}`);
    if ($(".new-chat")) $(".new-chat").click(); // otherwise the conversation so far could answer it
    await sleep(300);
    ms = await askAndWait("Who owns the supplier contract?");
    await log(`10b asked anyway | ${text($(".turn.last .answer"))}`);
    tab("Folders");
    await sleep(300);
    act("Rebuild");
    await waitFor(() => sampleStatus().includes("watching"), 120000, "re-indexing to finish");
    await log(`11 re-indexed | ${folderLine()}`);
    tab("Ask");
    await sleep(300);
    await convs.resume(titleA);
    $(".turn .sources-toggle").click();
    await waitFor(() => $(".turn .source"), 3000, "the sources of a saved answer");
    const backCards = $$(".turn .source");
    if (backCards.some((c) => c.disabled) || text($(".source-file", backCards[0])).includes("no longer available")) throw new Error("with the files indexed again, saved citations should be usable again");
    await log(`11a saved citations with the files indexed again | ${backCards.length} shown, all can be opened | "${text($(".source-file", backCards[0]))}"`);
    // Left for scripts\smoke-restart.js, which checks it after Loft has been closed and started again.
    row = await convs.menu(titleA, "Rename");
    box = $(".history .rename-box");
    box.value = "Kept for the restart check";
    box.dispatchEvent(new Event("input", { bubbles: true }));
    box.dispatchEvent(new KeyboardEvent("keydown", { key: "Enter", bubbles: true }));
    await sleep(500);
    await convs.close();
    tab("Folders");
    await sleep(300);

    // --- Settings: opened from the gear, holds themes, models and general options
    if ($$(".segmented button").map(text).join(",") !== "Ask,Folders") {
      throw new Error(`unexpected tabs: ${$$(".segmented button").map(text).join(",")}`);
    }
    $('[aria-label="Settings"]').click();
    await waitFor(() => $(".tab-content.shown .settings"), 3000, "Settings to open");
    await sleep(500);
    await log(`12 settings | ${text($(".tab-content.shown")).slice(0, 330)}`);

    // Usage: its own view from the header, with real figures for both model
    // servers and Loft itself. It is no longer inside Settings.
    if (text($(".performance")).includes("What Loft is using")) throw new Error("usage is still shown in Settings");
    $('[aria-label="Usage"]').click();
    await waitFor(() => $(".tab-content.shown .usage-view .part"), 8000, "the usage view");
    await sleep(2500); // the second reading has processor figures
    const parts = $$(".usage-view .part").map((p) => text(p.querySelector(".part-name")));
    for (const want of ["Chat model", "Embedding model", "Loft and its window"]) {
      if (!parts.includes(want)) throw new Error(`usage view lacks ${want}: ${parts.join(", ")}`);
    }
    const meters = $$(".usage-view .meter").map((m) => `${text(m.querySelector(".meter-title"))}: ${text(m.querySelector(".meter-figure"))} (${text(m.querySelector(".meter-caption"))})`);
    if (!meters.some((m) => /^Memory: (\d+%|<1%) used/.test(m)) || !meters.some((m) => /^Processor: (\d+%|<1%) used/.test(m))) {
      throw new Error(`usage meters: ${meters.join(" | ")}`);
    }
    const chatPart = $$(".usage-view .part").find((p) => text(p.querySelector(".part-name")) === "Chat model");
    if (!/Model \d.*context .*room for [\d,]+ tokens/.test(text(chatPart))) throw new Error(`chat model detail: ${text(chatPart)}`);
    await log(`12u usage | ${meters.join(" | ")} | ${$$(".usage-view .part .part-line").map(text).join(" | ")}`);
    // Larger window: the switch in the header doubles the panel (as far as the
    // display allows), the native window grows with it, and switching back
    // restores both.
    const largeSwitch = $('[aria-label="Larger window"]');
    const panelBox = () => {
      const r = $(".notch").getBoundingClientRect();
      return [Math.round(r.width), Math.round(r.height)];
    };
    const [pw0, ph0] = panelBox();
    const win0 = [window.innerWidth, window.innerHeight];
    largeSwitch.click();
    await sleep(1300);
    const [pw1, ph1] = panelBox();
    const win1 = [window.innerWidth, window.innerHeight];
    if (largeSwitch.getAttribute("aria-pressed") !== "true" || pw1 < pw0 * 1.5 || ph1 <= ph0 || win1[0] <= win0[0] || win1[1] <= win0[1]) {
      throw new Error(`larger window: panel ${pw0}x${ph0} -> ${pw1}x${ph1}, window ${win0} -> ${win1}`);
    }
    if (pw1 > win1[0] || ph1 > win1[1]) throw new Error(`the larger panel ${pw1}x${ph1} does not fit its window ${win1}`);
    largeSwitch.click();
    await sleep(1500);
    const [pw2, ph2] = panelBox();
    const win2 = [window.innerWidth, window.innerHeight];
    if (pw2 !== pw0 || ph2 !== ph0 || win2[0] !== win0[0] || win2[1] !== win0[1]) {
      throw new Error(`back to normal: panel ${pw2}x${ph2} (was ${pw0}x${ph0}), window ${win2} (was ${win0})`);
    }
    await log(`12L larger window | panel ${pw0}x${ph0} -> ${pw1}x${ph1} -> ${pw2}x${ph2} | window ${win0.join("x")} -> ${win1.join("x")} -> ${win2.join("x")}`);

    // Esc leaves it for the main tab; the steps below go on in Settings.
    window.dispatchEvent(new KeyboardEvent("keydown", { key: "Escape" }));
    await waitFor(() => !$(".tab-content.shown .usage-view") && $(".tab-content.shown .ask, .tab-content.shown.fill"), 3000, "Esc to leave Usage");
    $('[aria-label="Settings"]').click();
    await waitFor(() => $(".tab-content.shown .settings"), 3000, "Settings to open again");

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

    // Window style: floating. The window leaves the screen edge, stays where
    // it is put through closing and reopening, and goes back to the edge as a
    // notch. (Still on the main display, docked at the top, panel open.)
    const pickStyle = async (name) => {
      $$(".settings .style-choice")
        .find((b) => text($(".place-name", b)) === name)
        .click();
      await sleep(900);
    };
    const nativeWindow = window.__TAURI__.window.getCurrentWindow();
    const PhysicalPosition = window.__TAURI__.window.PhysicalPosition ?? window.__TAURI__.dpi.PhysicalPosition;
    const dpr = window.devicePixelRatio;
    await pickStyle("Floating");
    if (document.documentElement.dataset.style !== "floating") throw new Error("the style did not switch to floating");
    geo = await log(
      `13s floating, panel open | corners: ${getComputedStyle($(".notch")).borderTopLeftRadius} | ears: ${getComputedStyle($(".ear")).display} | placement choices shown: ${$$(".settings .placement").length}`,
    );
    if (geo[3] === 0) throw new Error(`floating: the window ${geo} is still against the top edge`);
    if (getComputedStyle($(".ear")).display !== "none" || parseFloat(getComputedStyle($(".notch")).borderTopLeftRadius) < 10) {
      throw new Error("floating: the panel still looks docked (ears, or square top corners)");
    }
    if ($$(".settings .placement").length) throw new Error("edge placement is offered for a floating window");
    // Put it somewhere else, as a drag would, and let Loft notice.
    const spot = [Math.round(200 * dpr), Math.round(300 * dpr)];
    await sleep(400);
    await nativeWindow.setPosition(new PhysicalPosition(spot[0], spot[1]));
    await sleep(1500);
    geo = await log("13t floating, moved to 200,300");
    if (geo[2] !== spot[0] || geo[3] !== spot[1]) throw new Error(`floating: the window ${geo} is not where it was put`);
    $('[aria-label="Collapse"]').click();
    await sleep(1100);
    geo = await log(`13u floating, collapsed | pill ${$(".notch").offsetWidth}x${$(".notch").offsetHeight}`);
    // The pill sits under the same top-centre as the panel did.
    if (Math.abs(geo[2] + geo[0] / 2 - (spot[0] + (540 * dpr) / 2)) > 2 || geo[3] !== spot[1]) {
      throw new Error(`floating: the pill ${geo} did not stay where the panel was`);
    }
    $(".pill").click();
    await waitFor(() => $(".notch.expanded"), 5000, "the floating panel to reopen");
    await sleep(600);
    geo = await log("13v floating, reopened");
    if (geo[2] !== spot[0] || geo[3] !== spot[1]) throw new Error(`floating: the panel ${geo} reopened somewhere else`);
    if (!$(".tab-content.shown .settings")) $('[aria-label="Settings"]').click();
    await sleep(400);

    // Living in the tray: nothing on screen while Loft is closed; the tray
    // icon (here: the same call it makes) brings the panel back.
    const traySwitch = () => $('.settings [role="switch"][aria-label="Live in the tray"]');
    if (traySwitch().disabled) {
      await log("13w no tray icon on this desktop: living in the tray not exercised");
    } else {
      traySwitch().click();
      await sleep(600);
      if (traySwitch().getAttribute("aria-checked") !== "true" || document.documentElement.dataset.tray !== "on") throw new Error("living in the tray did not turn on");
      const whileOpen = await nativeWindow.isVisible();
      $('[aria-label="Collapse"]').click();
      await sleep(1400);
      const whenClosed = await nativeWindow.isVisible();
      if (!whileOpen || whenClosed) throw new Error(`tray: visible while open ${whileOpen}, visible when closed ${whenClosed}`);
      await invoke("dev_summon");
      await waitFor(() => $(".notch.expanded"), 5000, "the panel to come back from the tray");
      await sleep(700);
      const summoned = await nativeWindow.isVisible();
      geo = await log(`13w tray | on screen while open: ${whileOpen}, when closed: ${whenClosed}, after the tray icon: ${summoned}`);
      if (!summoned || geo[2] !== spot[0] || geo[3] !== spot[1]) throw new Error(`tray: the panel did not come back where it was: visible ${summoned}, ${geo}`);
      await invoke("dev_summon"); // the icon again closes it
      await sleep(1500);
      if (await nativeWindow.isVisible()) throw new Error("tray: the icon did not close the panel again");
      await invoke("dev_summon");
      await waitFor(() => $(".notch.expanded"), 5000, "the panel to come back a second time");
      await sleep(600);
      if (!$(".tab-content.shown .settings")) $('[aria-label="Settings"]').click();
      await sleep(400);
      traySwitch().click();
      await sleep(600);
      $('[aria-label="Collapse"]').click();
      await sleep(1200);
      const pillBack = (await nativeWindow.isVisible()) && getComputedStyle($(".notch")).opacity === "1";
      await log(`13x tray off again | pill on screen when closed: ${pillBack}`);
      if (!pillBack) throw new Error("with living in the tray off, the pill did not come back");
      $(".pill").click();
      await waitFor(() => $(".notch.expanded"), 5000, "the panel to reopen");
      await sleep(600);
      if (!$(".tab-content.shown .settings")) $('[aria-label="Settings"]').click();
      await sleep(400);
    }

    await pickStyle("Notch");
    geo = await log(`13y notch again | style: ${document.documentElement.dataset.style} | placement choices shown: ${$$(".settings .placement").length}`);
    if (geo[3] !== 0 || Math.abs(geo[2] + geo[0] / 2 - (screen.width * dpr) / 2) > 2 || $$(".settings .placement").length !== 2) {
      throw new Error(`back to the notch: window ${geo} is not docked at the top centre`);
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

    const atLogin = () => $('.settings [role="switch"][aria-label="Launch at login"]');
    atLogin().click();
    await sleep(600);
    await log(`14 launch-at-login in a dev build | on: ${atLogin().getAttribute("aria-checked")} | ${text($(".note", atLogin().closest("section")))}`);
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
