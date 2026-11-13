<script lang="ts">
  import { onMount } from "svelte";
  import {
    cancelEngineInstall,
    getEngineLog,
    getInference,
    installEngine,
    onEngineInstall,
    onInferenceStatus,
    removeEngine,
    retryEngines,
    setEngineOverride,
    setInferenceSettings,
    tryEngine,
  } from "../api";
  import { ui } from "../state.svelte";
  import type {
    Backend,
    EngineInstallProgress,
    EngineName,
    EngineStatus,
    EngineTrial,
    InferenceOverview,
    InferenceProfile,
    PowerSettings,
    Target,
    TargetOption,
  } from "../types";
  import Benchmark from "./Benchmark.svelte";
  import Icon from "./Icon.svelte";
  import UsagePanel from "./Usage.svelte";

  // Which engine (llama.cpp build) and which device run the models, separately
  // for mains power and for battery. Loft recommends; the user decides.
  let overview = $state<InferenceOverview | null>(null);
  let error = $state("");
  let working = $state(false);
  /** The option picked for "Try another engine now", before it is started. */
  let tryKey = $state("");
  let log = $state<{ engine: EngineName; lines: string[] } | null>(null);
  /** What happened the last time an engine was tried. */
  let trial = $state<EngineTrial | null>(null);
  /** The engine being tried right now, in words. */
  let trying = $state("");
  /** The optional engine being downloaded, if one is. */
  let install = $state<EngineInstallProgress | null>(null);

  const AUTO = "auto";
  const key = (t?: Target | null) => (t ? `${t.backend}|${t.device ?? ""}` : AUTO);
  const optionFor = (k: string) => overview?.options.find((o) => key(o.target) === k);
  /** The target behind a dropdown value, as plain data (not a reactive proxy). */
  const targetFor = (k: string): Target | null => {
    const option = optionFor(k);
    return option ? $state.snapshot(option.target) : null;
  };

  async function load() {
    try {
      overview = await getInference();
    } catch (e) {
      error = String(e);
    }
  }

  onMount(() => {
    let unsub = () => {};
    let unsubInstall = () => {};
    onInferenceStatus((status) => {
      if (overview) overview.status = status;
    }).then((un) => (unsub = un));
    onEngineInstall((p) => {
      if (!p.done) install = p;
    }).then((un) => (unsubInstall = un));
    return () => {
      unsub();
      unsubInstall();
    };
  });

  // Read afresh each time Settings is opened (devices come and go).
  $effect(() => {
    if (ui.tab === "settings") load();
  });

  async function run(action: () => Promise<InferenceOverview>) {
    error = "";
    working = true;
    try {
      overview = await action();
    } catch (e) {
      error = String(e);
    } finally {
      working = false;
    }
  }

  /** Save one changed setting. */
  function save(change: (settings: PowerSettings) => void) {
    if (!overview) return;
    const settings: PowerSettings = $state.snapshot(overview.settings);
    change(settings);
    overview.settings = settings; // show the choice at once
    run(() => setInferenceSettings(settings));
  }

  function choose(profile: "pluggedIn" | "onBattery", k: string) {
    save((s) => {
      s[profile] = k === AUTO ? { ...s[profile], mode: "automatic" } : { ...s[profile], mode: "manual", target: targetFor(k) ?? s[profile].target };
    });
  }

  function chooseEmbed(profile: "pluggedIn" | "onBattery", k: string) {
    save((s) => {
      s[profile] = { ...s[profile], embedTarget: k === AUTO ? null : targetFor(k) };
    });
  }

  const profileKey = (p: InferenceProfile) => (p.mode === "manual" && p.target ? key(p.target) : AUTO);
  /** "Intel Iris Xe Graphics (OpenVINO, experimental)". */
  const optionLabel = (o: TargetOption) =>
    !o.experimental || !o.available ? o.label : o.label.endsWith(")") ? `${o.label.slice(0, -1)}, experimental)` : `${o.label} (experimental)`;
  /** A device name without the trademark marks and OpenVINO's tags. */
  const deviceName = (name: string) => name.replace(/\((R|TM|iGPU|dGPU)\)/gi, "").replace(/\s+/g, " ").trim();
  const stateWord = (e: EngineStatus) =>
    e.pending ? "Switch pending" : { ready: "Running", loading: "Starting…", error: "Stopped", stopped: "Not running" }[e.state];

  const status = $derived(overview?.status);
  const powerWord = $derived(
    !status ? "" : status.powerSource === "battery" ? "On battery" : status.powerSource === "ac" ? "Plugged in" : "Power source unknown",
  );
  const profileWord = $derived(
    !status
      ? ""
      : status.profile === "override"
        ? "temporary engine in use"
        : status.profile === "onBattery"
          ? "using the battery choice"
          : overview?.settings.autoSwitch
            ? "using the plugged-in choice"
            : "always using the plugged-in choice",
  );
  const tryable = $derived((overview?.options ?? []).filter((o) => o.available));

  const BACKEND_NAME: Record<Backend, string> = { cuda: "CUDA", vulkan: "Vulkan", openvino: "OpenVINO", metal: "Metal", cpu: "CPU" };
  const megabytes = (bytes: number) => `${Math.round(bytes / 1e6)} MB`;
  const downloadFor = (b: Backend) => overview?.downloads.find((d) => d.backend === b);

  /** Make `target` the saved choice for a power state (a recommendation was accepted). */
  function useTarget(profile: "pluggedIn" | "onBattery", target: Target) {
    save((s) => {
      s[profile] = { ...s[profile], mode: "manual", target };
    });
  }

  /**
   * Run the chat model on `target` now, check where it really is, and ask
   * it a test question. Temporary: the saved choices are not touched.
   */
  async function tryNow(target: Target | null, label: string) {
    if (!target) return;
    error = "";
    trial = null;
    trying = label;
    working = true;
    try {
      const done = await tryEngine(target);
      trial = done.trial;
      overview = done.overview;
    } catch (e) {
      error = String(e);
    } finally {
      trying = "";
      working = false;
    }
  }

  function restoreProfile() {
    trial = null;
    run(() => setEngineOverride(null));
  }

  async function installNow(backend: Backend) {
    error = "";
    install = { backend, stage: "downloading", received: 0, total: downloadFor(backend)?.sizeBytes ?? 0, done: false };
    try {
      overview = await installEngine(backend);
    } catch (e) {
      error = String(e);
    } finally {
      install = null;
    }
  }

  // The NPU is reached through OpenVINO. It is offered here whether or not
  // "automatic" would ever choose it.
  const openvino = $derived(overview?.inventory.backends.find((b) => b.backend === "openvino"));
  const npuOption = $derived(
    overview?.options.find((o) => o.available && o.target.backend === "openvino" && openvino?.devices.find((d) => d.key === o.target.device)?.kind === "npu"),
  );
  const npuHardware = $derived(overview?.inventory.hardware.find((h) => h.kind === "npu"));
  const seconds = (ms?: number | null) => (ms == null ? "—" : `${(ms / 1000).toFixed(1)} s`);

  async function showLog(engine: EngineName) {
    if (log?.engine === engine) {
      log = null;
      return;
    }
    log = { engine, lines: await getEngineLog(engine).catch((e) => [String(e)]) };
  }
