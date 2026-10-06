<!-- The shelf: running the simulation and the model it runs. CTCFs are edited on the stage. -->
<script lang="ts">
  import type { Rates } from "lefsim";

  import { fmt, fmtCompact } from "../render.ts";
  import { MAX_LEFS, type Run } from "./run.svelte.ts";

  let { run }: { run: Run } = $props();

  const RATE_FIELDS: { key: keyof Rates; label: string }[] = [
    { key: "step", label: "Step" },
    { key: "respawn", label: "Unload" },
    { key: "respawnBlocked", label: "Unload, blocked" },
    { key: "respawnCaptured", label: "Unload, held" },
  ];

  function setRate(key: keyof Rates, input: HTMLInputElement) {
    const v = Number(input.value);
    if (input.value.trim() !== "" && Number.isFinite(v) && v > 0) run.setRates({ [key]: v });
    else input.value = String(run.rates[key]);
  }
</script>

<section>
  <h2>Run</h2>
  <div class="buttons">
    <button class="btn primary play" onclick={() => run.setPlaying(!run.playing)}>{run.playing ? "Pause" : "Play"}</button>
    <button class="btn" onclick={() => run.restart()} title="Place the loop extruders afresh, with the seed">Restart</button>
    <label class="seed" title="The seed for placing the loop extruders and running them. A new seed restarts the run.">
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
    <span>Speed</span>
    <output class="mono">{fmt.format(run.speed)} / s</output>
    <input type="range" min="1" max="4" step="0.05" bind:value={run.speedLog} aria-label="Speed, simulation time units per second" />
  </label>
  <dl class="readout mono">
    <dt>time</dt>
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
  <h2>Model</h2>
  <label class="row">
    <span>Loop extruders</span>
    <output class="mono">{run.nLefs}</output>
    <input
      type="range"
      min="0"
      max="200"
      value={Math.min(run.nLefs, 200)}
      oninput={(e) => run.setLefCount(Number(e.currentTarget.value))}
    />
  </label>
  <div class="row">
    <span class="with-info">
      CTCF sites
      <button type="button" class="info" aria-label="How to edit CTCFs" aria-describedby="ctcf-help">i</button>
      <span class="tip" id="ctcf-help" role="tooltip">
        CTCFs sit in the two lanes under the loops, one per direction. Click a lane to add one facing that way, drag
        one to move it, or drag it out of the lanes to remove it.
      </span>
    </span>
    <output class="mono">{run.ctcfs.length}</output>
  </div>
  <label class="row">
    <span>CTCF strength</span>
    <output class="mono">{run.strength.toFixed(2)}</output>
    <input
      type="range"
      min="0"
      max="1"
      step="0.05"
      value={run.strength}
      onchange={(e) => run.setStrength(Number(e.currentTarget.value))}
      aria-label="CTCF strength: the probability that a landing leg interacts"
    />
  </label>
  <button class="btn small" onclick={() => run.clearCtcfs()} disabled={run.ctcfs.length === 0}>Remove all CTCFs</button>
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
  <p class="hint">Up to {MAX_LEFS} extruders on {fmt.format(3000)} sites. Rates are events per time unit.</p>
</section>

<style>
  section {
    display: flex;
    flex-direction: column;
    gap: 10px;
  }

  h2 {
    margin: 0;
    font-size: 12px;
    font-weight: 700;
    letter-spacing: 0.06em;
    text-transform: uppercase;
    color: var(--muted);
  }

  .buttons {
    display: flex;
    flex-wrap: wrap;
    align-items: center;
    gap: 8px;
  }

  .play {
    min-width: 76px;
  }

  .small {
    align-self: flex-start;
    font-size: 13px;
    padding: 4px 10px;
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

  .with-info {
    position: relative;
    display: inline-flex;
    align-items: center;
    gap: 6px;
  }

  .info {
    padding: 0;
    background: none;
    display: inline-grid;
    place-items: center;
    width: 16px;
    height: 16px;
    border-radius: 50%;
    border: 1px solid var(--muted);
    color: var(--muted);
    font-size: 11px;
    font-weight: 700;
    cursor: help;
  }

  .tip {
    position: absolute;
    left: 0;
    top: calc(100% + 6px);
    z-index: 4;
    width: min(250px, 70vw);
    padding: 8px 10px;
    border-radius: 8px;
    background: var(--ink);
    color: var(--paper);
    font-size: 13px;
    line-height: 1.4;
    visibility: hidden;
  }

  .with-info:hover .tip,
  .info:focus-visible + .tip,
  .info:focus + .tip {
    visibility: visible;
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
