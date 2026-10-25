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
  // Share the pending scheduler read and fallback deadline across both pages.
  const observing = mode === "local" && (page === "pool" || page === "connections") && Boolean(gatewayRunning);
  useEffect(() => {
    if (!observing) return;
    let active = true;
    let pending = false;
    let refreshQueued = false;
    let refreshTimer: number | undefined;
    let fallbackTimer: number | undefined;
    const clearTimers = () => {
      window.clearTimeout(refreshTimer);
      window.clearTimeout(fallbackTimer);
      refreshTimer = undefined;
      fallbackTimer = undefined;
    };
    const scheduleRefresh = () => {
      if (!active || document.visibilityState !== "visible") return;
      if (pending) {
        // Queue a follow-up read when a new runtime event arrives.
        refreshQueued = true;
        return;
      }
      if (refreshTimer !== undefined) return;
      refreshTimer = window.setTimeout(() => {
        refreshTimer = undefined;
        void refreshRouting();
      }, RUNTIME_EVENT_REFRESH_DEBOUNCE_MS);
    };
    const refreshRouting = async () => {
      if (!active || pending || document.visibilityState !== "visible") return;
      clearTimers();
      refreshQueued = false;
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
        if (active && document.visibilityState === "visible") {
          if (refreshQueued) {
            refreshQueued = false;
            scheduleRefresh();
          } else {
            // Preserve recovery/cooldown polling without racing a fresh event read
            // or piling up work behind a slow native response.
            fallbackTimer = window.setTimeout(() => void refreshRouting(), ROUTING_REFRESH_INTERVAL_MS);
          }
        }
      }
    };
    const onVisibilityChange = () => {
      if (document.visibilityState === "visible") scheduleRefresh();
      else clearTimers();
    };
    scheduleRefreshRef.current = scheduleRefresh;
    void refreshRouting();
    document.addEventListener("visibilitychange", onVisibilityChange);
    return () => {
      active = false;
      scheduleRefreshRef.current = () => {};
      clearTimers();
      document.removeEventListener("visibilitychange", onVisibilityChange);
    };
  }, [observing]);
  return requestRoutingRefresh;
}
