import { useEffect, useRef, useState } from "react";

/** Shows the clicked value immediately. A failed save returns to the last saved value.
 * `saving` stays true for the control's own in-flight save so it cannot be fired twice. */
export function usePendingFlag(saved: boolean) {
  const [pending, setPending] = useState<boolean | null>(null);
  const [saving, setSaving] = useState(false);
  const ticket = useRef(0);
  useEffect(() => {
    if (pending !== null && pending === saved) setPending(null);
  }, [pending, saved]);
  const select = (enabled: boolean, save: () => Promise<boolean>) => {
    const current = ++ticket.current;
    setPending(enabled);
    setSaving(true);
    void Promise.resolve().then(save).catch(() => false).then((ok) => {
      if (ticket.current !== current || ok) return;
      setPending((value) => value === enabled ? null : value);
    }).finally(() => {
      // A newer choice owns the flag; its own save releases it.
      if (ticket.current === current) setSaving(false);
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
