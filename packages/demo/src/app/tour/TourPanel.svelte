<!--
  The tour: one scene at a time, in place of the shelf. Each scene sets up its
  scene (see tour.ts); the stage and views stay live, so you can play with it,
  and "Reset this scene" sets it up again.
-->
<script lang="ts">
  import { SCENES } from "./tour.ts";

  let {
    scene,
    go,
    reset,
    explore,
  }: {
    /** The current scene's index. */
    scene: number;
    /** Show scene `i`. */
    go: (i: number) => void;
    /** Set the current scene up again. */
    reset: () => void;
    /** Leave the tour, keeping the current setup. */
    explore: () => void;
  } = $props();

  const current = $derived(SCENES[scene]!);
  const last = $derived(scene === SCENES.length - 1);
</script>

<div class="tour">
  <div class="progress">
    <span class="count">Scene {scene + 1} of {SCENES.length}</span>
    <span class="dots">
      {#each SCENES as s, i (s.id)}
        <button
          class="dot"
          class:on={i === scene}
          aria-label="Scene {i + 1}: {s.title}"
          aria-current={i === scene ? "step" : undefined}
          onclick={() => go(i)}
        ></button>
      {/each}
    </span>
  </div>

  <h2>{current.title}</h2>
  <!-- The scenes' Markdown is ours, bundled at build time. -->
  <div class="text">{@html current.html}</div>

  <div class="nav">
    <button class="btn" onclick={() => go(scene - 1)} disabled={scene === 0}>Back</button>
    <button class="btn primary" onclick={() => (last ? explore() : go(scene + 1))}>{last ? "Finish" : "Next"}</button>
  </div>
  <div class="more">
    <button class="link" onclick={reset}>Reset this scene</button>
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
