import { useEffect, useRef, useState } from "react";
import type { PoolRoutingPolicy } from "../../api/types";
import { useRelayState } from "../../state/RelayStateProvider";
import { applyPoolRoutingEdits, persistPoolRoutingEdits, readRoutingRuntime, type PoolRoutingEdit } from "./poolRoutingEdits";

const EMPTY_POLICY: PoolRoutingPolicy = { version: 1, mode: "smart", members: [] };

export function usePoolRoutingEditor(onClose: () => void) {
  const { mode, runtime, perform } = useRelayState();
  const current = runtime?.gateway.poolRouting;
  const base = useRef(current ?? EMPTY_POLICY);
  const pending = useRef<PoolRoutingEdit[]>([]);
  const task = useRef<Promise<boolean> | null>(null);
  const mounted = useRef(true);
  const [policy, setPolicy] = useState(base.current);
  const [saving, setSaving] = useState(false);
  const [errorKey, setErrorKey] = useState<string | null>(null);
  useEffect(() => {
    mounted.current = true;
    return () => { mounted.current = false; };
  }, []);
  useEffect(() => {
    base.current = current ?? EMPTY_POLICY;
    setPolicy(applyPoolRoutingEdits(base.current, pending.current));
  }, [current]);

  const edit = (change: PoolRoutingEdit) => {
    if (!current) return;
    pending.current.push(change);
    setPolicy((value) => applyPoolRoutingEdits(value, [change]));
    setErrorKey(null);
    if (task.current) return;
    setSaving(true);
    task.current = (async () => {
      while (pending.current.length && mounted.current) {
        const batch = [...pending.current];
        const ok = await perform("routing-policy", async () => {
          const saved = await persistPoolRoutingEdits(mode, batch);
          pending.current.splice(0, batch.length);
          base.current = saved;
          if (mounted.current) setPolicy(applyPoolRoutingEdits(saved, pending.current));
        }, undefined, { reportError: false, onError: (error, key) => {
          if (mounted.current) setErrorKey(error.code === "conflict" || error.code === "pool_routing_conflict" ? "pool.routingChanged" : key);
        } });
        if (!ok) {
          // A failed write may race with a successful external update. Show the
          // actual stored policy and discard only this editor's unsaved edits.
          try {
            const latest = await readRoutingRuntime(mode);
            base.current = latest?.gateway.poolRouting ?? base.current;
          } catch { /* Retain the last acknowledged policy when offline. */ }
          pending.current = [];
          if (mounted.current) setPolicy(base.current);
          return false;
        }
      }
      return true;
    })().finally(() => {
      task.current = null;
      if (mounted.current) setSaving(false);
    });
  };

  const close = async () => {
    const ok = await task.current;
    if (ok !== false && mounted.current) onClose();
  };
  return { policy, edit, saving, errorKey, available: Boolean(current), close };
}
