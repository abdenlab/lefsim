<!--
  A first-visit introduction, as a modal dialog centered in the window, with
  the simulation running behind it. Dismissed with
  either button, Escape or a click outside, and remembered in this browser;
  the header's "?" brings it back.
-->
<script lang="ts">
  import { onMount } from "svelte";

  let { tour, close }: { tour: () => void; close: () => void } = $props();

  let dialog: HTMLDialogElement;

  onMount(() => {
    dialog.showModal();
    return () => dialog.close();
  });
</script>

<dialog
  bind:this={dialog}
  class="welcome"
  aria-labelledby="welcome-title"
  onclose={close}
  onclick={(e) => {
    // A click on the backdrop (the dialog itself, outside its content) closes it.
    if (e.target === dialog) dialog.close();
  }}
>
  <div class="content">
    <h2 id="welcome-title">Loop extrusion, live!</h2>
    <p>
      Use this live simulator to explore and learn about how the process of <b>loop extrusion</b> organizes the genome.
    </p>
    <p>
      A long stretch of DNA (a chromatin fiber) is modeled as a 1D lattice. Each arc on the stage above represents a loop
      on that fiber held together by a loop-extruding factor (LEF) called <i>cohesin</i>. It appears
      <span class="key extruding">blue</span> while the loop grows, and <span class="key held">orange</span> when 
      an oriented extrusion barrier called <i>CTCF</i> (depicted by triangles) holds one or both of the LEF's two "legs".
    </p>
    <p>
      Learn more by browsing the view tabs below the stage and tuning the model parameters on the left, or by taking the tour.
    </p>
    <div class="actions">
      <button class="btn primary" onclick={tour}>Take the tour</button>
      <button class="btn" onclick={() => dialog.close()}>Start exploring</button>
    </div>
  </div>
</dialog>

<style>
  .welcome {
    margin: auto;
    width: min(560px, calc(100vw - 32px));
    padding: 0;
    border: 1px solid var(--rule);
    border-radius: 14px;
    background: var(--paper);
    color: var(--ink);
    box-shadow: 0 18px 50px rgb(0 0 0 / 0.22);
  }

  .welcome::backdrop {
    background: rgb(0 0 0 / 0.12);
  }

  .content {
    padding: 18px 20px 16px;
  }

  h2 {
    margin: 0 0 8px;
    font-size: 19px;
    text-wrap: balance;
    text-align: center;
  }

  p {
    margin: 0 0 6px;
    color: var(--ink-2);
    font-size: 15px;
  }

  .actions {
    display: flex;
    justify-content: center;
    gap: 8px;
    flex-wrap: wrap;
    margin-top: 16px;
  }

  @media (prefers-reduced-motion: no-preference) {
    .welcome[open] {
      animation: rise 0.25s ease-out;
    }

    @keyframes rise {
      from {
        opacity: 0;
        transform: translateY(12px);
      }
    }
  }

  /* The loop colors, as they appear on the stage. */
  .key {
    font-weight: 600;
    white-space: nowrap;
  }

  .key::before {
    content: "";
    display: inline-block;
    width: 14px;
    height: 3px;
    border-radius: 2px;
    margin-right: 5px;
    vertical-align: middle;
    background: currentColor;
  }

  .key.extruding {
    color: var(--extruding);
  }

  .key.held {
    color: var(--held);
  }

</style>