</script>

{#snippet engineRow(name: string, e: EngineStatus)}
  <div class="engine" class:bad={e.state === "error"}>
    <div class="engine-line">
      <span class="engine-name">{name}</span>
      <span class="engine-what">{e.active ?? (e.state === "error" ? "Not running" : e.requested)}</span>
      <span class="engine-state state-{e.pending ? 'pending' : e.state}">{stateWord(e)}</span>
    </div>
    {#if e.model}<div class="engine-sub">Model: {e.model}</div>{/if}
    {#if e.active && e.verified}
      <div class="engine-sub" title={e.detail ?? ""}>
        <Icon name="check" size={11} stroke={2.6} /> Confirmed from the engine's own log{e.detail ? `: ${e.detail}` : ""}
      </div>
    {/if}
    {#if e.fallbackUsed}
      <div class="engine-sub warn">Requested {e.requested}. Fell back because: {e.fallbackReason}</div>
    {:else if e.active && e.requestedTarget && e.active !== e.requested}
      <div class="engine-sub">Requested {e.requested}</div>
    {/if}
    {#if e.pending}<div class="engine-sub warn">The switch to {e.requested} is waiting for work in progress to finish.</div>{/if}
    {#each e.notes as note}<div class="engine-sub">{note}</div>{/each}
    {#if e.error}<div class="engine-sub bad-text">{e.error}</div>{/if}
    {#if e.hint && (e.error || e.fallbackUsed)}<div class="engine-sub">{e.hint}</div>{/if}
  </div>
{/snippet}

{#snippet installing(p: EngineInstallProgress)}
  <div class="install">
    <div class="bar" role="progressbar" aria-valuemin={0} aria-valuemax={100} aria-valuenow={p.total ? Math.round((p.received / p.total) * 100) : 0}>
      <span style="width: {p.total ? Math.round((p.received / p.total) * 100) : 0}%"></span>
    </div>
    <span class="install-text">
      {p.stage === "unpacking" ? "Unpacking…" : p.stage === "installing" ? "Installing…" : `Downloading ${megabytes(p.received)} of ${megabytes(p.total)}`}
    </span>
    <button class="small-btn" onclick={() => cancelEngineInstall()}>Cancel</button>
  </div>
{/snippet}

{#snippet picker(id: string, label: string, value: string, onpick: (k: string) => void, autoLabel: string)}
  <label class="field-label" for={id}>{label}</label>
  <div class="select">
    <select {id} {value} disabled={working} onchange={(e) => onpick(e.currentTarget.value)}>
      <option value={AUTO}>{autoLabel}</option>
      {#each overview?.options ?? [] as o (key(o.target))}
        <option value={key(o.target)} disabled={!o.available && key(o.target) !== value} title={o.reason ?? ""}>{optionLabel(o)}</option>
      {/each}
    </select>
    <span class="select-arrow"><Icon name="chevron-up" size={14} /></span>
  </div>
{/snippet}

<div class="performance">
  <div class="section-label">Performance &amp; battery</div>

  {#if !overview}
    <div class="muted">{error || "Looking for engines and devices…"}</div>
  {:else if status}
    <!-- What is running right now -->
    <div class="status card">
      <div class="power">
        <span class="power-dot {status.powerSource}"></span>
        <span class="power-word">{powerWord}</span>
        <span class="power-profile">· {profileWord}</span>
        {#if status.switching}<span class="spinner" aria-label="Switching"></span>{/if}
      </div>
      {@render engineRow("Chat", status.chat)}
      {@render engineRow("Embeddings", status.embed)}
      {#if status.chat.error || status.embed.error || status.chat.fallbackUsed}
        <button class="small-btn" disabled={working} onclick={() => run(retryEngines)} title="Look for engines and devices again, and try the chosen engine again">
          <Icon name="refresh" size={12} stroke={2.2} /> Retry
        </button>
      {/if}
    </div>

    <UsagePanel />

    <!-- The saved choices -->
    {@render picker("perf-plugged", "When plugged in", profileKey(overview.settings.pluggedIn), (k) => choose("pluggedIn", k), "Automatic — performance (recommended)")}
    {@render picker("perf-battery", "When running on battery", profileKey(overview.settings.onBattery), (k) => choose("onBattery", k), "Automatic — battery saving (recommended)")}
    <p class="help">
      Automatic on battery keeps off the discrete graphics card. That is a rule of thumb, until Test performance (below) has been run on battery on this computer.
    </p>

    <button class="toggle" class:on={overview.settings.autoSwitch} role="switch" aria-checked={overview.settings.autoSwitch} disabled={working} onclick={() => save((s) => (s.autoSwitch = !s.autoSwitch))}>
      <span class="toggle-text">
        <span class="toggle-title">Switch when the power source changes</span>
        <span class="toggle-hint">Off: the plugged-in choice is used all the time.</span>
      </span>
      <span class="tick">{#if overview.settings.autoSwitch}<Icon name="check" size={11} stroke={3.4} />{/if}</span>
    </button>
    <button class="toggle" class:on={overview.settings.allowFallback} role="switch" aria-checked={overview.settings.allowFallback} disabled={working} onclick={() => save((s) => (s.allowFallback = !s.allowFallback))}>
      <span class="toggle-text">
        <span class="toggle-title">Allow fallback to another engine</span>
        <span class="toggle-hint">Off: if the chosen engine cannot run the model, Loft stops and says so.</span>
      </span>
      <span class="tick">{#if overview.settings.allowFallback}<Icon name="check" size={11} stroke={3.4} />{/if}</span>
    </button>

    <!-- A temporary experiment, outside the saved choices -->
    <div class="try card" class:active={!!status.overrideTarget}>
      <div class="try-title">Try another engine now</div>
      {#if status.overrideTarget}
        <p class="try-text">
          Temporary: the chat model is on <strong>{optionFor(key(status.overrideTarget))?.label ?? status.chat.requested}</strong> until you end this,
          whatever the power source. Your plugged-in and battery choices are unchanged.
        </p>
        <button class="small-btn primary" disabled={working} onclick={restoreProfile}>Restore power profile</button>
      {:else}
        <p class="try-text">Runs the chat model on any installed engine until you end it. It does not change the choices above.</p>
        <div class="try-row">
          <div class="select">
            <select aria-label="Engine to try" bind:value={tryKey} disabled={working}>
              <option value="">Choose an engine…</option>
              {#each tryable as o (key(o.target))}
                <option value={key(o.target)}>{optionLabel(o)}</option>
              {/each}
            </select>
            <span class="select-arrow"><Icon name="chevron-up" size={14} /></span>
          </div>
          <button class="small-btn primary" disabled={working || !tryKey} onclick={() => tryNow(targetFor(tryKey), optionFor(tryKey)?.label ?? "")}>Try</button>
        </div>
      {/if}
      {#if trying}
        <div class="trial"><div class="trial-head"><span class="spinner inline" aria-hidden="true"></span> Starting {trying} and asking it a test question…</div></div>
      {:else if trial}
        <div class="trial" class:failed={!trial.ok}>
          {#if trial.ok}
            <div class="trial-head"><Icon name="check" size={12} stroke={2.8} /> {trial.requested} answered a test question</div>
            {#if trial.detail}<div class="trial-sub">Confirmed from the engine's own log: {trial.detail}</div>{/if}
            <div class="trial-sub">
              {trial.tokensPerSecond != null ? `${trial.tokensPerSecond >= 10 ? trial.tokensPerSecond.toFixed(0) : trial.tokensPerSecond.toFixed(1)} tokens/s · ` : ""}first
              token after {seconds(trial.firstTokenMs)} · {trial.answerTokens} tokens in {seconds(trial.totalMs)}
            </div>
            <div class="trial-answer">{trial.answer}</div>
          {:else}
            <div class="trial-head bad-text">{trial.pending ? `${trial.requested}: switch pending` : `${trial.requested} could not be used`}</div>
            {#if trial.problem}<div class="trial-sub">{trial.problem}</div>{/if}
            {#if trial.hint}<div class="trial-sub">{trial.hint}</div>{/if}
            {#if trial.fallbackUsed && trial.active}<div class="trial-sub warn">The chat model is running on {trial.active} instead.</div>{/if}
          {/if}
          {#if trial.note}<div class="trial-sub">{trial.note}</div>{/if}
        </div>
      {/if}
    </div>

    <!-- The NPU: always offered where OpenVINO exists, never chosen automatically -->
    {#if openvino}
      <div class="try card npu">
        <div class="try-title">Try NPU now <span class="badge">Experimental</span></div>
        {#if install?.backend === "openvino"}
          {@render installing(install)}
        {:else if openvino.status === "notInstalled"}
          <p class="try-text">
            An NPU is used through the OpenVINO engine, which is an optional download.
            {#if npuHardware}
              This computer reports an NPU: <strong>{npuHardware.name}</strong>.
            {:else}
              This computer does not report an NPU (they are part of Intel Core Ultra processors), so with the engine installed you could try
              OpenVINO on the Intel processor or graphics, but not on an NPU.
            {/if}
          </p>
          {#if downloadFor("openvino")}
            <button class="small-btn primary" disabled={working || !!install} onclick={() => installNow("openvino")}>
              <Icon name="download" size={12} stroke={2.2} /> Install the OpenVINO engine ({megabytes(downloadFor("openvino")?.sizeBytes ?? 0)})
            </button>
          {/if}
        {:else if openvino.status !== "available"}
          <p class="try-text">{openvino.detail ?? "The OpenVINO engine cannot be used on this computer."}</p>
        {:else if npuOption}
          <p class="try-text">
            Runs the chat model on <strong>{npuOption.label}</strong> now, whatever the power source, checks that it really is on the NPU, and asks it
            a short test question. Your plugged-in and battery choices are not changed. Not every model runs on an NPU; if this one cannot, Loft
            says why.
          </p>
          <button class="small-btn primary" disabled={working} onclick={() => tryNow($state.snapshot(npuOption.target), npuOption.label)}>Try NPU now</button>
        {:else}
          <p class="try-text">
            The OpenVINO engine is installed, but it found no NPU.
            {#if npuHardware}
              This computer reports <strong>{npuHardware.name}</strong>: install or update the Intel NPU driver, then choose "Look for engines again"
              under Advanced.
            {:else}
              This computer does not report one (they are part of Intel Core Ultra processors).
            {/if}
            OpenVINO on the processor or graphics can be tried with "Try another engine now".
          </p>
        {/if}
      </div>
    {/if}

    <Benchmark disabled={working || !!install} onuse={useTarget} ondone={load} />

    {#if error}<div class="note">{error}</div>{/if}

    <details class="advanced">
      <summary>Advanced engine controls</summary>

      <p class="help">The embedding model follows the chat engine unless you choose otherwise here. Changing the device does not rebuild the index.</p>
      {@render picker("perf-embed-plugged", "Embeddings when plugged in", key(overview.settings.pluggedIn.embedTarget), (k) => chooseEmbed("pluggedIn", k), "Same as the chat model")}
      {@render picker("perf-embed-battery", "Embeddings on battery", key(overview.settings.onBattery.embedTarget), (k) => chooseEmbed("onBattery", k), "Same as the chat model")}

      <div class="field-label">Engines (llama.cpp {overview.inventory.revision})</div>
      <div class="engines">
        {#each overview.inventory.backends as b (b.backend)}
          {@const d = downloadFor(b.backend)}
          <div class="backend">
            <span class="backend-name">{BACKEND_NAME[b.backend]}</span>
            <span class="backend-status s-{b.status}">
              {{ available: "Available", notInstalled: "Not installed", hardwareUnavailable: "Hardware unavailable", driverMissing: "Driver missing", failed: "Failed" }[b.status]}
            </span>
            {#if b.experimental}<span class="badge">Experimental</span>{/if}
            <span class="backend-detail">
              {b.devices.map((dev) => `${deviceName(dev.name)}${dev.memoryMib ? ` (${(dev.memoryMib / 1024).toFixed(1)} GB)` : ""}`).join(", ") || b.detail || (b.backend === "cpu" ? deviceName(overview.inventory.cpu) : "")}
            </span>
            {#if d}
              {#if d.note && !d.installed && b.status === "notInstalled"}<span class="backend-detail">{d.note}</span>{/if}
              <span class="backend-actions">
                {#if install?.backend === b.backend}
                  {@render installing(install)}
                {:else if d.installed}
                  <button class="small-btn" disabled={working || !!install} onclick={() => run(() => removeEngine(b.backend))} title="Delete this engine's files; it can be installed again">
                    <Icon name="trash" size={12} stroke={2.2} /> Remove ({megabytes(d.sizeBytes)})
                  </button>
                {:else if b.status === "notInstalled" && !d.blocked}
                  <button class="small-btn" disabled={working || !!install} onclick={() => installNow(b.backend)}>
                    <Icon name="download" size={12} stroke={2.2} /> Install ({megabytes(d.sizeBytes)})
                  </button>
                {/if}
              </span>
            {/if}
          </div>
        {/each}
      </div>

      {#if overview.inventory.hardware.length}
        <div class="field-label">Devices this computer reports</div>
        <div class="engines">
          {#each overview.inventory.hardware as h, i (i)}
            <div class="backend hardware">
              <span class="hardware-name">{deviceName(h.name)}</span>
              <span class="backend-status">{h.kind === "npu" ? "NPU" : h.kind === "gpu" ? "Graphics" : "Processor"} · driver {h.driver ?? "version unknown"}</span>
            </div>
          {/each}
        </div>
      {/if}

      <div class="field-label">Diagnostics</div>
      <div class="diag">
        <button class="small-btn" onclick={() => showLog("chat")}>Chat engine log</button>
        <button class="small-btn" onclick={() => showLog("embed")}>Embeddings engine log</button>
        <button class="small-btn" disabled={working} onclick={() => run(retryEngines)}>Look for engines again</button>
      </div>
      {#if log}<pre class="log scroll">{log.lines.join("\n") || "(nothing logged)"}</pre>{/if}
    </details>
  {/if}
</div>

<style>
  .performance {
    display: flex;
    flex-direction: column;
    gap: 8px;
  }
  .section-label {
    margin: 4px 0 0 2px;
  }
  .muted,
  .help {
    font-size: 11.5px;
    line-height: 1.4;
    color: var(--text-faint);
  }
  .help {
    margin: -2px 2px 0;
  }
  .note {
    font-size: 11.5px;
    line-height: 1.4;
    color: var(--warn);
    padding: 0 4px;
  }

  /* What is running now */
  .status {
    display: flex;
    flex-direction: column;
    gap: 8px;
    padding: 10px 12px 11px 14px;
  }
  .power {
    display: flex;
    align-items: center;
    gap: 7px;
    font-size: 12px;
  }
  .power-dot {
    width: 7px;
    height: 7px;
    border-radius: 50%;
    background: var(--text-faint);
  }
  .power-dot.ac {
    background: var(--good);
  }
  .power-dot.battery {
    background: var(--warn);
  }
  .power-word {
    font-weight: 600;
    color: var(--text);
  }
  .power-profile {
    color: var(--text-faint);
  }
  .spinner {
    width: 11px;
    height: 11px;
    margin-left: auto;
    border-radius: 50%;
    border: 2px solid var(--field-bg-hover);
    border-top-color: var(--accent);
    animation: spin 0.8s linear infinite;
  }
  @keyframes spin {
    to { transform: rotate(360deg); }
  }
  .engine {
    display: flex;
    flex-direction: column;
    gap: 2px;
    padding-top: 7px;
    border-top: 1px solid var(--notch-border);
  }
  .engine-line {
    display: flex;
    align-items: baseline;
    gap: 8px;
    font-size: 12.5px;
  }
  .engine-name {
    flex: none;
    width: 78px;
    color: var(--text-faint);
  }
  .engine-what {
    flex: 1;
    min-width: 0;
    font-weight: 600;
    color: var(--text);
    overflow-wrap: anywhere;
  }
  .engine-state {
    flex: none;
    font-size: 11px;
    color: var(--text-dim);
  }
  .engine-state.state-ready {
    color: var(--good);
  }
  .engine-state.state-error {
    color: var(--bad);
  }
  .engine-state.state-pending,
  .engine-state.state-loading {
    color: var(--warn);
  }
  .engine-sub {
    display: flex;
    gap: 5px;
    align-items: baseline;
    margin-left: 86px;
    font-size: 11px;
    line-height: 1.4;
    color: var(--text-faint);
  }
  .engine-sub.warn {
    color: var(--warn);
  }
  .bad-text {
    color: var(--bad);
  }

  /* Choices */
  .field-label {
    font-size: 12px;
    font-weight: 500;
    color: var(--text-dim);
    margin: 4px 2px 0;
  }
  .select {
    position: relative;
    flex: 1;
    min-width: 0;
  }
  select {
    -webkit-appearance: none;
    appearance: none;
    width: 100%;
    padding: 8px 34px 8px 13px;
    border-radius: 12px;
    border: 1px solid var(--border-strong);
    background: var(--field-bg);
    color: var(--text);
    font-family: inherit;
    font-size: 12.5px;
    text-overflow: ellipsis;
    cursor: pointer;
  }
  select:disabled {
    opacity: 0.6;
    cursor: default;
  }
  select:focus-visible {
    outline: 2px solid color-mix(in srgb, var(--accent) 70%, transparent);
    outline-offset: 2px;
  }
  option {
    background: var(--notch);
    color: var(--text);
  }
  .select-arrow {
    position: absolute;
    right: 11px;
    top: 50%;
    display: grid;
    color: var(--text-dim);
    transform: translateY(-50%) rotate(180deg);
    pointer-events: none;
  }
  .toggle {
    display: flex;
    align-items: center;
    gap: 12px;
    padding: 5px 2px;
    text-align: left;
  }
  .toggle-text {
    flex: 1;
    display: flex;
    flex-direction: column;
    gap: 1px;
    min-width: 0;
  }
  .toggle-title {
    font-size: 13px;
    font-weight: 600;
    color: var(--text);
  }
  .toggle-hint {
    font-size: 11.5px;
    color: var(--text-faint);
  }
  .tick {
    flex: none;
    display: grid;
    place-items: center;
    width: 18px;
    height: 18px;
    border-radius: 50%;
    border: 1.5px solid var(--text-faint);
    color: var(--notch);
  }
  .toggle.on .tick {
    border-color: var(--text);
    background: var(--text);
  }

  /* Try another engine now */
  .try {
    display: flex;
    flex-direction: column;
    gap: 7px;
    padding: 10px 12px 11px 14px;
    margin-top: 2px;
  }
  .try.active {
    border-color: color-mix(in srgb, var(--warn) 55%, transparent);
    background: color-mix(in srgb, var(--warn) 9%, transparent);
  }
  .try-title {
    font-size: 13px;
    font-weight: 600;
  }
  .try-text {
    font-size: 11.5px;
    line-height: 1.45;
    color: var(--text-dim);
  }
  .try-text strong {
    color: var(--text);
    font-weight: 600;
  }
  .try-row {
    display: flex;
    gap: 8px;
    align-items: center;
  }
  .small-btn {
    align-self: flex-start;
    flex: none;
    display: inline-flex;
    align-items: center;
    gap: 5px;
    font-size: 12px;
    font-weight: 600;
    padding: 6px 11px;
    border-radius: 9px;
    color: var(--text);
    background: var(--field-bg-hover);
    transition: background 0.14s ease;
  }
  .small-btn:hover:not(:disabled) {
    background: var(--raised-hover);
  }
  .small-btn:disabled {
    opacity: 0.5;
    cursor: default;
  }
  .small-btn.primary {
    background: var(--accent);
    color: #fff;
  }
  .small-btn.primary:hover:not(:disabled) {
    background: var(--accent);
    opacity: 0.88;
  }

  .try-title .badge {
    margin-left: 4px;
    vertical-align: 1px;
  }
  .trial {
    display: flex;
    flex-direction: column;
    gap: 3px;
    padding-top: 8px;
    border-top: 1px solid var(--notch-border);
  }
  .trial-head {
    display: flex;
    align-items: center;
    gap: 6px;
    font-size: 12px;
    font-weight: 600;
    color: var(--good);
  }
  .trial.failed .trial-head,
  .trial-head.bad-text {
    color: var(--bad);
  }
  .trial-sub {
    font-size: 11px;
    line-height: 1.45;
    color: var(--text-faint);
  }
  .trial-sub.warn {
    color: var(--warn);
  }
  .trial-answer {
    margin-top: 3px;
    padding: 7px 10px;
    border-radius: 9px;
    background: var(--field-bg);
    font-size: 11.5px;
    line-height: 1.45;
    color: var(--text-dim);
    user-select: text;
  }
  .spinner.inline {
    margin-left: 0;
    flex: none;
  }

  /* Installing an optional engine */
  .install {
    display: flex;
    flex-wrap: wrap;
    align-items: center;
    gap: 6px 10px;
  }
  .bar {
    flex-basis: 100%;
    height: 5px;
    border-radius: 3px;
    background: var(--field-bg-hover);
    overflow: hidden;
  }
  .bar span {
    display: block;
    height: 100%;
    border-radius: 3px;
    background: var(--accent);
    transition: width 0.2s ease;
  }
  .install-text {
    flex: 1;
    min-width: 0;
    font-size: 11.5px;
    color: var(--text-dim);
  }
  .backend-actions {
    flex-basis: 100%;
    margin-left: 72px;
  }
  .backend-actions:empty {
    display: none;
  }

  /* Advanced */
  .advanced {
    margin-top: 2px;
  }
  .advanced summary {
    font-size: 12.5px;
    font-weight: 600;
    color: var(--text-dim);
    padding: 5px 2px;
    cursor: pointer;
  }
  .advanced summary:hover {
    color: var(--text);
  }
  .advanced[open] {
    display: flex;
    flex-direction: column;
    gap: 8px;
  }
  .advanced .help {
    margin: 2px 2px 0;
  }
  .engines {
    display: flex;
    flex-direction: column;
    gap: 5px;
  }
  .backend {
    display: flex;
    flex-wrap: wrap;
    align-items: baseline;
    gap: 4px 8px;
    font-size: 12px;
  }
  .backend-name {
    width: 64px;
    font-weight: 600;
  }
  .backend-status {
    color: var(--text-dim);
  }
  .backend-status.s-available {
    color: var(--good);
  }
  .backend-status.s-failed,
  .backend-status.s-driverMissing {
    color: var(--warn);
  }
  .badge {
    font-size: 9.5px;
    font-weight: 600;
    color: var(--text-dim);
    background: var(--field-bg-hover);
    border-radius: 6px;
    padding: 1px 6px;
  }
  .hardware-name {
    font-weight: 600;
  }
  .backend-detail {
    flex-basis: 100%;
    margin-left: 72px;
    font-size: 11px;
    line-height: 1.4;
    color: var(--text-faint);
  }
  .diag {
    display: flex;
    flex-wrap: wrap;
    gap: 6px;
  }
  .log {
    max-height: 150px;
    padding: 8px 10px;
    border-radius: 10px;
    background: var(--field-bg);
    font: 10.5px/1.45 ui-monospace, "Cascadia Mono", Consolas, monospace;
    color: var(--text-dim);
    white-space: pre;
    overflow-x: auto;
    user-select: text;
  }
</style>
