import { useEffect, useRef, useState } from "react";
import type { PoolRoutingPolicy } from "../../api/types";
import { useRelayState } from "../../state/RelayStateProvider";
import { persistPoolRoutingEdits, readRoutingRuntime, type PoolRoutingEdit } from "./poolRoutingEdits";
import { supportsRotationEditing } from "./rotationModes";

const EMPTY_POLICY: PoolRoutingPolicy = { version: 3, mode: "automatic", members: [] };

export function usePoolRoutingEditor(onClose: () => void) {
  const { mode, runtime, perform } = useRelayState();
  const currentPolicy = runtime?.gateway.poolRouting;
  const currentRotation = runtime?.gateway.poolRotation;
  const base = useRef({ policy: currentPolicy ?? EMPTY_POLICY, rotation: currentRotation });
  const pending = useRef<PoolRoutingEdit[]>([]);
  const task = useRef<Promise<boolean> | null>(null);
  const mounted = useRef(true);
  const [view, setView] = useState(base.current);
  const [saving, setSaving] = useState(false);
  const [errorKey, setErrorKey] = useState<string | null>(null);
  useEffect(() => {
    mounted.current = true;
    return () => { mounted.current = false; };
  }, []);
  useEffect(() => {
    if (pending.current.length || task.current) return;
    if ((currentRotation?.inventoryRevision ?? 0) < (base.current.rotation?.inventoryRevision ?? 0)) return;
    base.current = { policy: currentPolicy ?? EMPTY_POLICY, rotation: currentRotation };
    setView(base.current);
  }, [currentPolicy, currentRotation, saving]);

  const edit = (change: PoolRoutingEdit) => {
    if (!supportsRotationEditing(currentPolicy, runtime?.capabilities.features ?? [])) return;
    pending.current.push(change);
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
          if (mounted.current) setView(saved);
        }, undefined, { backgroundRefresh: true, reportError: false, onError: (error, key) => {
          if (mounted.current) setErrorKey(error.code === "conflict" || error.code === "pool_routing_conflict" ? "pool.routingChanged" : key);
        } });
        if (!ok) {
          // A failed write may race with a successful external update. Show the
          // actual stored policy and discard only this editor's unsaved edits.
          try {
            const latest = await readRoutingRuntime(mode);
            if (latest?.gateway.poolRouting) base.current = { policy: latest.gateway.poolRouting, rotation: latest.gateway.poolRotation };
          } catch { /* Retain the last acknowledged policy when offline. */ }
          pending.current = [];
          if (mounted.current) setView(base.current);
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
  return { policy: view.policy, memberOrder: view.rotation?.memberOrder, edit, saving, errorKey, available: supportsRotationEditing(currentPolicy, runtime?.capabilities.features ?? []), close };
}
