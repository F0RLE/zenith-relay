type Schedule = (work: () => void) => () => void;

/** Coalesce derived-cache writes without retaining another serialized snapshot. */
export function createAnalyticsPersistence(write: () => void, schedule: Schedule) {
  let cancelPending: (() => void) | undefined;
  let revision = 0;
  let dirty = false;
  const flush = () => {
    revision += 1;
    cancelPending?.();
    cancelPending = undefined;
    if (!dirty) return;
    dirty = false;
    write();
  };
  const request = () => {
    dirty = true;
    if (cancelPending) return;
    const scheduledRevision = ++revision;
    cancelPending = schedule(() => {
      if (scheduledRevision !== revision) return;
      flush();
    });
  };
  return { request, flush };
}
