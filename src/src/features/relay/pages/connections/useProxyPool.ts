import { useCallback, useEffect, useRef, useState } from "react";
import { relayCommands } from "../../api/commands";
import type { ProxyPoolSummary } from "../../api/types";

export function useProxyPool(enabled = true, revision = 0) {
  const [pool, setPool] = useState<ProxyPoolSummary | null>(null);
  const [failed, setFailed] = useState(false);
  const revisionRef = useRef(0);
  const load = useCallback(async () => {
    if (!enabled) return;
    const requestRevision = ++revisionRef.current;
    try {
      const loadedPool = await relayCommands.getProxyPool();
      if (requestRevision !== revisionRef.current) return;
      setPool(loadedPool);
      setFailed(false);
    } catch {
      if (requestRevision === revisionRef.current) setFailed(true);
    }
  }, [enabled]);
  useEffect(() => { void load(); return () => { revisionRef.current += 1; }; }, [load, revision]);
  return { pool, setPool, failed, load };
}
