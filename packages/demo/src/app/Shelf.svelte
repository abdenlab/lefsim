<!-- The shelf: running the simulation and the model it runs. CTCFs are edited on the stage. -->
<script lang="ts">
  import type { Rates } from "lefsim";

  import { fmt, fmtCompact } from "../render.ts";
  import { MAX_SITES, MIN_SITES, type Run } from "./run.svelte.ts";

  let { run }: { run: Run } = $props();

  let fileInput: HTMLInputElement;
  /** Why the last file couldn't be opened, or what was opened. */
  let fileMessage = $state<{ text: string; error: boolean } | undefined>(undefined);
  let dragging = $state(false);

  const RATE_FIELDS: { key: keyof Rates; label: string }[] = [
    { key: "step", label: "Step" },
    { key: "respawn", label: "Unload" },
    { key: "respawnBlocked", label: "Unload, blocked" },
    { key: "respawnCaptured", label: "Unload, held" },
  ];

  function setSize(input: HTMLInputElement) {
    const n = Math.round(Number(input.value));
    if (!Number.isFinite(n) || n === run.nSites) {
      input.value = String(run.nSites);
      return;
    }
    run.resize(n);
    input.value = String(run.nSites);
  }

  /** Download the current state as a model file. */
  function save() {
    const blob = new Blob([run.toJson()], { type: "application/json" });
    const a = document.createElement("a");
    a.href = URL.createObjectURL(blob);
    a.download = `lefsim-model-${run.nSites}-sites.json`;
    a.click();
    URL.revokeObjectURL(a.href);
  }

  async function open(file: File | undefined) {
    if (!file) return;
    try {
      run.open(await file.text());
      fileMessage = { text: `Opened ${file.name}.`, error: false };
    } catch (e) {
      fileMessage = { text: `Couldn't open ${file.name}: ${e instanceof Error ? e.message : String(e)}`, error: true };
    }
  }

  function setRate(key: keyof Rates, input: HTMLInputElement) {
    const v = Number(input.value);
    if (input.value.trim() !== "" && Number.isFinite(v) && v > 0) run.setRates({ [key]: v });
    else input.value = String(run.rates[key]);
  }
</script>

<!-- Model files can be dropped anywhere on the shelf. -->
<div
  class="body"
  role="region"
  aria-label="Simulation and model"
  ondragover={(e) => {
    if (!e.dataTransfer?.types.includes("Files")) return;
    e.preventDefault();
    dragging = true;
  }}
  ondragleave={(e) => {
    if (!e.currentTarget.contains(e.relatedTarget as Node | null)) dragging = false;
  }}
  ondrop={(e) => {
    if (!e.dataTransfer?.files.length) return;
    e.preventDefault();
    e.stopPropagation();
    dragging = false;
    void open(e.dataTransfer.files[0]);
  }}
