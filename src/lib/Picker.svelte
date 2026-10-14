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
  // What is being typed, or null to show the value. Emptied on focus so that typing replaces
  // the value; the placeholder still shows it.
  let typed = $state<string | null>(null);

  function show() {
    active = options.indexOf(value);
    open = true;
  }

  /** Takes a picked or typed value (null keeps the old one) and closes the menu. */
  function take(picked: T | null) {
    open = false;
    typed = null;
    if (picked !== null && picked !== value) onpick(picked);
  }

  /** The box lets go after every pick, so the next click opens the menu afresh. */
  function done(picked: T | null) {
    take(picked);
    input.blur();
  }

  function onkeydown(event: KeyboardEvent) {
    if (event.key === "ArrowDown" || event.key === "ArrowUp") {
      event.preventDefault();
      if (!open) return show();
      active = (active + (event.key === "ArrowDown" ? 1 : options.length - 1)) % options.length;
    } else if (event.key === "Enter") {
      // Enter must not reach a form around the box, where it may start something.
      event.preventDefault();
      if (!open) return show();
      done(active >= 0 ? options[active] : typed && parse ? parse(typed) : null);
    } else if (event.key === "Escape" && open) {
      event.preventDefault();
      done(null);
    }
  }

  // The click must not reach a label around the box either, which would focus it again.
  function onclick(event: MouseEvent) {
    event.preventDefault();
    const option = (event.target as HTMLElement).closest<HTMLElement>("[data-index]");
    if (option) done(options[Number(option.dataset.index)]);
  }
</script>

<span class="picker">
  <input
    bind:this={input}
    role="combobox"
    aria-expanded={open}
    aria-controls="{id}-menu"
    aria-activedescendant={open && active >= 0 ? `${id}-${active}` : undefined}
    autocomplete="off"
    readonly={!parse}
    aria-label={ariaLabel}
    style:width={width ?? `${Math.max(...options.map((option) => label(option).length))}ch`}
    {title}
    placeholder={label(value)}
    value={typed ?? label(value)}
    onfocus={() => {
      if (parse) typed = "";
      show();
    }}
    onclick={() => open || show()}
    oninput={(event) => {
      typed = event.currentTarget.value;
      active = -1;
    }}
    onblur={() => take(typed && parse ? parse(typed) : null)}
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
        show();
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
