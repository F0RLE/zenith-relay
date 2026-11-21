import { useEffect, useRef, useState } from "react";

/** Shows the clicked value immediately. A failed save returns to the last saved value.
 * `saving` stays true for the control's own in-flight save so it cannot be fired twice. */
export function usePendingFlag(saved: boolean) {
  const [pending, setPending] = useState<boolean | null>(null);
  const [saving, setSaving] = useState(false);
  const inFlight = useRef(false);
  useEffect(() => {
    if (pending !== null && pending === saved) setPending(null);
  }, [pending, saved]);
  const select = (enabled: boolean, save: () => Promise<boolean>) => {
    if (inFlight.current) return;
    inFlight.current = true;
    setPending(enabled);
    setSaving(true);
    void Promise.resolve().then(save).catch(() => false).then((ok) => {
      if (ok) return;
      setPending((pendingValue) => pendingValue === enabled ? null : pendingValue);
    }).finally(() => {
      inFlight.current = false;
      setSaving(false);
    });
  };
  return { checked: pending ?? saved, saving, select };
}

/** Remembers a choice confirmed by a completed command until the snapshot agrees. */
export function useSavedChoice(saved: boolean) {
  const [pending, setPending] = useState<boolean | null>(null);
  useEffect(() => {
    if (pending !== null && pending === saved) setPending(null);
  }, [pending, saved]);
  return { value: pending ?? saved, confirm: setPending };
}
