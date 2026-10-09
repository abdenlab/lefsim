<!--
  The header's theme switch: one button that cycles light → dark → the OS
  setting. The choice is saved (see `setTheme` in render.ts).
-->
<script lang="ts">
  import { setTheme, type Theme, theme } from "../render.ts";

  let current = $state<Theme>(theme());

  const NEXT: Record<Theme, Theme> = { light: "dark", dark: "system", system: "light" };
  const LABEL: Record<Theme, string> = { light: "Light", dark: "Dark", system: "System" };

  let title = $derived(`Theme: ${LABEL[current]} (switch to ${LABEL[NEXT[current]].toLowerCase()})`);

  function cycle() {
    current = NEXT[current];
    setTheme(current);
  }
</script>

<button class="theme" onclick={cycle} {title} aria-label={title}>
  <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true">
    {#if current === "light"}
      <circle cx="12" cy="12" r="4" />
      <path d="M12 2v2M12 20v2M4.93 4.93l1.41 1.41M17.66 17.66l1.41 1.41M2 12h2M20 12h2M4.93 19.07l1.41-1.41M17.66 6.34l1.41-1.41" />
    {:else if current === "dark"}
      <path d="M21 12.8A9 9 0 1 1 11.2 3a7 7 0 0 0 9.8 9.8z" />
    {:else}
      <rect x="3" y="4" width="18" height="12" rx="2" />
      <path d="M8 20h8M12 16v4" />
    {/if}
  </svg>
</button>

<style>
  .theme {
    display: flex;
    align-items: center;
    justify-content: center;
    width: 26px;
    height: 26px;
    padding: 0;
    border: none;
    border-radius: 6px;
    background: none;
    color: var(--ink-2);
    cursor: pointer;
  }

  .theme:hover {
    color: var(--ink);
    background: var(--shelf);
  }

  svg {
    width: 16px;
    height: 16px;
  }
</style>
