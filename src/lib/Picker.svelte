<script lang="ts" generics="T">
  type Props = {
    value: T;
    /** What the menu offers, in order. */
    options: T[];
    /** A value in words, as the box and the menu show it. */
    label: (value: T) => string;
    /** The user picked or typed a different value. */
    onpick: (value: T) => void;
    /** Reads a typed value, null when it makes no sense. Without it the box can only be picked from. */
    parse?: (text: string) => T | null;
    /** What can be typed for an option, shown beside it. */
    short?: (value: T) => string;
    title?: string;
    /** For a box without a label around it. */
    ariaLabel?: string;
    /** Width of the box; by default about that of the longest option. */
    width?: string;
  };
  let { value, options, label, onpick, parse, short, title, ariaLabel, width }: Props = $props();

  const id = $props.id();
  let input: HTMLInputElement;
  let open = $state(false);
  // Index of the option the arrow keys are on, -1 for none.
  let active = $state(-1);
  // What is being typed, or null to show the value.
  let typed = $state<string | null>(null);

  function show() {
    active = options.indexOf(value);
    open = true;
  }

  /** The option the arrow keys are on, else what was typed, else null to keep the value. */
  const current = () => (active >= 0 ? options[active] : typed && parse ? parse(typed) : null);

  // Every pick, by key or click, ends by letting go of the box, and this takes it; leaving
  // the box with Tab or a click elsewhere takes the same as Enter would. The next click then
  // opens the menu afresh.
  function take() {
    const picked = current();
    open = false;
    typed = null;
    active = -1;
    if (picked !== null && picked !== value) onpick(picked);
  }

  function onkeydown(event: KeyboardEvent) {
    const last = options.length - 1;
    if (event.key === "ArrowDown" || event.key === "ArrowUp") {
      event.preventDefault();
      if (!open) return show();
      if (event.key === "ArrowDown") active = active >= last ? 0 : active + 1;
      else active = active <= 0 ? last : active - 1;
    } else if ((event.key === "Home" || event.key === "End") && (open || !parse)) {
      // In a box that takes typing, a closed menu leaves these keys to the caret.
      event.preventDefault();
      if (!open) show();
      active = event.key === "Home" ? 0 : last;
    } else if (event.key === "Enter") {
      // Enter must not reach a form around the box, where it may start something.
      event.preventDefault();
      if (!open && typed === null) return show();
      input.blur();
    } else if (event.key === "Escape" && (open || typed !== null)) {
      event.preventDefault();
      typed = null;
      active = -1;
      input.blur();
    } else if (!parse && event.key.length === 1 && !event.ctrlKey && !event.altKey && !event.metaKey) {
      // As in a select: a letter goes to the next option that starts with it.
      const key = event.key.toLowerCase();
      const from = open ? active : options.indexOf(value);
      const next = options.findIndex((_, i) => i > from && label(options[i]).toLowerCase().startsWith(key));
      const found = next >= 0 ? next : options.findIndex((option) => label(option).toLowerCase().startsWith(key));
      if (found < 0) return;
      if (!open) show();
      active = found;
    }
  }

  // The click must not reach a label around the box either, which would focus it again.
  function onclick(event: MouseEvent) {
    event.preventDefault();
    const option = (event.target as HTMLElement).closest<HTMLElement>("[data-index]");
    if (!option) return;
    active = Number(option.dataset.index);
    input.blur();
  }

  /** Opens the menu from a click, with the text selected so that typing replaces it. */
  function showFromClick() {
    show();
    if (parse) input.select();
  }
</script>

<span class="picker">
  <input
    bind:this={input}
    role="combobox"
    aria-expanded={open}
    aria-controls={open ? `${id}-menu` : undefined}
    aria-activedescendant={open && active >= 0 ? `${id}-${active}` : undefined}
    autocomplete="off"
    readonly={!parse}
    aria-label={ariaLabel}
    style:width={width ?? `${Math.max(...options.map((option) => label(option).length))}ch`}
    {title}
    placeholder={label(value)}
    value={typed ?? label(value)}
    onclick={() => open || showFromClick()}
    oninput={(event) => {
      typed = event.currentTarget.value;
      active = -1;
    }}
    onblur={take}
    {onkeydown}
  />
  <button
    type="button"
    tabindex="-1"
    aria-label="Show choices"
    onmousedown={(event) => {
      event.preventDefault();
      if (open) open = false;
      else {
        input.focus();
        showFromClick();
      }
    }}>▾</button
  >
  {#if open}
    <!-- Pressing the mouse button on the menu would take focus from the box and close it.
         The keys are handled by the box, which keeps focus while the menu is open. -->
    <!-- svelte-ignore a11y_click_events_have_key_events -->
    <ul id="{id}-menu" role="listbox" onmousedown={(event) => event.preventDefault()} {onclick}>
      {#each options as option, index}
        <li id="{id}-{index}" role="option" aria-selected={option === value} class:active={index === active} data-index={index}>
          {label(option)}{#if short}<kbd>{short(option)}</kbd>{/if}
        </li>
      {/each}
    </ul>
  {/if}
</span>

<style>
  .picker {
    position: relative;
    display: inline-flex;
    border-radius: 4px;
  }
  /* The ring goes around the box and its arrow together; around the box alone the arrow
     would cover its right side. */
  .picker:focus-within {
    outline: 2px solid var(--accent);
    outline-offset: 1px;
  }
  input:focus-visible {
    outline: none;
  }
  input,
  button {
    font: inherit;
    color: var(--text);
    background: var(--bg);
    border: 1px solid var(--line);
    padding: 5px 8px;
  }
  input {
    border-radius: 4px 0 0 4px;
  }
  input[readonly] {
    cursor: pointer;
  }
  button {
    border-left: 0;
    border-radius: 0 4px 4px 0;
    padding: 5px 7px;
    color: var(--muted);
    cursor: pointer;
  }
  ul {
    position: absolute;
    top: calc(100% + 4px);
    left: 0;
    z-index: 30;
    box-sizing: border-box;
    min-width: 100%;
    margin: 0;
    padding: 4px;
    list-style: none;
    background: var(--panel);
    border: 1px solid var(--line);
    border-radius: 6px;
    box-shadow: 0 8px 24px rgba(0, 0, 0, 0.45);
  }
  /* With the list's border and padding, the options start where the text in the box does. */
  li {
    display: flex;
    justify-content: space-between;
    gap: 16px;
    padding: 3px 4px;
    border-radius: 4px;
    color: var(--text);
    white-space: nowrap;
    cursor: pointer;
  }
  kbd {
    font: 12px/1.6 ui-monospace, "Cascadia Mono", Consolas, monospace;
    color: var(--muted);
  }
  li:hover,
  li.active {
    background: var(--line);
  }
  li[aria-selected="true"] {
    color: var(--accent);
  }
</style>
