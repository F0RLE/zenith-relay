import { useEffect, type Dispatch, type MutableRefObject, type SetStateAction } from "react";
import { relayCommands } from "../api/commands";
import type { PageId, RelayMode, RuntimeActivitySnapshot, RuntimeSnapshot } from "../api/types";
import { preferNewerRuntimeOrder, reconcileRuntimeActivityOverlay } from "../routingOrder";
import { ROUTING_REFRESH_INTERVAL_MS } from "./refreshPolicy";
import { visibleLocalRoutingOrder } from "./useRuntimeEvents";

type RuntimeRoutingOrder = NonNullable<RuntimeSnapshot["gateway"]["routingOrder"]>;

/** Poll the live candidate order while Pool or Connections is showing a running gateway. */
export function useRuntimeRouting({
  mode,
  page,
  gatewayRunning,
  runtimeRoutingSupported,
  runtimeRoutingOrderBase,
  runtimeActivityOverlay,
  setRuntime,
}: {
  mode: RelayMode;
  page: PageId;
  gatewayRunning: boolean | undefined;
  runtimeRoutingSupported: boolean;
  runtimeRoutingOrderBase: MutableRefObject<RuntimeRoutingOrder>;
  runtimeActivityOverlay: MutableRefObject<Map<string, RuntimeActivitySnapshot>>;
  setRuntime: Dispatch<SetStateAction<RuntimeSnapshot | null>>;
}) {
  useEffect(() => {
    if ((page !== "pool" && page !== "connections") || !gatewayRunning || mode === "zenith" || !runtimeRoutingSupported) return;
    let active = true;
    let pending = false;
    const refreshRouting = async () => {
      if (!active || pending || document.visibilityState !== "visible") return;
      pending = true;
      try {
        const routingOrder = mode === "local"
          ? await relayCommands.localRuntimeOrder()
          : await relayCommands.remoteRuntimeOrder();
        if (!active || routingOrder == null) return;
        if (mode === "local") {
          runtimeRoutingOrderBase.current = preferNewerRuntimeOrder(runtimeRoutingOrderBase.current, routingOrder);
          reconcileRuntimeActivityOverlay(runtimeRoutingOrderBase.current, runtimeActivityOverlay.current);
        }
        const visibleRoutingOrder = mode === "local"
          ? visibleLocalRoutingOrder(runtimeRoutingOrderBase.current, runtimeActivityOverlay.current)
          : routingOrder;
        setRuntime((snapshot) => snapshot ? {
          ...snapshot,
          gateway: { ...snapshot.gateway, routingOrder: visibleRoutingOrder },
        } : snapshot);
      } catch {
        // The full refresh keeps the last known order if the lightweight probe fails.
      } finally {
        pending = false;
      }
    };
    void refreshRouting();
    const interval = window.setInterval(() => void refreshRouting(), ROUTING_REFRESH_INTERVAL_MS);
    return () => {
      active = false;
      window.clearInterval(interval);
    };
  }, [mode, page, gatewayRunning, runtimeRoutingSupported]);
}
