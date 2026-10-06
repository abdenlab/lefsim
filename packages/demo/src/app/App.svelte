<!--
  The explorer: the shelf (running the simulation and the model it runs), the
  stage (loops on the lattice, CTCFs edited in place), and a tab per view of
  the run. On narrow screens the shelf becomes a bottom sheet.
-->
<script lang="ts">
  import { type Component, onMount } from "svelte";

  import { Run } from "./run.svelte.ts";
  import Shelf from "./Shelf.svelte";
  import Stage from "./Stage.svelte";
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
    { key: "contacts", label: "Contact map", view: ContactView },
    { key: "distance", label: "Distance", view: DistanceView },
    { key: "encounters", label: "Encounters", view: EncounterView },
    { key: "encounter-map", label: "Encounter map", view: EncounterMapView },
  ];

  /** The selected view, remembered in the URL hash so a reload or link keeps it. */
  let selected = $state(VIEWS.some((v) => v.key === location.hash.slice(1)) ? location.hash.slice(1) : VIEWS[0]!.key);
  const current = $derived(VIEWS.find((v) => v.key === selected)!);
  let sheetOpen = $state(false);

  function select(key: string) {
    selected = key;
    run.markers = undefined;
    history.replaceState(null, "", `#${key}`);
  }

  onMount(() => {
    run.restart();
    // Respect reduced motion: start paused, let the viewer press Play.
    if (!matchMedia("(prefers-reduced-motion: reduce)").matches) run.setPlaying(true);
    return () => run.setPlaying(false);
  });
</script>

<div class="app" class:sheet-open={sheetOpen}>
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
    <nav>
      <a href="https://abdenlab.github.io/lefsim/" target="_blank" rel="noopener">Docs</a>
      <a href="https://github.com/abdenlab/lefsim" target="_blank" rel="noopener">GitHub</a>
    </nav>
  </header>

  <aside class="shelf" id="shelf" aria-label="Simulation and model">
    <Shelf {run} />
  </aside>

  <main>
    <Stage {run} />
    <div class="tabs" role="tablist" aria-label="Views">
      {#each VIEWS as v (v.key)}
        <button role="tab" aria-selected={v.key === selected} onclick={() => select(v.key)}>{v.label}</button>
      {/each}
    </div>
    <div class="view" role="tabpanel" aria-label={current.label}>
      {#key selected}
        <current.view {run} />
      {/key}
    </div>
  </main>

  <div class="sheet-toggle">
    <button class="btn" aria-controls="shelf" aria-expanded={sheetOpen} onclick={() => (sheetOpen = !sheetOpen)}>
      {sheetOpen ? "Close" : "Simulation and model"}
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

  .spacer {
    flex: 1;
  }

  nav {
    display: flex;
    gap: 14px;
  }

  nav a {
    color: var(--ink-2);
    font-size: 14px;
    text-decoration: none;
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
