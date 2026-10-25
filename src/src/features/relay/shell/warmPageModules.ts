/** Warm code only, one module per idle slot after the initial screen is ready.
 * No hidden pages mount, subscribe to events, poll providers or fetch reports. */
export function warmPageModules(loaders: Array<() => Promise<unknown>>) {
  let disposed = false;
  let pending = false;
  let cancel: (() => void) | undefined;
  const queue = [...loaders];
  const next = () => {
    if (disposed || pending || !queue.length || document.visibilityState !== "visible") return;
    pending = true;
    const run = () => {
      cancel = undefined;
      if (disposed || document.visibilityState !== "visible") { pending = false; return; }
      const load = queue.shift();
      void load?.().catch(() => undefined).finally(() => { pending = false; next(); });
    };
    if (typeof window.requestIdleCallback === "function") {
      const id = window.requestIdleCallback(run, { timeout: 1_500 });
      cancel = () => window.cancelIdleCallback(id);
    } else {
      const id = window.setTimeout(run, 100);
      cancel = () => window.clearTimeout(id);
    }
  };
  // Give the initial screen its paint before evaluating secondary chunks.
  const frame = requestAnimationFrame(next);
  document.addEventListener("visibilitychange", next);
  return () => {
    disposed = true;
    cancelAnimationFrame(frame);
    cancel?.();
    document.removeEventListener("visibilitychange", next);
  };
}
