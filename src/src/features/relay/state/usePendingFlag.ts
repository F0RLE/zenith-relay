import { useEffect, useRef, useState } from "react";

/** Shows the clicked value immediately. A failed save returns to the last saved value. */
export function usePendingFlag(saved: boolean) {
  const [pending, setPending] = useState<boolean | null>(null);
  const ticket = useRef(0);
  useEffect(() => {
    if (pending !== null && pending === saved) setPending(null);
  }, [pending, saved]);
  const select = (enabled: boolean, save: () => Promise<boolean>) => {
    const current = ++ticket.current;
    setPending(enabled);
    void save().then((ok) => {
      if (ticket.current !== current || ok) return;
      setPending((value) => value === enabled ? null : value);
    });
  };
  return { checked: pending ?? saved, select };
}

/** Remembers a choice confirmed by a completed command until the snapshot agrees. */
export function useSavedChoice(saved: boolean) {
  const [pending, setPending] = useState<boolean | null>(null);
  useEffect(() => {
    if (pending !== null && pending === saved) setPending(null);
  }, [pending, saved]);
  return { value: pending ?? saved, confirm: setPending };
}
