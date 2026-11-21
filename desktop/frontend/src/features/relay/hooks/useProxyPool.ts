import { useCallback, useEffect, useRef, useState } from "react";
import type { SetStateAction } from "react";
import { relayCommands } from "../api/commands";
import type { ProxyPoolSummary } from "../api/types";
import { createDisplaySnapshotCache } from "../state/displaySnapshotCache";

// Only the redacted summary (endpoint without credentials), never proxy URLs.
const proxyPreview = createDisplaySnapshotCache("proxies", 1, (value): value is ProxyPoolSummary => {
  const pool = value as ProxyPoolSummary | null;
  return pool != null && Number.isFinite(pool.total) && Array.isArray(pool.entries)
    && pool.entries.every((entry) => entry && typeof entry.id === "string" && typeof entry.endpoint === "string" && Array.isArray(entry.assignedAccountIds));
});
let cachedPool: ProxyPoolSummary | null = null;
let mutationRevision = 0;
let nextRead = 0;
let acceptedRead = 0;

export function useProxyPool(enabled = true, revision = 0) {
  const [pool, setPoolState] = useState<ProxyPoolSummary | null>(() => enabled ? cachedPool : null);
  const [failed, setFailed] = useState(false);
  const revisionRef = useRef(0);
  const setPool = useCallback((nextPool: SetStateAction<ProxyPoolSummary | null>) => {
    // A committed mutation owns the snapshot over every older in-flight read.
    revisionRef.current += 1;
    mutationRevision += 1;
    cachedPool = typeof nextPool === "function" ? nextPool(cachedPool) : nextPool;
    if (cachedPool) proxyPreview.remember("local", cachedPool);
    setPoolState(cachedPool);
    setFailed(false);
  }, []);
  const load = useCallback(async () => {
    if (!enabled) return;
    const requestRevision = ++revisionRef.current;
    const requestMutation = mutationRevision;
    const read = ++nextRead;
    void proxyPreview.restore().then(() => {
      if (requestRevision !== revisionRef.current || requestMutation !== mutationRevision) return;
      cachedPool ??= proxyPreview.peek("local");
      setPoolState(cachedPool);
    });
    try {
      const loadedPool = await relayCommands.getProxyPool();
      if (requestRevision !== revisionRef.current) return;
      if (requestMutation === mutationRevision && read > acceptedRead) {
        cachedPool = loadedPool;
        proxyPreview.remember("local", loadedPool);
        acceptedRead = read;
      }
      setPoolState(cachedPool);
      setFailed(false);
    } catch {
      if (requestRevision === revisionRef.current && requestMutation === mutationRevision) setFailed(true);
    }
  }, [enabled]);
  useEffect(() => {
    setPoolState(enabled ? cachedPool : null);
    void load();
    return () => { revisionRef.current += 1; };
  }, [enabled, load, revision]);
  useEffect(() => {
    if (!enabled) return;
    let disposed = false;
    let dirty = false;
    let timer: number | undefined;
    let unlisten: (() => void) | undefined;
    const changed = () => {
      dirty = true;
      if (disposed || document.visibilityState !== "visible" || timer !== undefined) return;
      timer = window.setTimeout(() => {
        timer = undefined;
        if (document.visibilityState !== "visible") return;
        dirty = false;
        void load();
      }, 100);
    };
    const visible = () => { if (dirty) changed(); };
    void relayCommands.onProxyPoolChanged(changed).then((release) => {
      if (disposed) release(); else unlisten = release;
    }).catch(() => { /* The next explicit view read still revalidates storage. */ });
    document.addEventListener("visibilitychange", visible);
    return () => {
      disposed = true;
      if (timer !== undefined) window.clearTimeout(timer);
      unlisten?.();
      document.removeEventListener("visibilitychange", visible);
    };
  }, [enabled, load]);
  return { pool: enabled ? pool : null, setPool, failed, load };
}
