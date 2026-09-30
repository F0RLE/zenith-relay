import { useCallback, useEffect, useRef, useState } from "react";
import { relayCommands } from "../../api/commands";
import type { ProxyCheckResult } from "../../api/types";

export type ProxyCheckState = { pending: boolean; result?: ProxyCheckResult };
export type ProxyChecks = ReturnType<typeof useProxyChecks>;

/** Session-only diagnostics, separate from proxy assignments and routing health. */
export function useProxyChecks(resetKey: string) {
  const [checks, setChecks] = useState<Record<string, ProxyCheckState>>({});
  const generation = useRef(0);
  const inFlight = useRef(new Set<string>());
  useEffect(() => {
    generation.current += 1;
    inFlight.current.clear();
    setChecks({});
    return () => { generation.current += 1; };
  }, [resetKey]);

  const check = useCallback(async (proxyId: string) => {
    if (inFlight.current.has(proxyId)) return;
    const run = generation.current;
    inFlight.current.add(proxyId);
    setChecks((current) => ({ ...current, [proxyId]: { pending: true } }));
    let result: ProxyCheckResult;
    try {
      result = await relayCommands.checkStoredProxy(proxyId);
    } catch {
      result = { proxyId, checkedAtMs: Date.now(), elapsedMs: 0, ip: null, countryCode: null, errorCode: "proxy_check_unavailable" };
    }
    if (run !== generation.current) return;
    inFlight.current.delete(proxyId);
    setChecks((current) => ({ ...current, [proxyId]: { pending: false, result } }));
  }, []);

  const checkMany = useCallback(async (proxyIds: string[]) => {
    const run = generation.current;
    const queue = [...new Set(proxyIds)];
    await Promise.all(Array.from({ length: Math.min(3, queue.length) }, async () => {
      while (run === generation.current) {
        const id = queue.shift();
        if (!id) break;
        await check(id);
      }
    }));
  }, [check]);
  return { checks, check, checkMany };
}
