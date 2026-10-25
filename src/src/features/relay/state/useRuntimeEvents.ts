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

export function useRuntimeEvents({
  requestRoutingRefresh,
  modeRef,
  pageRef,
  stateRevision,
  runtimeActivityOverlay,
  runtimeActivityRuntimeId,
  runtimeRoutingOrderBase,
  setRuntime,
  setRuntimeActivity,
  setUsageRevision,
  invalidateUsageCache,
  runBackgroundRefresh,
}: {
  requestRoutingRefresh: () => void;
  modeRef: MutableRefObject<RelayMode>;
  pageRef: MutableRefObject<PageId>;
  stateRevision: MutableRefObject<number>;
  runtimeActivityOverlay: MutableRefObject<Map<string, RuntimeActivitySnapshot>>;
  runtimeActivityRuntimeId: MutableRefObject<number>;
  runtimeRoutingOrderBase: MutableRefObject<RuntimeRoutingOrder>;
  setRuntime: Dispatch<SetStateAction<RuntimeSnapshot | null>>;
  setRuntimeActivity: Dispatch<SetStateAction<RuntimeActivityState>>;
  setUsageRevision: Dispatch<SetStateAction<number>>;
  invalidateUsageCache: () => void;
  runBackgroundRefresh: (force?: boolean) => void;
}) {
  useEffect(() => {
    let active = true;
    let runtimeRefreshQueued = false;
    let runtimeRefreshTimer: number | undefined;
    let usageRefreshTimer: number | undefined;
    let usageDirty = false;
    let unlisten: (() => void) | undefined;
    let unlistenUsage: (() => void) | undefined;
    let unlistenRuntimeActivity: (() => void) | undefined;
    let runtimeActivityFrame: number | undefined;
    let runtimeActivityFlushQueued = false;
    let pendingRuntimeActivity: RuntimeActivityState | null = null;
    const flushRuntimeActivity = () => {
      runtimeActivityFrame = undefined;
      runtimeActivityFlushQueued = false;
      if (!active || modeRef.current !== "local") {
        pendingRuntimeActivity = null;
        return;
      }
      const pending = pendingRuntimeActivity;
      pendingRuntimeActivity = null;
      if (pending) {
        const candidates = Object.fromEntries(runtimeActivityOverlay.current);
        setRuntimeActivity((previousActivitySnapshot) => {
          // A late event can still be new for a different candidate.
          const latestActivity = compareRuntimeActivity(pending, previousActivitySnapshot) > 0
            ? pending
            : previousActivitySnapshot;
          return { ...latestActivity, candidates };
        });
      }
      if (document.visibilityState !== "visible" || !isRuntimeRefreshPage(pageRef.current)) return;
      setRuntime((snapshot) => {
        if (!snapshot) return snapshot;
        const currentOrder = snapshot.gateway.routingOrder ?? [];
        const baseOrder = runtimeRoutingOrderBase.current.length
          ? runtimeRoutingOrderBase.current
          : currentOrder;
        const nextOrder = applyRuntimeActivities(baseOrder, runtimeActivityOverlay.current.values());
        return nextOrder === currentOrder
          ? snapshot
          : { ...snapshot, gateway: { ...snapshot.gateway, routingOrder: nextOrder } };
      });
    };
    const scheduleRuntimeActivityFlush = () => {
      if (runtimeActivityFlushQueued || document.visibilityState !== "visible") return;
      runtimeActivityFlushQueued = true;
      runtimeActivityFrame = window.requestAnimationFrame(flushRuntimeActivity);
    };
    const handleActivityVisibilityChange = () => {
      if (document.visibilityState !== "visible") {
        if (runtimeActivityFrame !== undefined) window.cancelAnimationFrame(runtimeActivityFrame);
        runtimeActivityFrame = undefined;
        runtimeActivityFlushQueued = false;
        return;
      }
      // Publish the latest per-candidate overlay once the window becomes visible.
      if (pendingRuntimeActivity) scheduleRuntimeActivityFlush();
      if (usageDirty) scheduleUsageRefresh();
    };
    document.addEventListener("visibilitychange", handleActivityVisibilityChange);
    const scheduleUsageRefresh = () => {
      if (!active) return;
      invalidateUsageCache();
      usageDirty = true;
      const targetPage = pageRef.current;
      if (!active || document.visibilityState !== "visible" || !isUsageRefreshPage(targetPage) || usageRefreshTimer !== undefined) return;
      usageRefreshTimer = window.setTimeout(() => {
        usageRefreshTimer = undefined;
        if (!active || document.visibilityState !== "visible" || !isUsageRefreshPage(pageRef.current)) return;
        setUsageRevision((previousRevision) => previousRevision + 1);
        usageDirty = false;
      }, usageRefreshDebounceMs(targetPage));
    };
    void relayCommands.onStateChanged(() => {
      if (!active) return;
      stateRevision.current += 1;
      // Settings can change labels/pricing on the next visit, but quota,
      // discovery and account health events are not history writes. Only a
      // usage notification should reload an already displayed report.
      invalidateUsageCache();
      if (document.visibilityState !== "visible") return;
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
      requestRoutingRefresh();
    }).then((stop) => {
      if (active) unlistenRuntimeActivity = stop;
      else stop();
    }).catch(() => {
      // The short routing poll remains the fallback when activity events are unavailable.
    });
    void relayCommands.onUsageRecorded(() => {
      if (modeRef.current === "local") scheduleUsageRefresh();
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
      pendingRuntimeActivity = null;
      document.removeEventListener("visibilitychange", handleActivityVisibilityChange);
      unlisten?.();
      unlistenUsage?.();
      unlistenRuntimeActivity?.();
    };
  }, [requestRoutingRefresh, runBackgroundRefresh, invalidateUsageCache]);
}