>
  <section>
    <h2>Run</h2>
    <div class="buttons">
      <button
        class="btn primary play"
        onclick={() => run.setPlaying(!run.playing)}
        aria-label={run.playing ? "Pause" : "Play"}
        title={run.playing ? "Pause" : "Play"}
      >
        <svg viewBox="0 0 16 16" aria-hidden="true">
          {#if run.playing}
            <rect x="3.5" y="2.5" width="3" height="11" rx="0.75" /><rect x="9.5" y="2.5" width="3" height="11" rx="0.75" />
          {:else}
            <path d="M4.5 2.6v10.8a.6.6 0 0 0 .9.5l8.6-5.4a.6.6 0 0 0 0-1L5.4 2.1a.6.6 0 0 0-.9.5Z" />
          {/if}
        </svg>
      </button>
      <button class="btn" onclick={() => run.restart()} title="Start the run over: the loop extruders placed afresh with the seed, at time 0">Reset</button>
      <label class="seed" title="The seed for placing the loop extruders and running them. A new seed resets the run.">
        <span>Seed</span>
        <input
          class="mono"
          type="number"
          min="0"
          step="1"
          value={run.seed}
          onchange={(e) => {
            run.seed = Math.max(0, Math.floor(Number(e.currentTarget.value) || 0));
            run.restart();
          }}
        />
      </label>
    </div>
    <label class="row">
      <span>Simulation speed</span>
      <output class="mono">{fmt.format(run.speed)} / s</output>
      <input type="range" min="1" max="4" step="0.05" bind:value={run.speedLog} aria-label="Speed, simulation time units per second" />
    </label>
    <dl class="readout mono">
      <dt>clock</dt>
      <dd>{fmt.format(run.time)}</dd>
      <dt>events</dt>
      <dd>{run.playing ? `${fmtCompact.format(run.eventsPerSecond)} / s` : "–"}</dd>
      <dt>loops held</dt>
      <dd>{run.held} of {run.frame?.held.length ?? 0}</dd>
    </dl>
    {#if run.playing && run.behind}
      <p class="behind">Falling behind: playing slower than the speed asks.</p>
    {/if}
  </section>

  <section>
    <div class="heading">
      <h2>Model</h2>
      <span class="icons">
        <button
          type="button"
          class="icon-btn"
          onclick={() => fileInput.click()}
          title="Open a model file (JSON), or drop one on the page"
          aria-label="Open model"
        >
          <svg viewBox="0 0 16 16" aria-hidden="true"><path d="M2 4.5V12a1 1 0 0 0 1 1h10a1 1 0 0 0 1-1V6.5a1 1 0 0 0-1-1H8L6.5 3.5H3a1 1 0 0 0-1 1Z" /></svg>
        </button>
        <button
          type="button"
          class="icon-btn"
          onclick={save}
          title="Save the current state as a model file (JSON), with the extruders where they are now"
          aria-label="Save model"
        >
          <svg viewBox="0 0 16 16" aria-hidden="true"><path d="M8 2.5v7.5M4.8 7 8 10.2 11.2 7M3 12.5h10" /></svg>
        </button>
        {#if run.loaded}
          <button
            type="button"
            class="icon-btn"
            onclick={() => run.reloadOpened()}
            title="Go back to the model you opened"
            aria-label="Reload opened model"
          >
            <svg viewBox="0 0 16 16" aria-hidden="true"><path d="M3 8a5 5 0 1 0 1.6-3.7M3 2.5v2.3h2.3" /></svg>
          </button>
        {/if}
      </span>
    </div>
    <input
      bind:this={fileInput}
      type="file"
      accept=".json,application/json"
      hidden
      onchange={(e) => {
        void open(e.currentTarget.files?.[0]);
        e.currentTarget.value = "";
      }}
    />
    {#if fileMessage}
      <p class="hint" class:error={fileMessage.error}>{fileMessage.text}</p>
    {/if}
    <label class="row" title="Changing it starts over: the default CTCFs, and extruders at the default density.">
      <span>Lattice sites</span>
      <input
        class="mono size"
        type="number"
        min={MIN_SITES}
        max={MAX_SITES}
        step="100"
        value={run.nSites}
        onchange={(e) => setSize(e.currentTarget)}
      />
    </label>
    <div class="row">
      <span class="with-icons">
        CTCF sites
        <button
          type="button"
          class="icon-btn"
          onclick={() => run.clearCtcfs()}
          disabled={run.ctcfs.length === 0}
          title="Remove all CTCFs"
          aria-label="Remove all CTCFs"
        >
          <svg viewBox="0 0 16 16" aria-hidden="true"
            ><path d="M3 4.5h10M6.5 4.5V3h3v1.5M4.5 4.5l.6 8.5h5.8l.6-8.5M6.8 7v3.8M9.2 7v3.8" /></svg
          >
        </button>
      </span>
      <span class="stepper">
        <button
          type="button"
          class="icon-btn"
          onclick={() => run.removeLastCtcf()}
          disabled={run.ctcfs.length === 0}
          title="Remove the most recently added CTCF"
          aria-label="Remove a CTCF"><svg viewBox="0 0 16 16" aria-hidden="true"><path d="M4 8h8" /></svg></button
        >
        <output class="mono">{run.ctcfs.length}</output>
        <button
          type="button"
          class="icon-btn"
          onclick={() => run.addCtcfEvenly()}
          title="Add a CTCF in the middle of the widest stretch without one, pairing with its neighbor into a domain"
          aria-label="Add a CTCF"><svg viewBox="0 0 16 16" aria-hidden="true"><path d="M4 8h8M8 4v8" /></svg></button
        >
      </span>
    </div>
    <div class="row">
      <span class="with-lock">
        CTCF strength
        <button
          type="button"
          class="icon-btn lock"
          aria-pressed={run.strengthLocked}
          title={run.strengthLocked
            ? "One strength for every CTCF. Unlock to set each CTCF's strength on the stage."
            : "Each CTCF has its own strength. Lock to set them all to one value."}
          aria-label={run.strengthLocked ? "Strength shared by all CTCFs: unlock" : "Strength set per CTCF: lock"}
          onclick={() => run.setStrengthLocked(!run.strengthLocked)}
        >
          <svg viewBox="0 0 16 16" aria-hidden="true">
            <!-- Locked: the shackle closes into the body. Unlocked: it swings up, open on the right. -->
            <rect x="2.5" y="7.3" width="11" height="7" rx="1.4" />
            <path d={run.strengthLocked ? "M4.8 7.3V4.8a3.2 3.2 0 0 1 6.4 0v2.5" : "M4.8 7.3V4.8a3.2 3.2 0 0 1 6.3-.8"} />
          </svg>
        </button>
      </span>
      <output class="mono">
        {#if run.strengthLocked}
          {run.strength.toFixed(2)}
        {:else}
          {@const [lo, hi] = run.strengthRange}
          {lo === hi ? lo.toFixed(2) : `${lo.toFixed(2)}–${hi.toFixed(2)}`}
        {/if}
      </output>
      {#if run.strengthLocked}
        <input
          type="range"
          min="0"
          max="1"
          step="0.05"
          value={run.strength}
          onchange={(e) => run.setStrength(Number(e.currentTarget.value))}
          aria-label="CTCF strength: the probability that a landing leg interacts"
        />
      {:else}
        <p class="hint span">Click a CTCF on the stage to set its strength.</p>
      {/if}
    </div>
    <label class="row">
      <span>Loop extruders</span>
      <output class="mono">{run.nLefs}</output>
      <input
        type="range"
        min="0"
        max={run.maxLefs}
        value={run.nLefs}
        oninput={(e) => run.setLefCount(Number(e.currentTarget.value))}
      />
    </label>
    <h3>Rates</h3>
    <div class="rates">
      {#each RATE_FIELDS as f (f.key)}
        <label for="rate-{f.key}">{f.label}</label>
        <input
          id="rate-{f.key}"
          class="mono"
          type="number"
          min="0"
          step="any"
          value={run.rates[f.key]}
          onchange={(e) => setRate(f.key, e.currentTarget)}
        />
      {/each}
    </div>
    <p class="hint">
      Rates are events per time unit.
    </p>
  </section>

  {#if dragging}
    <div class="drop">Drop a model file to open it</div>
  {/if}
</div>

<!-- Elsewhere on the page, a dropped file is ignored, rather than replacing the app. -->
<svelte:window
  ondragover={(e) => {
    if (e.dataTransfer?.types.includes("Files")) e.preventDefault();
  }}
  ondrop={(e) => {
    if (e.dataTransfer?.types.includes("Files")) e.preventDefault();
  }}
/>

<style>
  section {
    display: flex;
    flex-direction: column;
    gap: 10px;
  }

  h2 {
    margin: 0;
    font-size: 13px;
    font-weight: 700;
    letter-spacing: 0.06em;
    text-transform: uppercase;
    color: var(--ink);
  }

  .buttons {
    display: flex;
    flex-wrap: wrap;
    align-items: center;
    gap: 8px;
  }

  .play {
    display: inline-grid;
    place-items: center;
    width: 52px;
    height: 34px;
    padding: 0;
  }

  .play svg {
    width: 16px;
    height: 16px;
    fill: currentColor;
  }

  .row {
    display: grid;
    grid-template-columns: 1fr auto;
    align-items: center;
    gap: 4px 10px;
    font-size: 14px;
    color: var(--ink-2);
  }

  .row output {
    color: var(--ink);
    font-size: 13px;
  }

  .row input[type="range"] {
    grid-column: 1 / -1;
    width: 100%;
  }

  .seed {
    display: flex;
    align-items: center;
    gap: 6px;
    margin-left: auto;
    font-size: 14px;
    color: var(--ink-2);
  }

  .size {
    width: 96px;
    text-align: right;
  }

  .with-lock {
    display: inline-flex;
    align-items: center;
    gap: 6px;
    white-space: nowrap;
  }

  .heading {
    display: flex;
    align-items: center;
    justify-content: space-between;
  }

  .icons {
    display: flex;
    gap: 4px;
  }

  /* Small square icon buttons: the lock, delete, open, save. */
  .icon-btn {
    display: inline-grid;
    place-items: center;
    width: 22px;
    height: 22px;
    padding: 0;
    border: 1px solid var(--rule);
    border-radius: 6px;
    background: var(--paper);
    color: var(--ink-2);
    cursor: pointer;
  }

  .icon-btn:hover:not(:disabled) {
    border-color: var(--muted);
    color: var(--ink);
  }

  .icon-btn:disabled {
    opacity: 0.4;
    cursor: default;
  }

  .icon-btn svg {
    width: 13px;
    height: 13px;
    fill: none;
    stroke: currentColor;
    stroke-width: 1.5;
    stroke-linecap: round;
    stroke-linejoin: round;
  }

  .lock[aria-pressed="false"] {
    color: var(--extruding);
    border-color: var(--extruding);
  }

  .span {
    grid-column: 1 / -1;
  }

  .error {
    color: var(--warn);
  }

  .body {
    position: relative;
    display: flex;
    flex-direction: column;
    gap: 22px;
    min-height: 100%;
  }

  .drop {
    position: absolute;
    inset: -6px;
    z-index: 20;
    display: grid;
    place-items: center;
    border: 2px dashed var(--extruding);
    border-radius: 14px;
    background: color-mix(in srgb, var(--paper) 85%, transparent);
    padding: 16px;
    text-align: center;
    font-size: 16px;
    font-weight: 600;
    pointer-events: none;
  }

  .seed input {
    width: 52px;
    text-align: right;
  }

  h3 {
    margin: 4px 0 0;
    font-size: 14px;
    font-weight: 600;
    color: var(--ink-2);
  }

  .with-icons {
    display: inline-flex;
    align-items: center;
    gap: 6px;
    white-space: nowrap;
  }

  .stepper {
    display: inline-flex;
    align-items: center;
    gap: 6px;
  }

  .stepper output {
    min-width: 2ch;
    text-align: center;
  }

  .readout {
    display: grid;
    grid-template-columns: auto 1fr;
    gap: 2px 12px;
    margin: 0;
    font-size: 13px;
    color: var(--ink-2);
  }

  .readout dd {
    margin: 0;
    text-align: right;
    color: var(--ink);
  }

  .behind {
    margin: 0;
    font-size: 13px;
    color: var(--warn);
  }

  .hint {
    margin: 0;
  }

  .rates {
    display: grid;
    grid-template-columns: 1fr auto;
    gap: 6px 10px;
    align-items: center;
    font-size: 14px;
    color: var(--ink-2);
  }

  .rates input {
    width: 90px;
    text-align: right;
  }
</style>
