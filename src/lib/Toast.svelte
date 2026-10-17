<script module lang="ts">
  let shown = $state<{ text: string; error: boolean; id: number } | null>(null);
  let timer: ReturnType<typeof setTimeout> | undefined;
  let next = 0;

  /**
   * Shows a message at the bottom of the window, over everything, in place of any earlier
   * one. It goes by itself after a few seconds; an error stays until it is closed or
   * another message takes its place, so that it is not missed.
   */
  export function showToast(text: string, error = false) {
    clearTimeout(timer);
    shown = { text, error, id: next++ };
    if (!error) timer = setTimeout(() => (shown = null), 6000);
  }
</script>

<script lang="ts">
  function close() {
    clearTimeout(timer);
    shown = null;
  }
</script>

<!-- The live regions are always there, so that a message put into one is read out; a
     region that appears together with its message often is not. -->
{#each [false, true] as error}
  <div role={error ? "alert" : "status"}>
    {#if shown && shown.error === error}
      {#key shown.id}
        <div class="toast" class:error>
          <span>{shown.text}</span>
          <button aria-label="Close the message" title="Close" onclick={close}>×</button>
        </div>
      {/key}
    {/if}
  </div>
{/each}

<style>
  .toast {
    position: fixed;
    z-index: 30;
    bottom: 24px;
    left: 50%;
    translate: -50% 0;
    display: flex;
    align-items: center;
    gap: 12px;
    max-width: min(640px, calc(100vw - 32px));
    padding: 10px 10px 10px 14px;
    border: 1px solid var(--line);
    border-left: 4px solid var(--accent);
    border-radius: 6px;
    background: var(--panel);
    box-shadow: 0 6px 24px rgba(0, 0, 0, 0.5);
    overflow-wrap: anywhere;
    animation: rise 0.15s ease-out;
  }
  .toast.error {
    border-left-color: var(--bad);
  }
  button {
    flex: none;
    width: 24px;
    height: 24px;
    padding: 0;
    border: 0;
    border-radius: 4px;
    background: none;
    color: var(--muted);
    font-size: 16px;
    cursor: pointer;
  }
  button:hover {
    background: var(--line);
    color: var(--text);
  }
  @keyframes rise {
    from {
      opacity: 0;
      translate: -50% 8px;
    }
  }
  @media print {
    .toast {
      display: none;
    }
  }
</style>
