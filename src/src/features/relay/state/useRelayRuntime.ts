import { useCallback, useEffect, useRef, useState } from "react";
import { recordPerformance } from "../../../platform/desktop";
import { relayCommands, type UiState } from "../api/commands";
import type { PageId, RelayMode, RuntimeActivitySnapshot, RuntimeActivityState, RuntimeSnapshot } from "../api/types";
import { applyRuntimeActivities, compareRuntimeActivity, preferNewerRuntimeOrder, reconcileRuntimeActivityOverlay } from "../routingOrder";
import {
  RELAY_STORAGE_KEYS,
  readRelayPreference,
  writeRelayPreference,
} from "./relayPreferences";
import { LatestRequestGate } from "./latestRequestGate";
import {
  ROUTING_REFRESH_INTERVAL_MS,
  RUNTIME_EVENT_REFRESH_DEBOUNCE_MS,
  RUNTIME_REFRESH_INTERVAL_MS,
  STARTUP_RUNTIME_RETRY_DELAYS_MS,
  isRuntimeRefreshPage,
  isUsageRefreshPage,
  startupSnapshotNeedsRetry,
  usageRefreshDebounceMs,
} from "./refreshPolicy";
import { loadRuntimeSnapshot } from "./snapshotLoader";

type RelayRuntimeDependencies = {
  cancelOperations: () => void;
  clearInactiveUsage: (mode: RelayMode) => void;
  resetUsage: () => void;
  reportErrorFeedback: (cause: unknown, key: string, fallbackCode: string) => void;
};

type RuntimeRoutingOrder = NonNullable<RuntimeSnapshot["gateway"]["routingOrder"]>;

