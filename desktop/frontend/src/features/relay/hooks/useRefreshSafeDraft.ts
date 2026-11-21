import { useState } from "react";

/** Refresh clean fields; only an explicit scope/reset change replaces a dirty field. */
export function useRefreshSafeDraft<T extends string>(saved: T, resetKey?: string) {
  const [draft, setDraft] = useState(saved);
  const [[previous, previousKey], setBaseline] = useState([saved, resetKey] as const);
  // Adjust before committing children; an effect would briefly expose stale input.
  if (previous !== saved || previousKey !== resetKey) {
    setBaseline([saved, resetKey]);
    if (previousKey !== resetKey || draft === previous) setDraft(saved);
  }
  return [draft, setDraft] as const;
}
