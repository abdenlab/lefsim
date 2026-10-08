<!--
  The tour: one step at a time, in place of the shelf. Each step sets up its
  scene (see tour.ts); the stage and views stay live, so you can play with it,
  and "Reset this step" sets it up again.
-->
<script lang="ts">
  import { STEPS } from "./tour.ts";

  let {
    step,
    go,
    reset,
    explore,
  }: {
    /** The current step's index. */
    step: number;
    /** Show step `i`. */
    go: (i: number) => void;
    /** Set the current step up again. */
    reset: () => void;
    /** Leave the tour, keeping the current setup. */
    explore: () => void;
  } = $props();

  const current = $derived(STEPS[step]!);
  const last = $derived(step === STEPS.length - 1);
</script>

<div class="tour">
  <div class="progress">
    <span class="count">Step {step + 1} of {STEPS.length}</span>
    <span class="dots">
      {#each STEPS as s, i (s.id)}
        <button
          class="dot"
          class:on={i === step}
          aria-label="Step {i + 1}: {s.title}"
          aria-current={i === step ? "step" : undefined}
          onclick={() => go(i)}
        ></button>
      {/each}
    </span>
  </div>

  <h2>{current.title}</h2>
  <!-- The steps' Markdown is ours, bundled at build time. -->
  <div class="text">{@html current.html}</div>

  <div class="nav">
    <button class="btn" onclick={() => go(step - 1)} disabled={step === 0}>Back</button>
    <button class="btn primary" onclick={() => (last ? explore() : go(step + 1))}>{last ? "Finish" : "Next"}</button>
  </div>
  <div class="more">
    <button class="link" onclick={reset}>Reset this step</button>
    <button class="link" onclick={explore}>Explore this setup</button>
  </div>
</div>

<style>
  .tour {
    display: flex;
    flex-direction: column;
    gap: 14px;
  }

  .progress {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 10px;
  }

  .count {
    font-size: 13px;
    color: var(--muted);
  }

  .dots {
    display: flex;
    gap: 6px;
  }

  .dot {
    width: 9px;
    height: 9px;
    padding: 0;
    border: 0;
    border-radius: 50%;
    background: var(--rule);
    cursor: pointer;
  }

  .dot.on {
    background: var(--extruding);
  }

  h2 {
    margin: 0;
    font-size: 21px;
    line-height: 1.25;
    text-wrap: balance;
  }

  .text {
    color: var(--ink-2);
    font-size: 15px;
    line-height: 1.55;
  }

  .text :global(p) {
    margin: 0 0 0.8em;
  }

  .text :global(p:last-child) {
    margin-bottom: 0;
  }

  .text :global(strong) {
    color: var(--ink);
  }

  .text :global(a) {
    color: var(--extruding);
  }

  .nav {
    display: flex;
    gap: 8px;
  }

  .nav .btn {
    flex: 1;
  }

  .more {
    display: flex;
    justify-content: space-between;
    gap: 8px;
    flex-wrap: wrap;
  }

  .link {
    padding: 0;
    border: 0;
    background: none;
    color: var(--extruding);
    font-size: 14px;
    cursor: pointer;
  }

  .link:hover {
    text-decoration: underline;
  }

  /* On phones the tour sits in a bottom sheet: the text scrolls on its own,
     so the title, Back and Next, and the links stay in view. */
  @media (max-width: 760px) {
    .tour {
      flex: 1;
      min-height: 0;
    }

    .text {
      flex: 1 1 auto;
      min-height: 0;
      overflow-y: auto;
    }
  }
</style>