/** Own mode navigation, runtime snapshots, event synchronization, and refresh timing. */
export function useRelayRuntime({
  cancelOperations,
  clearInactiveUsage,
  resetUsage,
  reportErrorFeedback,
}: RelayRuntimeDependencies) {
  const [mode, setModeState] = useState<RelayMode>(() => readRelayPreference(RELAY_STORAGE_KEYS.mode, "local") as RelayMode);
  const [page, setPageState] = useState<PageId>("overview");
  const [runtime, setRuntime] = useState<RuntimeSnapshot | null>(null);
  const [runtimeRevision, setRuntimeRevision] = useState(0);
  const [usageRevision, setUsageRevision] = useState(0);
  const [readyState, setReadyState] = useState<UiState | null>(null);
  const [runtimeActivity, setRuntimeActivity] = useState<RuntimeActivityState>({
    revision: 0,
    lastCandidateId: null,
    candidates: {},
  });
  const [loading, setLoading] = useState(true);
  const modeRef = useRef(mode);
  const pageRef = useRef(page);
  const stateRevision = useRef(1);
  const refreshedRevision = useRef(0);
  const snapshotRequest = useRef(new LatestRequestGate());
  const backgroundRefreshPending = useRef<symbol | null>(null);
  const backgroundRefreshRetryTimer = useRef<number | undefined>(undefined);
  const runtimeRefreshPage = useRef<PageId>("overview");
  const modeSwitchStartedAt = useRef<{ mode: RelayMode; startedAt: number } | null>(null);
  const pageOpenStartedAt = useRef<{ page: PageId; startedAt: number } | null>(null);
  const runtimeActivityOverlay = useRef(new Map<string, RuntimeActivitySnapshot>());
  const runtimeActivityRuntimeId = useRef(0);
  const runtimeSnapshotRef = useRef<RuntimeSnapshot | null>(null);
  // Keep the scheduler order separate from the activity facts. The map also
  // retains the latest zero-count event: a routing poll can finish while a
  // request is in flight, and that stale snapshot must not resurrect the
  // candidate after its release event arrives.
  const runtimeRoutingOrderBase = useRef<RuntimeRoutingOrder>([]);
  const runtimeRoutingSupported = mode !== "remote"
    || Boolean(runtime?.capabilities.features.includes("runtime_routing"));

  const setPage = useCallback((next: PageId) => {
    if (pageRef.current === next) return;
    pageRef.current = next;
    pageOpenStartedAt.current = { page: next, startedAt: performance.now() };
    setPageState(next);
  }, []);

  const invalidateRefreshes = useCallback(() => {
    snapshotRequest.current.invalidate();
    backgroundRefreshPending.current = null;
    if (backgroundRefreshRetryTimer.current !== undefined) {
      window.clearTimeout(backgroundRefreshRetryTimer.current);
      backgroundRefreshRetryTimer.current = undefined;
    }
  }, []);

  const setMode = useCallback((next: RelayMode) => {
    if (modeRef.current === next) return;
    invalidateRefreshes();
    modeSwitchStartedAt.current = { mode: next, startedAt: performance.now() };
    modeRef.current = next;
    stateRevision.current += 1;
    runtimeActivityOverlay.current.clear();
    runtimeActivityRuntimeId.current = 0;
    runtimeRoutingOrderBase.current = [];
    runtimeSnapshotRef.current = null;
    setRuntimeActivity({ revision: 0, lastCandidateId: null, candidates: {} });
    writeRelayPreference(RELAY_STORAGE_KEYS.mode, next);
    setRuntime(null);
    resetUsage();
    setModeState(next);
    setPage("overview");
    cancelOperations();
  }, [cancelOperations, invalidateRefreshes, resetUsage, setPage]);

  const loadSnapshot = useCallback(async (force: boolean) => {
    const requestedMode = mode;
    if (modeRef.current !== requestedMode) return;
    const requestedRevision = stateRevision.current;
    if (!force && refreshedRevision.current === requestedRevision) return;
    const startedAt = performance.now();
    await snapshotRequest.current.run(
      async () => {
        const loaded = await loadRuntimeSnapshot(requestedMode, relayCommands);
        void recordPerformance("full_snapshot", performance.now() - startedAt, requestedMode);
        return loaded;
      },
      (loaded) => {
        if (requestedMode === "zenith") setReadyState(loaded.readyState);
        if (requestedMode === "local") {
          runtimeRoutingOrderBase.current = preferNewerRuntimeOrder(runtimeRoutingOrderBase.current, loaded.snapshot?.gateway.routingOrder ?? []);
          reconcileRuntimeActivityOverlay(runtimeRoutingOrderBase.current, runtimeActivityOverlay.current);
        }
        const snapshot = loaded.snapshot && requestedMode === "local"
          ? {
            ...loaded.snapshot,
            gateway: {
              ...loaded.snapshot.gateway,
              routingOrder: visibleLocalRoutingOrder(runtimeRoutingOrderBase.current, runtimeActivityOverlay.current),
            },
          }
          : loaded.snapshot;
        runtimeSnapshotRef.current = snapshot;
        setRuntime(snapshot);
        clearInactiveUsage(requestedMode);
        refreshedRevision.current = requestedRevision;
        setRuntimeRevision((current) => current + 1);
      },
    );
  }, [clearInactiveUsage, mode]);

  const refresh = useCallback(async (force = true) => {
    if (modeRef.current !== mode) return;
    // Explicit refreshes, including those following a mutation, replace any
    // older background work instead of keeping its pending marker alive.
    invalidateRefreshes();
    await loadSnapshot(force);
  }, [invalidateRefreshes, loadSnapshot, mode]);

  const runBackgroundRefresh = useCallback((force = false) => {
    const refreshMode = mode;
    if (
      !isRuntimeRefreshPage(pageRef.current)
      || backgroundRefreshPending.current !== null
      || backgroundRefreshRetryTimer.current !== undefined
      || modeRef.current !== refreshMode
    ) return;
    const request = Symbol();
    backgroundRefreshPending.current = request;
    void (async () => {
      try {
        await loadSnapshot(force);
        // A previous visit to this mode must not schedule retries or release
        // the pending marker belonging to the current visit.
        if (backgroundRefreshPending.current !== request) return;
        // A state event may arrive while the snapshot is in flight. Keep one
        // trailing retry, but let a short settling window coalesce a burst of
        // writes instead of spinning through full snapshots back-to-back.
        if (
          modeRef.current === refreshMode
          && isRuntimeRefreshPage(pageRef.current)
          && document.visibilityState === "visible"
          && refreshedRevision.current !== stateRevision.current
          && backgroundRefreshRetryTimer.current === undefined
        ) {
          backgroundRefreshRetryTimer.current = window.setTimeout(() => {
            backgroundRefreshRetryTimer.current = undefined;
            if (
              modeRef.current === refreshMode
              && isRuntimeRefreshPage(pageRef.current)
              && document.visibilityState === "visible"
            ) {
              runBackgroundRefresh();
            }
          }, RUNTIME_EVENT_REFRESH_DEBOUNCE_MS);
        }
      } catch {
        // The next state event, focus, or periodic refresh retries the snapshot.
      } finally {
        if (backgroundRefreshPending.current === request) backgroundRefreshPending.current = null;
      }
    })();
  }, [loadSnapshot, mode]);

  useEffect(() => {
    let active = true;
    let startupRetryTimer: number | undefined;
    let startupRetryIndex = 0;
    const scheduleStartupRetry = () => {
      if (!active || modeRef.current !== mode || !startupSnapshotNeedsRetry(runtimeSnapshotRef.current)) return;
      const delay = STARTUP_RUNTIME_RETRY_DELAYS_MS[startupRetryIndex];
      if (delay === undefined) return;
      startupRetryIndex += 1;
      startupRetryTimer = window.setTimeout(() => {
        startupRetryTimer = undefined;
        void refresh(true)
          .then(scheduleStartupRetry)
          .catch(() => {
            // State/focus events and the regular refresh remain the fallback
            // if a retry races a transient native startup error.
            scheduleStartupRetry();
          });
      }, delay);
    };
    setLoading(true);
    refresh()
      .then(scheduleStartupRetry)
      .catch((error) => active && reportErrorFeedback(error, "feedback.refreshFailed", "refresh_failed"))
      .finally(() => {
        if (!active) return;
        setLoading(false);
        if (performance.getEntriesByName("zenith:interactive", "mark").length) return;
        requestAnimationFrame(() => requestAnimationFrame(() => {
          performance.mark("zenith:interactive");
          const measure = performance.measure("zenith:interactive", "zenith:html-start", "zenith:interactive");
          void recordPerformance("interactive", measure.duration, "startup");
          window.dispatchEvent(new Event("zenith-startup-ready"));
        }));
      });
    return () => {
      active = false;
      if (startupRetryTimer !== undefined) window.clearTimeout(startupRetryTimer);
    };
  }, [mode, refresh, reportErrorFeedback]);

  useEffect(() => {
    if ((page !== "pool" && page !== "connections") || !runtime?.gateway.running || mode === "zenith" || !runtimeRoutingSupported) return;
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
  }, [mode, page, runtime?.gateway.running, runtimeRoutingSupported]);

  useEffect(() => {
    const enteredRuntimeRefreshPage = isRuntimeRefreshPage(page) && !isRuntimeRefreshPage(runtimeRefreshPage.current);
    runtimeRefreshPage.current = page;
    if (!isRuntimeRefreshPage(page)) return;

    const refreshVisibleRuntime = () => {
      if (document.visibilityState === "visible" && (mode === "remote" || refreshedRevision.current !== stateRevision.current)) {
        runBackgroundRefresh(mode === "remote");
      }
    };
    if (enteredRuntimeRefreshPage) refreshVisibleRuntime();
    const interval = window.setInterval(() => {
      // Remote changes do not emit desktop state events. Poll their snapshot
      // while visible; local snapshots can continue relying on revisions.
      if (document.visibilityState === "visible") runBackgroundRefresh(mode === "remote");
    }, RUNTIME_REFRESH_INTERVAL_MS);
    window.addEventListener("focus", refreshVisibleRuntime);
    document.addEventListener("visibilitychange", refreshVisibleRuntime);
    return () => {
      window.clearInterval(interval);
      window.removeEventListener("focus", refreshVisibleRuntime);
      document.removeEventListener("visibilitychange", refreshVisibleRuntime);
    };
  }, [mode, page, runBackgroundRefresh]);

  useEffect(() => invalidateRefreshes, [invalidateRefreshes]);

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
        setRuntimeActivity((current) => compareRuntimeActivity(pending, current) > 0
          ? { ...pending, candidates }
          : current);
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
        setUsageRevision((current) => current + 1);
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
      const previous = runtimeActivityOverlay.current.get(activity.candidateId);
      if (previous && compareRuntimeActivity(activity, previous) <= 0) return;
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

  useEffect(() => {
    const pending = modeSwitchStartedAt.current;
    if (!runtime || !pending || pending.mode !== mode) return;
    let secondFrame = 0;
    const firstFrame = requestAnimationFrame(() => {
      secondFrame = requestAnimationFrame(() => {
        if (modeSwitchStartedAt.current !== pending) return;
        modeSwitchStartedAt.current = null;
        void recordPerformance("mode_switch", performance.now() - pending.startedAt, mode);
      });
    });
    return () => {
      cancelAnimationFrame(firstFrame);
      cancelAnimationFrame(secondFrame);
    };
  }, [mode, runtime]);

  useEffect(() => {
    const pending = pageOpenStartedAt.current;
    if (!pending || pending.page !== page) return;
    let secondFrame = 0;
    const firstFrame = requestAnimationFrame(() => {
      secondFrame = requestAnimationFrame(() => {
        if (pageOpenStartedAt.current !== pending) return;
        pageOpenStartedAt.current = null;
        void recordPerformance("page_open", performance.now() - pending.startedAt, page);
      });
    });
    return () => {
      cancelAnimationFrame(firstFrame);
      cancelAnimationFrame(secondFrame);
    };
  }, [page]);

  return {
    mode,
    setMode,
    page,
    setPage,
    runtime,
    runtimeRevision,
    usageRevision,
    readyState,
    loading,
    runtimeActivity,
    refresh,
  };
}

function visibleLocalRoutingOrder(
  base: RuntimeRoutingOrder,
  overlay: ReadonlyMap<string, RuntimeActivitySnapshot>,
) {
  return applyRuntimeActivities(
    base,
    overlay.values(),
  );
}
