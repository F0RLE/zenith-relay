import { useCallback, useEffect, useRef, type Dispatch, type MutableRefObject, type SetStateAction } from "react";
import { relayCommands } from "../api/commands";
import type { PageId, RelayMode, RuntimeActivitySnapshot, RuntimeSnapshot } from "../api/types";
import { applyRuntimeActivities, preferNewerRuntimeOrder, reconcileRuntimeActivityOverlay, sameRuntimeOrder } from "../routingOrder";
import { ROUTING_REFRESH_INTERVAL_MS, RUNTIME_EVENT_REFRESH_DEBOUNCE_MS } from "./refreshPolicy";

type RuntimeRoutingOrder = NonNullable<RuntimeSnapshot["gateway"]["routingOrder"]>;

/** Refresh scheduler preferences after activity, with polling as a fallback. */
export function useRuntimeRouting({
  mode,
  page,
  gatewayRunning,
  runtimeRoutingOrderBase,
  runtimeActivityOverlay,
  setRuntime,
}: {
  mode: RelayMode;
  page: PageId;
  gatewayRunning: boolean | undefined;
  runtimeRoutingOrderBase: MutableRefObject<RuntimeRoutingOrder>;
  runtimeActivityOverlay: MutableRefObject<Map<string, RuntimeActivitySnapshot>>;
  setRuntime: Dispatch<SetStateAction<RuntimeSnapshot | null>>;
}) {
  const scheduleRefreshRef = useRef<() => void>(() => {});
  const requestRoutingRefresh = useCallback(() => scheduleRefreshRef.current(), []);
  useEffect(() => {
    if (mode !== "local" || (page !== "pool" && page !== "connections") || !gatewayRunning) return;
    let active = true;
    let pending = false;
    let refreshQueued = false;
    let refreshTimer: number | undefined;
    const scheduleRefresh = () => {
      if (!active || refreshTimer !== undefined || document.visibilityState !== "visible") return;
      refreshTimer = window.setTimeout(() => {
        refreshTimer = undefined;
        void refreshRouting();
      }, RUNTIME_EVENT_REFRESH_DEBOUNCE_MS);
    };
    const refreshRouting = async () => {
      if (!active || document.visibilityState !== "visible") return;
      if (pending) {
        refreshQueued = true;
        return;
      }
      pending = true;
      try {
        const routingOrder = await relayCommands.localRuntimeOrder();
        if (!active || routingOrder == null) return;
        runtimeRoutingOrderBase.current = preferNewerRuntimeOrder(runtimeRoutingOrderBase.current, routingOrder);
        reconcileRuntimeActivityOverlay(runtimeRoutingOrderBase.current, runtimeActivityOverlay.current);
        const visibleRoutingOrder = applyRuntimeActivities(runtimeRoutingOrderBase.current, runtimeActivityOverlay.current.values());
        setRuntime((snapshot) => {
          if (!snapshot || sameRuntimeOrder(snapshot.gateway.routingOrder ?? [], visibleRoutingOrder)) return snapshot;
          return {
            ...snapshot,
            gateway: { ...snapshot.gateway, routingOrder: visibleRoutingOrder },
          };
        });
      } catch {
        // The full refresh keeps the last known order if the lightweight probe fails.
      } finally {
        pending = false;
        if (refreshQueued) {
          refreshQueued = false;
          scheduleRefresh();
        }
      }
    };
    scheduleRefreshRef.current = scheduleRefresh;
    void refreshRouting();
    const interval = window.setInterval(() => void refreshRouting(), ROUTING_REFRESH_INTERVAL_MS);
    return () => {
      active = false;
      scheduleRefreshRef.current = () => {};
      if (refreshTimer !== undefined) window.clearTimeout(refreshTimer);
      window.clearInterval(interval);
    };
  }, [mode, page, gatewayRunning]);
  return requestRoutingRefresh;
}
