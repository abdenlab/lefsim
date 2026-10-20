<!--
  The explorer: the shelf (running the simulation and the model it runs), the
  stage (loops on the lattice, CTCFs edited in place), and a tab per view of
  the run. On narrow screens the shelf becomes a bottom sheet that opens to
  just below the stage, with a handle bar that stays in view when it's hidden.
  The tour takes the shelf's place, one scene at a time (see tour/).
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
  /** The page of the scene's text. */
  let page = $state(0);
  /** Past the last scene's last page: the farewell (tour/outro.md). */
  let outro = $state(false);

  function select(key: string) {
    if (!VIEWS.some((v) => v.key === key)) return;
    selected = key;
    run.markers = undefined;
    if (mode === "explore") history.replaceState(null, "", `#${key}`);
  }

  /** Show tour scene `i` at its first page: set up its simulation and its view, and play. */
  function go(i: number) {
    scene = Math.max(0, Math.min(SCENES.length - 1, i));
    page = 0;
    outro = false;
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

  const nPages = $derived(SCENES[scene]!.pages.length);
  const atStart = $derived(scene === 0 && page === 0);
  const atEnd = $derived(outro);
  /** "Scene 3 of 7", with "· 2/3" when the scene has more than one page. */
  const progress = $derived(
    outro ? "End of the tour" : `Scene ${scene + 1} of ${SCENES.length}${nPages > 1 ? ` · ${page + 1}/${nPages}` : ""}`,
  );

  /** The next page, or the next scene (set up afresh) after a scene's last page, then the farewell, then finish. */
  function next() {
    if (outro) finish();
    else if (page < nPages - 1) page++;
    else if (scene < SCENES.length - 1) go(scene + 1);
    else outro = true;
  }

  /** The previous page, or the previous scene's last page. */
  function back() {
    if (outro) outro = false;
    else if (page > 0) page--;
    else if (scene > 0) {
      go(scene - 1);
      page = SCENES[scene]!.pages.length - 1;
    }
  }
  const shelfLabel = $derived(mode === "tour" ? "Tour" : "Control panel");

  // On phones the sheet opens to the bottom of the stage, which is measured here
  // (as `--sheet-top`), so the loops stay in view while you edit the model.
  let app: HTMLDivElement;
  let header: HTMLElement;
  let sheet: HTMLElement;

  /**
   * Dragging the sheet's handle (phones): the sheet follows the finger, then
   * settles open or hidden by where it was let go and how fast it was moving.
   * A press that barely moves is left to the handle's buttons as a tap.
   */
  const DRAG_SLOP = 6;
  let drag: { y0: number; ty0: number; hidden: number; ty: number; moved: boolean; samples: [number, number][] } | undefined;
  let swallowClick = false;

  /** How far down the sheet sits when hidden: all but its handle (and the home indicator's inset). */
  function hiddenOffset(): number {
    const probe = document.createElement("div");
    probe.style.cssText = "position:fixed;visibility:hidden;height:env(safe-area-inset-bottom, 0px)";
    document.body.append(probe);
    const inset = probe.offsetHeight;
    probe.remove();
    const handle = sheet.querySelector<HTMLElement>(".handle")!;
    return sheet.offsetHeight - handle.offsetHeight - inset;
  }

  function handleDown(e: PointerEvent) {
    swallowClick = false;
    if (!matchMedia("(max-width: 760px)").matches || e.button !== 0) return;
    const ty0 = new DOMMatrix(getComputedStyle(sheet).transform).m42;
    drag = { y0: e.clientY, ty0, hidden: hiddenOffset(), ty: ty0, moved: false, samples: [[e.timeStamp, e.clientY]] };
    // Follow the pointer anywhere, not just over the handle. (Capturing it
    // instead would send a tap's click to the handle, not to the button under
    // the finger.)
    addEventListener("pointermove", handleMove);
    addEventListener("pointerup", handleUp);
    addEventListener("pointercancel", handleUp);
  }

  function handleMove(e: PointerEvent) {
    if (!drag) return;
    const dy = e.clientY - drag.y0;
    if (!drag.moved && Math.abs(dy) < DRAG_SLOP) return;
    drag.moved = true;
    drag.ty = Math.min(drag.hidden, Math.max(0, drag.ty0 + dy));
    drag.samples = [...drag.samples.filter(([t]) => e.timeStamp - t < 100), [e.timeStamp, e.clientY]];
    sheet.style.transition = "none";
    sheet.style.transform = `translateY(${drag.ty}px)`;
  }

  function handleUp(e: PointerEvent) {
    removeEventListener("pointermove", handleMove);
    removeEventListener("pointerup", handleUp);
    removeEventListener("pointercancel", handleUp);
    if (!drag) return;
    const d = drag;
    drag = undefined;
    if (!d.moved) return;
    // A flick (over the last 100 ms) decides by its direction; otherwise, by which end is nearer.
    const recent = d.samples.filter(([t]) => e.timeStamp - t < 100);
    const [t0, y0] = recent[0] ?? [e.timeStamp, e.clientY];
    const velocity = e.timeStamp > t0 ? (e.clientY - y0) / (e.timeStamp - t0) : 0; // px/ms, down is positive
    const open = Math.abs(velocity) > 0.4 ? velocity < 0 : d.ty < d.hidden / 2;
    // Hand back to the stylesheet, which animates from here to the end state.
    sheet.style.transition = "";
    sheet.style.transform = "";
    sheetOpen = open;
    swallowClick = true;
  }

  /** After a drag, a click that follows (from a mouse) isn't a tap on the handle's buttons. */
  function handleClick(e: MouseEvent) {
    if (!swallowClick) return;
    swallowClick = false;
    e.stopPropagation();
    e.preventDefault();
  }

  onMount(() => {
    const stage = app.querySelector<HTMLElement>("main > .stage")!;
    const measure = () => app.style.setProperty("--sheet-top", `${header.offsetHeight + stage.offsetHeight}px`);
    const ro = new ResizeObserver(measure);
    ro.observe(header);
    ro.observe(stage);
    return () => ro.disconnect();
  });

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

<div class="app" class:sheet-open={sheetOpen} class:touring={mode === "tour"} bind:this={app}>
  <header bind:this={header}>
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
      <button aria-pressed={mode === "tour"} onclick={() => mode === "explore" && startTour()}
        ><span class="wide">Take the tour</span><span class="narrow">Tour</span></button
      >
    </span>
    <nav>
      {#if mode === "explore" && !welcome}
        <button class="help" onclick={() => (welcome = true)} title="Show the introduction" aria-label="Show the introduction"
          >?</button
        >
      {/if}
      <ThemeToggle />
      <a href="https://abdenlab.github.io/lefsim/" target="_blank" rel="noopener" aria-label="Docs">
        <svg class="icon" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true"
          ><path d="M4 19.5V5a2 2 0 0 1 2-2h14v15H6.5A2.5 2.5 0 0 0 4 20.5 2.5 2.5 0 0 0 6.5 23H20v-5" /></svg
        ><span class="label">Docs</span>
      </a>
      <a href="https://github.com/abdenlab/lefsim" target="_blank" rel="noopener" aria-label="GitHub">
        <svg class="icon" viewBox="0 0 16 16" fill="currentColor" aria-hidden="true"
          ><path
            d="M8 0C3.58 0 0 3.58 0 8c0 3.54 2.29 6.53 5.47 7.59.4.07.55-.17.55-.38 0-.19-.01-.82-.01-1.49-2.01.37-2.53-.49-2.69-.94-.09-.23-.48-.94-.82-1.13-.28-.15-.68-.52-.01-.53.63-.01 1.08.58 1.23.82.72 1.21 1.87.87 2.33.66.07-.52.28-.87.51-1.07-1.78-.2-3.64-.89-3.64-3.95 0-.87.31-1.59.82-2.15-.08-.2-.36-1.02.08-2.12 0 0 .67-.21 2.2.82.64-.18 1.32-.27 2-.27.68 0 1.36.09 2 .27 1.53-1.04 2.2-.82 2.2-.82.44 1.1.16 1.92.08 2.12.51.56.82 1.27.82 2.15 0 3.07-1.87 3.75-3.65 3.95.29.25.54.73.54 1.48 0 1.07-.01 1.93-.01 2.2 0 .21.15.46.55.38A8.013 8.013 0 0 0 16 8c0-4.42-3.58-8-8-8z"
          /></svg
        ><span class="label">GitHub</span>
      </a>
    </nav>
  </header>

  <aside class="shelf" aria-label={shelfLabel} bind:this={sheet}>
    <!-- Phones only: the sheet's handle, which stays in view while the sheet is hidden. -->
    <div
      class="handle"
      role="presentation"
      onpointerdown={handleDown}
      onclickcapture={handleClick}
    >
      {#if mode === "tour"}
        <button class="step" onclick={back} disabled={atStart} aria-label="Back">
          <svg viewBox="0 0 24 24" aria-hidden="true"><path d="M15 5l-7 7 7 7" /></svg>
        </button>
      {/if}
      <button class="grab" aria-controls="sheet-body" aria-expanded={sheetOpen} onclick={() => (sheetOpen = !sheetOpen)}>
        <span class="title">{mode === "tour" ? progress : shelfLabel}</span>
        <span class="state">{sheetOpen ? "" : "Show"}</span>
        <svg class="chevron" viewBox="0 0 24 24" aria-hidden="true"><path d="M6 15l6-6 6 6" /></svg>
      </button>
      {#if mode === "tour"}
        {#if atEnd}
          <button class="step finish" onclick={finish}>Finish</button>
        {:else}
          <button class="step" onclick={next} aria-label="Next">
            <svg viewBox="0 0 24 24" aria-hidden="true"><path d="M9 5l7 7-7 7" /></svg>
          </button>
        {/if}
      {/if}
    </div>
    <div class="sheet-body" id="sheet-body">
      {#if mode === "tour"}
        <TourPanel {scene} {page} {outro} {progress} {atStart} {atEnd} {go} {next} {back} {explore} />
      {:else}
        <Shelf {run} />
      {/if}
    </div>
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
    display: flex;
    align-items: center;
    color: var(--ink-2);
    font-size: 14px;
    text-decoration: none;
  }

  /* Phones show icons for the links, and a shorter tour button. */
  nav .icon,
  .narrow {
    display: none;
  }

  nav .icon {
    width: 18px;
    height: 18px;
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

  nav a:hover {
    color: var(--ink);
    text-decoration: underline;
  }

  .shelf {
    grid-area: shelf;
    min-height: 0;
    background: var(--shelf);
    border-right: 1px solid var(--rule);
    display: flex;
    flex-direction: column;
  }

  .sheet-body {
    flex: 1;
    min-height: 0;
    overflow-y: auto;
    padding: 14px 16px 24px;
    display: flex;
    flex-direction: column;
    gap: 22px;
  }

  .handle {
    display: none;
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

  @media (max-width: 760px) {
    .app {
      grid-template-columns: minmax(0, 1fr);
      grid-template-areas: "top" "main";
      --handle-height: 52px;
    }

    /* The header on one row: no tagline, icon links, a shorter tour button. */
    header {
      gap: 8px;
      padding: 8px 10px 8px 12px;
    }

    .tagline,
    .spacer,
    .wide,
    nav .label {
      display: none;
    }

    .narrow,
    nav .icon {
      display: inline;
    }

    .modes button {
      padding: 4px 8px;
    }

    nav {
      gap: 10px;
    }

    main {
      padding-bottom: calc(var(--handle-height) + 16px + env(safe-area-inset-bottom, 0px));
    }

    /* The sheet: opens to just below the stage, and hides down to its handle. */
    .shelf {
      position: fixed;
      left: 0;
      right: 0;
      bottom: 0;
      z-index: 5;
      height: calc(100dvh - var(--sheet-top, 50dvh));
      border-right: 0;
      border-top: 1px solid var(--rule);
      border-radius: 14px 14px 0 0;
      box-shadow: 0 -6px 20px rgb(0 0 0 / 0.16);
      transform: translateY(calc(100% - var(--handle-height) - env(safe-area-inset-bottom, 0px)));
      transition: transform 0.25s ease;
    }

    .sheet-open .shelf {
      transform: none;
    }

    .sheet-body {
      padding: 4px 16px calc(16px + env(safe-area-inset-bottom, 0px));
    }

    .handle {
      position: relative;
      touch-action: none;
      user-select: none;
      display: flex;
      align-items: stretch;
      flex: none;
      height: var(--handle-height);
      padding: 0 6px;
      margin-bottom: env(safe-area-inset-bottom, 0px);
    }

    .sheet-open .handle {
      margin-bottom: 0;
    }

    /* The grabber: a small bar centered on the top edge. */
    .handle::before {
      content: "";
      position: absolute;
      top: 6px;
      left: 50%;
      width: 36px;
      height: 4px;
      margin-left: -18px;
      border-radius: 2px;
      background: var(--rule);
    }

    .handle button {
      border: 0;
      background: none;
      color: var(--ink);
      font: inherit;
      cursor: pointer;
    }

    .grab {
      flex: 1;
      display: flex;
      align-items: center;
      gap: 8px;
      padding: 6px 10px 0;
      min-width: 0;
    }

    .grab .title {
      font-weight: 600;
      font-size: 15px;
      white-space: nowrap;
      overflow: hidden;
      text-overflow: ellipsis;
    }

    .grab .state {
      margin-left: auto;
      font-size: 13px;
      color: var(--muted);
    }

    .handle svg {
      width: 20px;
      height: 20px;
      fill: none;
      stroke: currentColor;
      stroke-width: 2;
      stroke-linecap: round;
      stroke-linejoin: round;
    }

    .chevron {
      color: var(--muted);
      transition: transform 0.25s ease;
    }

    .sheet-open .chevron {
      transform: rotate(180deg);
    }

    .step {
      display: grid;
      place-items: center;
      width: 44px;
      padding: 6px 0 0;
    }

    .step:disabled {
      color: var(--rule);
      cursor: default;
    }

    .step.finish {
      width: auto;
      padding: 6px 10px 0;
      font-weight: 600;
      color: var(--extruding);
    }
  }

  @media (prefers-reduced-motion: reduce) {
    .shelf,
    .chevron {
      transition: none;
    }
  }
</style>
