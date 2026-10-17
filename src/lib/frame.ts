// A window that is not drawn gets no animation frames: minimized, or hidden by WebView2 when
// it is told so. Wayland has no minimized state to tell the page, so `document.hidden` stays
// false there, but the compositor stops asking for frames all the same. Work that only feeds
// the screen therefore waits for the next frame, and catches up once the window is shown.

/** Returns a function that runs `fn` on the next frame, once however often it is called. */
export function onFrame(fn: () => void) {
  let frame = 0;
  const run = () => {
    frame ||= requestAnimationFrame(() => {
      frame = 0;
      fn();
    });
  };
  /** Drops the pending run, for when what it would update goes away. */
  run.cancel = () => {
    cancelAnimationFrame(frame);
    frame = 0;
  };
  return run;
}
