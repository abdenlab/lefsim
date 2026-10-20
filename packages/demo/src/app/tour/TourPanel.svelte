<!--
  The tour: one scene at a time, in place of the shelf. Each scene sets up the
  simulation (see tour.ts); the stage and views stay live, so you can play with it,
  and "Explore this setup" leaves the tour with it.
-->
<script lang="ts">
  import { OUTRO, SCENES } from "./tour.ts";

  let {
    scene,
    page,
    outro,
    progress,
    atStart,
    atEnd,
    go,
    next,
    back,
    explore,
  }: {
    /** The current scene's index. */
    scene: number;
    /** The current page of its text. */
    page: number;
    /** Showing the farewell after the last scene, which has no title. */
    outro: boolean;
    /** Where the tour is, e.g. "Scene 3 of 7 · 2/3". */
    progress: string;
    /** At the tour's first page (nothing to go back to). */
    atStart: boolean;
    /** At the tour's last page (Next finishes). */
    atEnd: boolean;
    /** Show scene `i`, from its first page. */
    go: (i: number) => void;
    /** The next page or scene, or finish the tour. */
    next: () => void;
    /** The previous page or scene. */
    back: () => void;
    /** Leave the tour, keeping the current setup. */
    explore: () => void;
  } = $props();

  const current = $derived(SCENES[scene]!);
</script>

<div class="tour">
  <div class="progress">
    <span class="count">{progress}</span>
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

  <!-- Above the text, so Back and Next stay put from scene to scene. -->
  <div class="nav">
    <button class="btn" onclick={back} disabled={atStart}>Back</button>
    <button class="btn primary" onclick={next}>{atEnd ? "Finish" : "Next"}</button>
  </div>

  <!-- The scenes' Markdown is ours, bundled at build time. -->
  {#if outro}
    <div class="text">{@html OUTRO}</div>
  {:else}
    <h2>{current.title}</h2>
    <div class="text">{@html current.pages[page]}</div>
  {/if}

  <div class="more">
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
    justify-content: center;
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

  /* On phones the tour sits in a bottom sheet whose handle shows the scene
     count, Back and Next; the text scrolls on its own, so the title and the
     links stay in view. */
  @media (max-width: 760px) {
    .tour {
      flex: 1;
      min-height: 0;
      gap: 10px;
    }

    .count,
    .nav {
      display: none;
    }

    h2 {
      font-size: 19px;
    }

    .text {
      flex: 1 1 auto;
      min-height: 0;
      overflow-y: auto;
    }
  }
</style>
