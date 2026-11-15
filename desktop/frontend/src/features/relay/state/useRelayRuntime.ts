import { useCallback, useEffect, useRef, useState } from "react";
import { recordPerformance } from "../../../platform/desktop";
import { revealStartupShell, setStartupPhase } from "../../../platform/startup";
import { relayCommands } from "../api/commands";
import type { PageId, RelayMode, RuntimeActivitySnapshot, RuntimeActivityState, RuntimeSnapshot } from "../api/types";
import { applyRuntimeActivities, preferNewerRuntimeOrder, reconcileRuntimeActivityOverlay } from "../routingOrder";
import {
  RELAY_STORAGE_KEYS,
  readRelayMode,
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
import { useRuntimeEvents } from "./useRuntimeEvents";
import { useRuntimeRouting } from "./useRuntimeRouting";
import { loadRuntimeSnapshot } from "./snapshotLoader";
import { rememberRuntimePreview, runtimePreview } from "./runtimePreview";

type RelayRuntimeDependencies = {
  cancelOperations: () => void;
  resetUsage: () => void;
  invalidateUsageCache: () => void;
  reportErrorFeedback: (cause: unknown, key: string, fallbackCode: string) => void;
};

type RuntimeRoutingOrder = NonNullable<RuntimeSnapshot["gateway"]["routingOrder"]>;

/** Own mode navigation, runtime snapshots, event synchronization, and refresh timing. */
export function useRelayRuntime({
  cancelOperations,
  resetUsage,
  invalidateUsageCache,
  reportErrorFeedback,
}: RelayRuntimeDependencies) {
  const [mode, setModeState] = useState<RelayMode>(() => readRelayMode());
  const [page, setPageState] = useState<PageId>("overview");
  const [runtime, setRuntime] = useState<RuntimeSnapshot | null>(null);
  const [runtimeReady, setRuntimeReady] = useState(false);
  const [runtimeRevision, setRuntimeRevision] = useState(0);
  const [usageRevision, setUsageRevision] = useState(0);
  const [runtimeActivity, setRuntimeActivity] = useState<RuntimeActivityState>({
    revision: 0,
    lastCandidateId: null,
    candidates: {},
  });
  const [loading, setLoading] = useState(true);
  const [initialized, setInitialized] = useState(false);
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
    setRuntimeReady(false);
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
        if (loaded && requestedMode === "local") rememberRuntimePreview(loaded);
        if (requestedMode === "local") {
          runtimeRoutingOrderBase.current = preferNewerRuntimeOrder(runtimeRoutingOrderBase.current, loaded?.gateway.routingOrder ?? []);
          reconcileRuntimeActivityOverlay(runtimeRoutingOrderBase.current, runtimeActivityOverlay.current);
        }
        const snapshot = loaded && requestedMode === "local"
          ? {
            ...loaded,
            gateway: {
              ...loaded.gateway,
              routingOrder: applyRuntimeActivities(runtimeRoutingOrderBase.current, runtimeActivityOverlay.current.values()),
            },
          }
          : loaded;
        runtimeSnapshotRef.current = snapshot;
        if (snapshot) snapshotsByMode.current[requestedMode] = snapshot;
        setRuntime(snapshot);
        setRuntimeReady(true);
        setInitialized(true);
        refreshedRevision.current = requestedRevision;
        setRuntimeRevision((previousRevision) => previousRevision + 1);
      },
    );
  }, [mode]);

  useEffect(() => {
    if (mode !== "local") return;
    let active = true;
    void runtimePreview.restore().then(() => {
      // Never overwrite a newer native snapshot or leak local data into Hosted.
      if (!active || modeRef.current !== "local" || runtimeSnapshotRef.current) return;
      const preview = runtimePreview.peek("local");
      if (preview) setRuntime((current) => current ?? preview);
    });
    return () => { active = false; };
  }, [mode]);

  const refresh = useCallback(async (force = true) => {
    if (modeRef.current !== mode) return;
    // Explicit refreshes invalidate older background work, including after mutations.
    invalidateRefreshes();
    await loadSnapshot(force);
  }, [invalidateRefreshes, loadSnapshot, mode]);

  const runBackgroundRefresh = useCallback((force = false) => {
    const refreshMode = mode;
    if (
      document.visibilityState !== "visible"
      || !isRuntimeRefreshPage(pageRef.current)
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
        // Coalesce state events arriving during the snapshot into one trailing
        // retry after a short settling window.
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
      if (!active || mode === "hosted" || modeRef.current !== mode || !startupSnapshotNeedsRetry(runtimeSnapshotRef.current)) return;
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
      .catch((error) => {
        if (!active) return;
        reportErrorFeedback(error, "feedback.refreshFailed", "refresh_failed");
        setStartupPhase("error");
      })
      .finally(() => {
        if (!active) return;
        setLoading(false);
      });
    return () => {
      active = false;
      if (startupRetryTimer !== undefined) window.clearTimeout(startupRetryTimer);
    };
  }, [mode, refresh, reportErrorFeedback]);

  useEffect(() => {
    if (!initialized || loading || performance.getEntriesByName("zenith:interactive", "mark").length) return;
    let secondFrame = 0;
    const firstFrame = requestAnimationFrame(() => {
      secondFrame = requestAnimationFrame(() => {
        performance.mark("zenith:interactive");
        const measure = performance.measure("zenith:interactive", "zenith:html-start", "zenith:interactive");
        void recordPerformance("interactive", measure.duration, "startup");
        revealStartupShell();
      });
    });
    return () => {
      cancelAnimationFrame(firstFrame);
      cancelAnimationFrame(secondFrame);
    };
  }, [initialized, loading]);

  const requestRoutingRefresh = useRuntimeRouting({
    mode,
    page,
    gatewayRunning: runtimeReady && runtime?.gateway.running,
    runtimeRoutingOrderBase,
    runtimeActivityOverlay,
    setRuntime,
  });

  useEffect(() => {
    const enteredRuntimeRefreshPage = isRuntimeRefreshPage(page) && !isRuntimeRefreshPage(runtimeRefreshPage.current);
    runtimeRefreshPage.current = page;
    if (mode === "hosted" || !isRuntimeRefreshPage(page)) return;

    const refreshVisibleRuntime = () => {
      if (document.visibilityState === "visible" && refreshedRevision.current !== stateRevision.current) {
        runBackgroundRefresh(false);
      }
    };
    if (enteredRuntimeRefreshPage) refreshVisibleRuntime();
    const interval = window.setInterval(() => {
      // Revisions coalesce unchanged local snapshots.
      if (document.visibilityState === "visible") runBackgroundRefresh(false);
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
  });

  usePaintDuration(
    modeSwitchStartedAt.current,
    mode === "hosted" ? loading : runtime,
    (pending) => (mode === "hosted" ? !loading : Boolean(runtime)) && pending.mode === mode && modeSwitchStartedAt.current === pending,
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
    runtimeReady,
    runtimeRevision,
    usageRevision,
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
