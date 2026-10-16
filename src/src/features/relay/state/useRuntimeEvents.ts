import { useEffect, type Dispatch, type MutableRefObject, type SetStateAction } from "react";
import { relayCommands } from "../api/commands";
import type { PageId, RelayMode, RuntimeActivitySnapshot, RuntimeActivityState, RuntimeSnapshot } from "../api/types";
import { applyRuntimeActivities, compareRuntimeActivity } from "../routingOrder";
import {
  RUNTIME_EVENT_REFRESH_DEBOUNCE_MS,
  isRuntimeRefreshPage,
  isUsageRefreshPage,
  usageRefreshDebounceMs,
} from "./refreshPolicy";

type RuntimeRoutingOrder = NonNullable<RuntimeSnapshot["gateway"]["routingOrder"]>;

export function visibleLocalRoutingOrder(
  base: RuntimeRoutingOrder,
  overlay: ReadonlyMap<string, RuntimeActivitySnapshot>,
) {
  return applyRuntimeActivities(
    base,
    overlay.values(),
  );
}

export function useRuntimeEvents({
  modeRef,
  pageRef,
  stateRevision,
  runtimeActivityOverlay,
  runtimeActivityRuntimeId,
  runtimeRoutingOrderBase,
  setRuntime,
  setRuntimeActivity,
  setUsageRevision,
  runBackgroundRefresh,
}: {
  modeRef: MutableRefObject<RelayMode>;
  pageRef: MutableRefObject<PageId>;
  stateRevision: MutableRefObject<number>;
  runtimeActivityOverlay: MutableRefObject<Map<string, RuntimeActivitySnapshot>>;
  runtimeActivityRuntimeId: MutableRefObject<number>;
  runtimeRoutingOrderBase: MutableRefObject<RuntimeRoutingOrder>;
  setRuntime: Dispatch<SetStateAction<RuntimeSnapshot | null>>;
  setRuntimeActivity: Dispatch<SetStateAction<RuntimeActivityState>>;
  setUsageRevision: Dispatch<SetStateAction<number>>;
  runBackgroundRefresh: (force?: boolean) => void;
}) {
  useEffect(() => {
    let active = true;
    let runtimeRefreshQueued = false;
    let runtimeRefreshTimer: number | undefined;
    let usageRefreshTimer: number | undefined;
    let unlisten: (() => void) | undefined;
    let unlistenUsage: (() => void) | undefined;
    let unlistenRuntimeActivity: (() => void) | undefined;
    let runtimeActivityFrame: number | undefined;
    let runtimeActivityFallbackTimer: number | undefined;
    let runtimeActivityFlushQueued = false;
    let pendingRuntimeActivity: RuntimeActivityState | null = null;
    const flushRuntimeActivity = () => {
      runtimeActivityFrame = undefined;
      runtimeActivityFallbackTimer = undefined;
      runtimeActivityFlushQueued = false;
      if (!active || modeRef.current !== "local") {
        pendingRuntimeActivity = null;
        return;
      }
      const pending = pendingRuntimeActivity;
      pendingRuntimeActivity = null;
      if (pending) {
        const candidates = Object.fromEntries(runtimeActivityOverlay.current);
        setRuntimeActivity((previousActivitySnapshot) => compareRuntimeActivity(pending, previousActivitySnapshot) > 0
          ? { ...pending, candidates }
          : previousActivitySnapshot);
      }
      if (document.visibilityState !== "visible" || !isRuntimeRefreshPage(pageRef.current)) return;
      setRuntime((snapshot) => {
        if (!snapshot) return snapshot;
        const currentOrder = snapshot.gateway.routingOrder ?? [];
        const baseOrder = runtimeRoutingOrderBase.current.length
          ? runtimeRoutingOrderBase.current
          : currentOrder;
        const nextOrder = visibleLocalRoutingOrder(baseOrder, runtimeActivityOverlay.current);
        return nextOrder === currentOrder
          ? snapshot
          : { ...snapshot, gateway: { ...snapshot.gateway, routingOrder: nextOrder } };
      });
    };
    const scheduleRuntimeActivityFlush = () => {
      if (runtimeActivityFlushQueued) return;
      runtimeActivityFlushQueued = true;
      if (document.visibilityState === "visible") {
        runtimeActivityFrame = window.requestAnimationFrame(flushRuntimeActivity);
      } else {
        // Background documents may throttle animation frames indefinitely.
        runtimeActivityFallbackTimer = window.setTimeout(flushRuntimeActivity, 16);
      }
    };
    const handleActivityVisibilityChange = () => {
      if (document.visibilityState === "hidden" && runtimeActivityFrame !== undefined) {
        window.cancelAnimationFrame(runtimeActivityFrame);
        runtimeActivityFrame = undefined;
        flushRuntimeActivity();
      }
    };
    document.addEventListener("visibilitychange", handleActivityVisibilityChange);
    const scheduleUsageRefresh = () => {
      const targetPage = pageRef.current;
      if (!active || document.visibilityState !== "visible" || !isUsageRefreshPage(targetPage) || usageRefreshTimer !== undefined) return;
      usageRefreshTimer = window.setTimeout(() => {
        usageRefreshTimer = undefined;
        if (!active || document.visibilityState !== "visible" || !isUsageRefreshPage(pageRef.current)) return;
        setUsageRevision((previousRevision) => previousRevision + 1);
      }, usageRefreshDebounceMs(targetPage));
    };
    void relayCommands.onStateChanged(() => {
      stateRevision.current += 1;
      if (!active || document.visibilityState !== "visible") return;
      if (isUsageRefreshPage(pageRef.current)) {
        scheduleUsageRefresh();
        if (pageRef.current === "usage") return;
      }
      if (runtimeRefreshQueued || !isRuntimeRefreshPage(pageRef.current)) return;
      runtimeRefreshQueued = true;
      runtimeRefreshTimer = window.setTimeout(() => {
        if (!active) return;
        runBackgroundRefresh();
        runtimeRefreshQueued = false;
      }, RUNTIME_EVENT_REFRESH_DEBOUNCE_MS);
    }).then((stop) => {
      if (active) unlisten = stop;
      else stop();
    }).catch(() => {
      // Initial load and periodic refresh still keep the UI current if Tauri event wiring is unavailable.
    });
    void relayCommands.onRuntimeActivity((activity) => {
      if (!active || modeRef.current !== "local") return;
      const runtimeId = activity.runtimeId ?? 0;
      if (runtimeId < Math.max(runtimeActivityRuntimeId.current, runtimeRoutingOrderBase.current[0]?.runtimeId ?? 0)) return;
      if (runtimeId > runtimeActivityRuntimeId.current) {
        runtimeActivityOverlay.current.clear();
        runtimeActivityRuntimeId.current = runtimeId;
      }
      const previousActivity = runtimeActivityOverlay.current.get(activity.candidateId);
      if (previousActivity && compareRuntimeActivity(activity, previousActivity) <= 0) return;
      // Keep both active and zero-count snapshots. A zero-count snapshot is a
      // tombstone for an older live poll state and must be applied to the next
      // base order as well as the current one.
      runtimeActivityOverlay.current.set(activity.candidateId, activity);
      if (!pendingRuntimeActivity || compareRuntimeActivity(activity, pendingRuntimeActivity) > 0) {
        pendingRuntimeActivity = {
          ...(activity.runtimeId == null ? {} : { runtimeId: activity.runtimeId }),
          revision: activity.revision,
          lastCandidateId: activity.candidateId,
          candidates: {},
        };
      }
      scheduleRuntimeActivityFlush();
    }).then((stop) => {
      if (active) unlistenRuntimeActivity = stop;
      else stop();
    }).catch(() => {
      // The short routing poll remains the fallback when activity events are unavailable.
    });
    void relayCommands.onUsageRecorded(() => {
      if (modeRef.current === "local" && isUsageRefreshPage(pageRef.current)) scheduleUsageRefresh();
    }).then((stop) => {
      if (active) unlistenUsage = stop;
      else stop();
    }).catch(() => {
      // The manual refresh remains available if event wiring is unavailable.
    });
    return () => {
      active = false;
      if (runtimeRefreshTimer !== undefined) window.clearTimeout(runtimeRefreshTimer);
      if (usageRefreshTimer !== undefined) window.clearTimeout(usageRefreshTimer);
      if (runtimeActivityFrame !== undefined) window.cancelAnimationFrame(runtimeActivityFrame);
      if (runtimeActivityFallbackTimer !== undefined) window.clearTimeout(runtimeActivityFallbackTimer);
      pendingRuntimeActivity = null;
      document.removeEventListener("visibilitychange", handleActivityVisibilityChange);
      unlisten?.();
      unlistenUsage?.();
      unlistenRuntimeActivity?.();
    };
  }, [runBackgroundRefresh]);
}
