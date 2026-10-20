<!--
  The explorer: the shelf (running the simulation and the model it runs), the
  stage (loops on the lattice, CTCFs edited in place), and a tab per view of
  the run. On narrow screens the shelf becomes a bottom sheet. The tour takes
  the shelf's place, one scene at a time (see tour/).
-->
<script lang="ts">
  import { type Component, onMount } from "svelte";

  import { DEFAULT_PRESET, Run } from "./run.svelte.ts";
  import Shelf from "./Shelf.svelte";
  import Stage from "./Stage.svelte";
  import ThemeToggle from "./ThemeToggle.svelte";
  import TourPanel from "./tour/TourPanel.svelte";
  import { SCENES } from "./tour/tour.ts";
  import Welcome from "./Welcome.svelte";
  import ContactView from "./views/ContactView.svelte";
  import DistanceView from "./views/DistanceView.svelte";
  import EncounterMapView from "./views/EncounterMapView.svelte";
  import EncounterView from "./views/EncounterView.svelte";
  import KymographView from "./views/KymographView.svelte";
  import OccupancyView from "./views/OccupancyView.svelte";

  const run = new Run();

  const VIEWS: { key: string; label: string; view: Component<{ run: Run }> }[] = [
    { key: "kymograph", label: "Kymograph", view: KymographView },
    { key: "occupancy", label: "Occupancy", view: OccupancyView },
    // Distance before the contact map, which is derived from it.
    { key: "distance", label: "Distance", view: DistanceView },
    { key: "contacts", label: "Contact map", view: ContactView },
    { key: "encounters", label: "Encounters", view: EncounterView },
    { key: "encounter-map", label: "Encounter map", view: EncounterMapView },
  ];

  const reducedMotion = matchMedia("(prefers-reduced-motion: reduce)").matches;

  /**
   * The selected view, remembered in the URL hash so a reload or link keeps it
   * (in the tour, the hash is "#tour").
   */
  let selected = $state(VIEWS.some((v) => v.key === location.hash.slice(1)) ? location.hash.slice(1) : VIEWS[0]!.key);
  const current = $derived(VIEWS.find((v) => v.key === selected)!);
  let sheetOpen = $state(false);
  let mode = $state<"explore" | "tour">("explore");

  /** The welcome card: shown until dismissed, which this browser remembers (if it can). */
  const WELCOMED = "lefsim.welcomed";
  let welcome = $state(!remembered());

  function remembered(): boolean {
    try {
      return localStorage.getItem(WELCOMED) === "1";
    } catch {
      return false;
    }
  }

  function dismissWelcome() {
    welcome = false;
    try {
      localStorage.setItem(WELCOMED, "1");
    } catch {
      // Private browsing or blocked storage: it just shows again next time.
    }
  }
  let scene = $state(0);

  function select(key: string) {
    if (!VIEWS.some((v) => v.key === key)) return;
    selected = key;
    run.markers = undefined;
    if (mode === "explore") history.replaceState(null, "", `#${key}`);
  }

  /** Show tour scene `i`: set up its simulation and its view, and play. */
  function go(i: number) {
    scene = Math.max(0, Math.min(SCENES.length - 1, i));
    const s = SCENES[scene]!;
    run.applyPreset(s.preset);
    select(s.view);
    if (!reducedMotion) run.setPlaying(true);
  }

  function startTour() {
    dismissWelcome();
    mode = "tour";
    history.replaceState(null, "", "#tour");
    sheetOpen = true;
    go(0);
  }

  /** Leave the tour for the full app, keeping the current setup. */
  function explore() {
    mode = "explore";
    history.replaceState(null, "", `#${selected}`);
  }

  /** Finish the tour: back to the app's default setup and first view, with the shelf hidden. */
  function finish() {
    run.applyPreset(DEFAULT_PRESET);
    select(VIEWS[0]!.key);
    sheetOpen = false;
    explore();
    if (!reducedMotion) run.setPlaying(true);
  }

  onMount(() => {
    if (location.hash === "#tour") startTour();
    else {
      run.restart();
      // Respect reduced motion: start paused, let the viewer press Play.
      if (!reducedMotion) run.setPlaying(true);
    }
    return () => run.setPlaying(false);
  });
</script>

