<script module lang="ts">
  export type MenuItem =
    | { id?: string; text: string; action: () => void; enabled?: boolean; checked?: boolean }
    | { item: "Separator" };

  let open = $state<{ x: number; y: number; items: MenuItem[] } | null>(null);
  // What had the focus before the menu opened; it gets it back when the menu closes.
  let opener: HTMLElement | null = null;

  /**
   * Shows a right-click menu at the pointer. It is drawn by the page rather than by Windows:
   * a native menu opened from the webview sometimes ignored the mouse until Esc.
   */
  export function popupMenu(event: MouseEvent, { items }: { items: MenuItem[] }) {
    event.preventDefault();
    opener = document.activeElement instanceof HTMLElement ? document.activeElement : null;
    open = { x: event.clientX, y: event.clientY, items };
  }
</script>

<script lang="ts">
  let menuEl = $state<HTMLDivElement>();
  let width = $state(0);
  let height = $state(0);
  let innerWidth = $state(0);
  let innerHeight = $state(0);
  // Opens to the lower right of the pointer, or flips where the window ends, as Windows does.
  const left = $derived(open && (open.x + width > innerWidth ? Math.max(0, open.x - width) : open.x));
  const top = $derived(open && (open.y + height > innerHeight ? Math.max(0, open.y - height) : open.y));
  const checks = $derived(open?.items.some((item) => "checked" in item && item.checked !== undefined));

  $effect(() => {
    if (open) menuEl?.querySelector<HTMLButtonElement>("button:not(:disabled)")?.focus();
  });

  /** Closes the menu from the keyboard or by a choice, and gives the focus back. */
  function dismiss() {
    open = null;
    opener?.focus();
  }

  function choose(item: MenuItem) {
    dismiss();
    if ("action" in item) item.action();
  }

  // A press anywhere else closes the menu and still does what it does there, so a right-click
  // elsewhere opens that place's menu.
  function pressed(event: PointerEvent) {
    if (open && !menuEl?.contains(event.target as Node)) open = null;
  }

  function keys(event: KeyboardEvent) {
    if (!open || !menuEl) return;
    if (event.key === "Escape" || event.key === "Tab") {
      event.preventDefault();
      dismiss();
      return;
    }
    const buttons = [...menuEl.querySelectorAll<HTMLButtonElement>("button:not(:disabled)")];
    const at = buttons.indexOf(document.activeElement as HTMLButtonElement);
    const keys: Record<string, number> = { ArrowDown: at + 1, ArrowUp: at - 1, Home: 0, End: buttons.length - 1 };
    if (!(event.key in keys)) return;
    event.preventDefault();
    buttons[(keys[event.key] + buttons.length) % buttons.length]?.focus();
  }
</script>

<svelte:window
  bind:innerWidth
  bind:innerHeight
  onpointerdowncapture={pressed}
  onkeydown={keys}
  onblur={() => (open = null)}
  onresize={() => (open = null)}
/>

{#if open}
  <div
    class="menu"
    role="menu"
    tabindex="-1"
    bind:this={menuEl}
    bind:clientWidth={width}
    bind:clientHeight={height}
    style:left="{left}px"
    style:top="{top}px"
    oncontextmenu={(event) => event.preventDefault()}
  >
    {#each open.items as item, i (i)}
      {#if "item" in item}
        <div class="separator" role="separator"></div>
      {:else}
        <button
          role={item.checked === undefined ? "menuitem" : "menuitemcheckbox"}
          aria-checked={item.checked}
          disabled={item.enabled === false}
          onclick={() => choose(item)}
        >
          {#if checks}<span class="check">{item.checked ? "✓" : ""}</span>{/if}{item.text}
        </button>
      {/if}
    {/each}
  </div>
{/if}

<style>
  .menu {
    position: fixed;
    z-index: 100;
    min-width: 160px;
    padding: 4px;
    border: 1px solid var(--line);
    border-radius: 6px;
    background: var(--panel);
    box-shadow: 0 6px 20px rgba(0, 0, 0, 0.45);
    font-size: 13px;
  }
  button {
    display: flex;
    width: 100%;
    padding: 5px 24px 5px 10px;
    border: 0;
    border-radius: 4px;
    background: none;
    color: var(--text);
    font: inherit;
    text-align: left;
    white-space: nowrap;
  }
  button:hover:not(:disabled),
  button:focus-visible {
    outline: none;
    background: var(--line);
  }
  button:disabled {
    color: var(--muted);
    opacity: 0.6;
  }
  .check {
    width: 20px;
    flex: none;
  }
  .separator {
    height: 1px;
    margin: 4px 6px;
    background: var(--line);
  }
</style>
