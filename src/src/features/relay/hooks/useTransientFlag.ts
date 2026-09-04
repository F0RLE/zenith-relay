import { useCallback, useEffect, useRef, useState } from "react";

export function useTransientFlag(durationMs: number) {
  const [active, setActive] = useState(false);
  const timeoutRef = useRef<number | null>(null);

  const clearTimer = useCallback(() => {
    if (timeoutRef.current != null) window.clearTimeout(timeoutRef.current);
    timeoutRef.current = null;
  }, []);

  const deactivate = useCallback(() => {
    clearTimer();
    setActive(false);
  }, [clearTimer]);

  const activate = useCallback(() => {
    clearTimer();
    setActive(true);
    timeoutRef.current = window.setTimeout(() => {
      timeoutRef.current = null;
      setActive(false);
    }, durationMs);
  }, [clearTimer, durationMs]);

  useEffect(() => clearTimer, [clearTimer]);

  return [active, activate, deactivate] as const;
}