<div class="app" class:sheet-open={sheetOpen} class:touring={mode === "tour"}>
  <header>
    <a class="brand" href="/">
      <svg viewBox="0 0 24 24" fill="none" stroke-width="2" stroke-linecap="round" aria-hidden="true">
        <path d="M2 19 H22" stroke="var(--muted)" />
        <path d="M4 19 A8 13 0 0 1 20 19" stroke="var(--held)" />
        <path d="M8.5 19 A3.5 7 0 0 1 15.5 19" stroke="var(--extruding)" />
      </svg>
      lefsim
    </a>
    <span class="tagline">Loops. Extruding. In your browser.</span>
    <span class="spacer"></span>
    <span class="seg modes" role="group" aria-label="Mode">
      <button aria-pressed={mode === "explore"} onclick={() => mode === "tour" && explore()}>Explore</button>
      <button aria-pressed={mode === "tour"} onclick={() => mode === "explore" && startTour()}>Take the tour</button>
    </span>
    <nav>
      {#if mode === "explore" && !welcome}
        <button class="help" onclick={() => (welcome = true)} title="Show the introduction" aria-label="Show the introduction"
          >?</button
        >
      {/if}
      <ThemeToggle />
      <a href="https://abdenlab.github.io/lefsim/" target="_blank" rel="noopener">Docs</a>
      <a href="https://github.com/abdenlab/lefsim" target="_blank" rel="noopener">GitHub</a>
    </nav>
  </header>

  <aside class="shelf" id="shelf" aria-label={mode === "tour" ? "Tour" : "Simulation and model"}>
    {#if mode === "tour"}
      <TourPanel {scene} {go} reset={() => go(scene)} {explore} {finish} />
    {:else}
      <Shelf {run} />
    {/if}
  </aside>

  <main>
    <Stage {run} />
    {#if welcome && mode === "explore"}
      <Welcome tour={startTour} close={dismissWelcome} />
    {/if}
    <div class="tabs" role="tablist" aria-label="Views">
      {#each VIEWS as v (v.key)}
        <button role="tab" aria-selected={v.key === selected} onclick={() => select(v.key)}>{v.label}</button>
      {/each}
    </div>
    <div class="view" role="tabpanel" aria-label={current.label}>
      {#key `${selected}:${run.nSites}`}
        <current.view {run} />
      {/key}
    </div>
  </main>

  <div class="sheet-toggle">
    <button class="btn" aria-controls="shelf" aria-expanded={sheetOpen} onclick={() => (sheetOpen = !sheetOpen)}>
      {sheetOpen ? "Close" : mode === "tour" ? "Tour" : "Simulation and model"}
    </button>
  </div>
</div>

<style>
  .app {
    height: 100%;
    display: grid;
    grid-template-rows: auto 1fr;
    grid-template-columns: 290px minmax(0, 1fr);
    grid-template-areas: "top top" "shelf main";
  }

  header {
    grid-area: top;
    display: flex;
    align-items: center;
    gap: 14px;
    flex-wrap: wrap;
    padding: 10px 16px;
    border-bottom: 1px solid var(--rule);
  }

  .brand {
    display: flex;
    align-items: center;
    gap: 8px;
    font-weight: 700;
    font-size: 17px;
    color: var(--ink);
    text-decoration: none;
  }

  .brand svg {
    width: 26px;
    height: 26px;
  }

  .tagline {
    color: var(--muted);
    font-size: 14px;
  }

  .modes button {
    font-size: 14px;
    padding: 4px 12px;
  }

  .spacer {
    flex: 1;
  }

  nav {
    display: flex;
    align-items: center;
    gap: 14px;
    /* Stay on the right when the header wraps on narrow screens. */
    margin-left: auto;
  }

  nav a {
    color: var(--ink-2);
    font-size: 14px;
    text-decoration: none;
  }

  .help {
    width: 22px;
    height: 22px;
    padding: 0;
    border: 1px solid var(--rule);
    border-radius: 50%;
    background: var(--paper);
    color: var(--ink-2);
    font-size: 13px;
    font-weight: 700;
    line-height: 1;
    cursor: pointer;
  }

  .help:hover {
    border-color: var(--muted);
    color: var(--ink);
  }

  nav {
    align-items: center;
  }

  nav a:hover {
    color: var(--ink);
    text-decoration: underline;
  }

  .shelf {
    grid-area: shelf;
    background: var(--shelf);
    border-right: 1px solid var(--rule);
    overflow-y: auto;
    padding: 14px 16px 24px;
    display: flex;
    flex-direction: column;
    gap: 22px;
  }

  main {
    grid-area: main;
    min-width: 0;
    overflow-y: auto;
    display: flex;
    flex-direction: column;
  }

  main > :global(.stage) {
    padding: 8px 16px 4px;
    border-bottom: 1px solid var(--rule);
  }

  .tabs {
    display: flex;
    gap: 2px;
    padding: 0 12px;
    border-bottom: 1px solid var(--rule);
    overflow-x: auto;
    scrollbar-width: none;
    flex: none;
  }

  .tabs button {
    border: 0;
    background: none;
    padding: 10px 12px;
    cursor: pointer;
    color: var(--ink-2);
    white-space: nowrap;
    border-bottom: 2px solid transparent;
    margin-bottom: -1px;
  }

  .tabs button[aria-selected="true"] {
    color: var(--ink);
    border-bottom-color: var(--ink);
    font-weight: 600;
  }

  .view {
    padding: 12px 16px 24px;
    display: flex;
    flex-direction: column;
    gap: 10px;
  }

  .sheet-toggle {
    display: none;
  }

  @media (max-width: 760px) {
    .app {
      grid-template-columns: minmax(0, 1fr);
      grid-template-areas: "top" "main";
    }

    .tagline {
      display: none;
    }

    main {
      padding-bottom: calc(72px + env(safe-area-inset-bottom, 0px));
    }

    .shelf {
      position: fixed;
      left: 0;
      right: 0;
      bottom: 0;
      z-index: 5;
      max-height: 75vh;
      border-right: 0;
      border-top: 1px solid var(--rule);
      border-radius: 14px 14px 0 0;
      box-shadow: 0 -8px 24px rgb(0 0 0 / 0.18);
      padding-bottom: calc(80px + env(safe-area-inset-bottom, 0px));
      transform: translateY(105%);
      transition: transform 0.25s ease;
    }

    .sheet-open .shelf {
      transform: none;
    }

    /* In the tour, keep the scene in view above the text, which scrolls inside the panel. */
    .touring .shelf {
      max-height: 52vh;
      overflow: hidden;
    }

    .sheet-toggle {
      display: flex;
      position: fixed;
      z-index: 6;
      left: 16px;
      right: 16px;
      bottom: calc(12px + env(safe-area-inset-bottom, 0px));
    }

    .sheet-toggle .btn {
      flex: 1;
      padding: 11px;
      font-weight: 600;
      box-shadow: 0 2px 10px rgb(0 0 0 / 0.15);
    }
  }

  @media (prefers-reduced-motion: reduce) {
    .shelf {
      transition: none;
    }
  }
</style>
