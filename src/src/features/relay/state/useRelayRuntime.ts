import { useCallback, useEffect, useRef, useState } from "react";
import { recordPerformance } from "../../../platform/desktop";
import { relayCommands, type UiState } from "../api/commands";
import type { PageId, RelayMode, RuntimeActivitySnapshot, RuntimeActivityState, RuntimeSnapshot } from "../api/types";
import { preferNewerRuntimeOrder, reconcileRuntimeActivityOverlay } from "../routingOrder";
import {
  RELAY_STORAGE_KEYS,
  readRelayPreference,
  writeRelayPreference,
} from "./relayPreferences";
import { LatestRequestGate } from "./latestRequestGate";
import {
  RUNTIME_EVENT_REFRESH_DEBOUNCE_MS,
  RUNTIME_REFRESH_INTERVAL_MS,
  STARTUP_RUNTIME_RETRY_DELAYS_MS,
  isRuntimeRefreshPage,
  startupSnapshotNeedsRetry,
} from "./refreshPolicy";
import { useRuntimeEvents, visibleLocalRoutingOrder } from "./useRuntimeEvents";
import { useRuntimeRouting } from "./useRuntimeRouting";
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
  const visibleRuntimeRef = useRef<RuntimeSnapshot | null>(null);
  const snapshotsByMode = useRef<Partial<Record<RelayMode, RuntimeSnapshot>>>({});
  // Keep the scheduler order separate from the activity facts. The map also
  // retains the latest zero-count event: a routing poll can finish while a
  // request is in flight, and that stale snapshot must not resurrect the
  // candidate after its release event arrives.
  const runtimeRoutingOrderBase = useRef<RuntimeRoutingOrder>([]);
  visibleRuntimeRef.current = runtime;
  const runtimeRoutingSupported = mode !== "remote"
    || Boolean(runtime?.capabilities.features.includes("runtime_routing"));

  const setPage = useCallback((targetPage: PageId) => {
    if (pageRef.current === targetPage) return;
    pageRef.current = targetPage;
    pageOpenStartedAt.current = { page: targetPage, startedAt: performance.now() };
    setPageState(targetPage);
  }, []);

  const invalidateRefreshes = useCallback(() => {
    snapshotRequest.current.invalidate();
    backgroundRefreshPending.current = null;
    if (backgroundRefreshRetryTimer.current !== undefined) {
      window.clearTimeout(backgroundRefreshRetryTimer.current);
      backgroundRefreshRetryTimer.current = undefined;
    }
  }, []);

  const setMode = useCallback((targetMode: RelayMode) => {
    if (modeRef.current === targetMode) return;
    if (visibleRuntimeRef.current) snapshotsByMode.current[modeRef.current] = visibleRuntimeRef.current;
    const cached = snapshotsByMode.current[targetMode] ?? null;
    invalidateRefreshes();
    modeSwitchStartedAt.current = { mode: targetMode, startedAt: performance.now() };
    modeRef.current = targetMode;
    stateRevision.current += 1;
    runtimeActivityOverlay.current.clear();
    runtimeActivityRuntimeId.current = 0;
    runtimeRoutingOrderBase.current = [];
    runtimeSnapshotRef.current = cached;
    setRuntimeActivity({ revision: 0, lastCandidateId: null, candidates: {} });
    writeRelayPreference(RELAY_STORAGE_KEYS.mode, targetMode);
    setRuntime(cached);
    setLoading(cached === null);
    resetUsage();
    setModeState(targetMode);
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
        if (snapshot) snapshotsByMode.current[requestedMode] = snapshot;
        setRuntime(snapshot);
        clearInactiveUsage(requestedMode);
        refreshedRevision.current = requestedRevision;
        setRuntimeRevision((previousRevision) => previousRevision + 1);
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
    const refreshRequestToken = Symbol();
    backgroundRefreshPending.current = refreshRequestToken;
    void (async () => {
      try {
        await loadSnapshot(force);
        // A previous visit to this mode must not schedule retries or release
        // the pending marker belonging to the current visit.
        if (backgroundRefreshPending.current !== refreshRequestToken) return;
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
        if (backgroundRefreshPending.current === refreshRequestToken) backgroundRefreshPending.current = null;
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
    if (runtimeSnapshotRef.current === null) setLoading(true);
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

  useRuntimeRouting({
    mode,
    page,
    gatewayRunning: runtime?.gateway.running,
    runtimeRoutingSupported,
    runtimeRoutingOrderBase,
    runtimeActivityOverlay,
    setRuntime,
  });

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

  useRuntimeEvents({
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
  });

  usePaintDuration(
    modeSwitchStartedAt.current,
    runtime,
    (pending) => Boolean(runtime) && pending.mode === mode && modeSwitchStartedAt.current === pending,
    () => {
      modeSwitchStartedAt.current = null;
    },
    "mode_switch",
    mode,
  );
  usePaintDuration(
    pageOpenStartedAt.current,
    page,
    (pending) => pending.page === page && pageOpenStartedAt.current === pending,
    () => {
      pageOpenStartedAt.current = null;
    },
    "page_open",
    page,
  );

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

// `watch` restarts the two-frame measurement when the screen becomes ready.
// The callbacks read refs, so they stay tied to the pending mark rather than
// to a new function on every render.
function usePaintDuration<T extends { startedAt: number }>(
  pending: T | null,
  watch: unknown,
  accept: (pending: T) => boolean,
  clear: () => void,
  name: string,
  detail: string,
) {
  useEffect(() => {
    if (!pending || !accept(pending)) return;
    let secondFrame = 0;
    const firstFrame = requestAnimationFrame(() => {
      secondFrame = requestAnimationFrame(() => {
        if (!accept(pending)) return;
        clear();
        void recordPerformance(name, performance.now() - pending.startedAt, detail);
      });
    });
    return () => {
      cancelAnimationFrame(firstFrame);
      cancelAnimationFrame(secondFrame);
    };
  }, [pending, watch, name, detail]);
}
