import { useCallback, useEffect, useRef, useState } from "react";
import { relayCommands } from "../../api/commands";
import type { ProxyPoolSummary } from "../../api/types";

export function useProxyPool(enabled = true, revision = 0) {
  const [pool, setPool] = useState<ProxyPoolSummary | null>(null);
  const [failed, setFailed] = useState(false);
  const revisionRef = useRef(0);
  const load = useCallback(async () => {
    if (!enabled) return;
    const current = ++revisionRef.current;
    try {
      const next = await relayCommands.getProxyPool();
      if (current !== revisionRef.current) return;
      setPool(next);
      setFailed(false);
    } catch {
      if (current === revisionRef.current) setFailed(true);
    }
  }, [enabled]);
  useEffect(() => { void load(); return () => { revisionRef.current += 1; }; }, [load, revision]);
  return { pool, setPool, failed, load };
}
