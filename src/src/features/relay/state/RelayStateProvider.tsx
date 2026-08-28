import { ReactNode, useCallback, useEffect, useMemo, useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import { recordPerformance, setWindowBackgroundColor } from "../../../platform/desktop";
import { relayCommands, type UiState } from "../api/commands";
import type { PageId, ProfileActivation, ProfileBinding, RelayMode, RuntimeActivitySnapshot, RuntimeSnapshot } from "../api/types";
import { useConfirm } from "../components/Ui";
import { buildAccountIdentityIndex, displayAccountIdentity } from "./accountIdentity";
import {
  RELAY_STORAGE_KEYS,
  readRelayPreference,
  writeRelayPreference,
} from "./relayPreferences";
import { useAccountIdentityReveal } from "./useAccountIdentityReveal";
import { useRelayOperations } from "./useRelayOperations";
import { useRelayPreferences } from "./useRelayPreferences";
import { useRelayUsage } from "./useRelayUsage";
import { RelayContext, type PerformOptions, type RelayContextValue } from "./relayStateContext";
import {
  ROUTING_REFRESH_INTERVAL_MS,
  RUNTIME_EVENT_REFRESH_DEBOUNCE_MS,
  RUNTIME_REFRESH_INTERVAL_MS,
  USAGE_EVENT_REFRESH_DEBOUNCE_MS,
  isRuntimeRefreshPage,
} from "./refreshPolicy";
import { projectRuntimeAccountLabels } from "./runtimeDisplay";
import { applyRuntimeActivities, applyRuntimeActivity } from "../routingOrder";
import { loadRuntimeSnapshot } from "./snapshotLoader";

export { useRelayState } from "./relayStateContext";
export type { Feedback } from "./relayStateContext";

export function RelayStateProvider({ children }: { children: ReactNode }) {
  const { i18n, t } = useTranslation();
  const confirm = useConfirm();
  const [mode, setModeState] = useState<RelayMode>(() => readRelayPreference(RELAY_STORAGE_KEYS.mode, "local") as RelayMode);
  const [page, setPageState] = useState<PageId>("overview");
  const [runtime, setRuntime] = useState<RuntimeSnapshot | null>(null);
  const [runtimeRevision, setRuntimeRevision] = useState(0);
  const [usageRevision, setUsageRevision] = useState(0);
  const [readyState, setReadyState] = useState<UiState | null>(null);
  const [loading, setLoading] = useState(true);
  const [revealedAccountIdentities, setRevealedAccountIdentities] = useState<Record<string, string>>({});
  const {
    busy,
    feedback,
    performOperation,
    cancelOperations,
    clearFeedback,
    reportErrorFeedback,
  } = useRelayOperations();
  const {
    localUsagePage,
    remoteUsage,
    remoteUsagePage,
    loadLocalUsage,
    loadRemoteUsage,
    resetUsage,
    clearInactiveUsage,
  } = useRelayUsage(relayCommands);
  const modeRef = useRef(mode);
  const pageRef = useRef(page);
  const stateRevision = useRef(1);
  const refreshedRevision = useRef(0);
  const backgroundRefreshPending = useRef(new Set<RelayMode>());
  const runtimeRefreshPage = useRef<PageId>("overview");
  const modeSwitchStartedAt = useRef<{ mode: RelayMode; startedAt: number } | null>(null);
  const pageOpenStartedAt = useRef<{ page: PageId; startedAt: number } | null>(null);
  const runtimeActivityOverlay = useRef(new Map<string, RuntimeActivitySnapshot>());
  const setPage = useCallback((next: PageId) => {
    if (pageRef.current === next) return;
    pageRef.current = next;
    pageOpenStartedAt.current = { page: next, startedAt: performance.now() };
    setPageState(next);
  }, []);
  const setMode = useCallback((next: RelayMode) => {
    if (modeRef.current === next) return;
    modeSwitchStartedAt.current = { mode: next, startedAt: performance.now() };
    modeRef.current = next;
    stateRevision.current += 1;
    runtimeActivityOverlay.current.clear();
    writeRelayPreference(RELAY_STORAGE_KEYS.mode, next);
    setRuntime(null);
    resetUsage();
    setModeState(next);
    setPage("overview");
    cancelOperations();
  }, [cancelOperations, resetUsage, setPage]);
  const {
    onboardingComplete,
    finishOnboarding,
    resetOnboarding,
    theme,
    setTheme,
    profileSwitchBackupPrompt,
    setProfileSwitchBackupPrompt,
    codexPoolOauthSelection,
    setCodexPoolOauthSelection,
    accountIdentitiesVisible,
    setAccountIdentitiesVisible,
    accountValueVisible,
    setAccountValueVisible,
  } = useRelayPreferences({
    setMode,
    setPage,
    setRevealedAccountIdentities,
  });
  const canRevealAccountIdentities = mode === "local" || (mode === "remote" && Boolean(runtime?.capabilities.features.includes("account_identity_reveal")));
  const accountIdentitiesBusy = useAccountIdentityReveal({
    accounts: runtime?.accounts ?? [],
    canReveal: canRevealAccountIdentities,
    identitiesVisible: accountIdentitiesVisible,
    mode,
    setRevealedIdentities: setRevealedAccountIdentities,
  });
  const accountIndex = useMemo(() => buildAccountIdentityIndex(runtime?.accounts ?? []), [runtime?.accounts]);
  const codexWebsocketsEnabled = runtime?.gateway.codexWebsocketsEnabled ?? true;

  const refresh = useCallback(async (force = true) => {
    const requestedMode = mode;
    const requestedRevision = stateRevision.current;
    if (
      !force
      && modeRef.current === requestedMode
      && refreshedRevision.current === requestedRevision
    ) return;
    const startedAt = performance.now();
    const loaded = await loadRuntimeSnapshot(requestedMode, relayCommands);
    void recordPerformance("full_snapshot", performance.now() - startedAt, requestedMode);
    if (modeRef.current !== requestedMode) return;
    if (requestedMode === "zenith") setReadyState(loaded.readyState);
    const snapshot = loaded.snapshot && requestedMode === "local"
      ? {
        ...loaded.snapshot,
        gateway: {
          ...loaded.snapshot.gateway,
          routingOrder: applyRuntimeActivities(loaded.snapshot.gateway.routingOrder ?? [], runtimeActivityOverlay.current.values()),
        },
      }
      : loaded.snapshot;
    setRuntime(snapshot);
    clearInactiveUsage(requestedMode);
    refreshedRevision.current = requestedRevision;
    setRuntimeRevision((current) => current + 1);
  }, [clearInactiveUsage, mode]);

  const runBackgroundRefresh = useCallback(() => {
    const refreshMode = mode;
    if (
      !isRuntimeRefreshPage(pageRef.current)
      || backgroundRefreshPending.current.has(refreshMode)
      || modeRef.current !== refreshMode
    ) return;
    backgroundRefreshPending.current.add(refreshMode);
    void (async () => {
      try {
        do {
          await refresh(false);
        } while (
          modeRef.current === refreshMode
          && isRuntimeRefreshPage(pageRef.current)
          && document.visibilityState === "visible"
          && refreshedRevision.current !== stateRevision.current
        );
      } catch {
        // The next state event, focus, or periodic refresh retries the snapshot.
      } finally {
        backgroundRefreshPending.current.delete(refreshMode);
      }
    })();
  }, [mode, refresh]);

  useEffect(() => {
    let active = true;
    setLoading(true);
    refresh()
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
    };
  }, [refresh, reportErrorFeedback]);

  useEffect(() => {
    if ((page !== "pool" && page !== "connections") || !runtime?.gateway.running || mode === "zenith") return;
    if (mode === "remote" && !runtime.capabilities.features.includes("runtime_routing")) return;
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
        const visibleRoutingOrder = mode === "local"
          ? applyRuntimeActivities(routingOrder, runtimeActivityOverlay.current.values())
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
  }, [mode, page, runtime?.gateway.running, runtime?.capabilities.features]);

  useEffect(() => {
    const enteredRuntimeRefreshPage = isRuntimeRefreshPage(page) && !isRuntimeRefreshPage(runtimeRefreshPage.current);
    runtimeRefreshPage.current = page;
    if (!isRuntimeRefreshPage(page)) return;

    const refreshVisibleRuntime = () => {
      if (document.visibilityState === "visible" && refreshedRevision.current !== stateRevision.current) {
        runBackgroundRefresh();
      }
    };
    if (enteredRuntimeRefreshPage) refreshVisibleRuntime();
    const interval = window.setInterval(() => {
      if (document.visibilityState === "visible") runBackgroundRefresh();
    }, RUNTIME_REFRESH_INTERVAL_MS);
    window.addEventListener("focus", refreshVisibleRuntime);
    document.addEventListener("visibilitychange", refreshVisibleRuntime);
    return () => {
      window.clearInterval(interval);
      window.removeEventListener("focus", refreshVisibleRuntime);
      document.removeEventListener("visibilitychange", refreshVisibleRuntime);
    };
  }, [page, runBackgroundRefresh]);

  useEffect(() => {
    let active = true;
    let runtimeRefreshQueued = false;
    let runtimeRefreshTimer: number | undefined;
    let usageRefreshTimer: number | undefined;
    let unlisten: (() => void) | undefined;
    let unlistenUsage: (() => void) | undefined;
    let unlistenRuntimeActivity: (() => void) | undefined;
    const scheduleUsageRefresh = () => {
      if (!active || document.visibilityState !== "visible" || pageRef.current !== "usage" || usageRefreshTimer !== undefined) return;
      usageRefreshTimer = window.setTimeout(() => {
        usageRefreshTimer = undefined;
        if (!active || pageRef.current !== "usage" || document.visibilityState !== "visible") return;
        setUsageRevision((current) => current + 1);
      }, USAGE_EVENT_REFRESH_DEBOUNCE_MS);
    };
    void relayCommands.onStateChanged(() => {
      stateRevision.current += 1;
      runtimeActivityOverlay.current.clear();
      if (!active || document.visibilityState !== "visible") return;
      if (pageRef.current === "usage") {
        scheduleUsageRefresh();
        return;
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
      const previous = runtimeActivityOverlay.current.get(activity.candidateId);
      if (previous && activity.revision <= previous.revision) return;
      runtimeActivityOverlay.current.set(activity.candidateId, activity);
      if (document.visibilityState !== "visible" || !isRuntimeRefreshPage(pageRef.current)) return;
      setRuntime((snapshot) => snapshot ? {
        ...snapshot,
        gateway: {
          ...snapshot.gateway,
          routingOrder: applyRuntimeActivity(snapshot.gateway.routingOrder ?? [], activity),
        },
      } : snapshot);
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
      unlisten?.();
      unlistenUsage?.();
      unlistenRuntimeActivity?.();
    };
  }, [runBackgroundRefresh]);

  useEffect(() => {
    const systemTheme = window.matchMedia("(prefers-color-scheme: dark)");
    const applyTheme = () => {
      document.documentElement.dataset.theme = theme;
      const dark = theme === "dark" || (theme === "system" && systemTheme.matches);
      void setWindowBackgroundColor(dark ? "#121719" : "#f2f5f6");
    };
    applyTheme();
    if (theme !== "system") return;
    systemTheme.addEventListener("change", applyTheme);
    return () => systemTheme.removeEventListener("change", applyTheme);
  }, [theme]);

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

  const perform = useCallback(async (id: string, work: () => Promise<unknown>, successKey?: string, options?: PerformOptions) => {
    return performOperation(id, work, refresh, successKey, options);
  }, [performOperation, refresh]);

  const launchAttachedCodex = useCallback(() => perform(
    "profile-launch",
    relayCommands.launchManagedCodex,
    "feedback.launched",
  ), [perform]);

  const activateCodexProfile = useCallback(async (
    id: string,
    work: () => Promise<ProfileActivation>,
    launchAfter = false,
  ) => {
    if (profileSwitchBackupPrompt && !await confirm(t("profiles.switchBackupMessage"), {
      title: t("profiles.switchBackupTitle"),
      confirmLabel: t("profiles.switchBackupAction"),
    })) return false;
    const activated = await perform(id, work, launchAfter ? undefined : "feedback.profileAttached");
    return activated && (!launchAfter || await launchAttachedCodex());
  }, [confirm, launchAttachedCodex, perform, profileSwitchBackupPrompt, t]);

  const launchCodexProfile = useCallback(async (_binding: ProfileBinding) => {
    const stopped = await perform("profile-stop", relayCommands.stopManagedCodex);
    return stopped && launchAttachedCodex();
  }, [launchAttachedCodex, perform]);

  const setCodexBackgroundTasksEnabled = useCallback((enabled: boolean) => perform(
    "codex-background-tasks",
    mode === "local"
      ? () => relayCommands.setCodexBackgroundTasks(enabled)
      : mode === "remote"
        ? () => relayCommands.setRemoteCodexBackgroundTasks(enabled)
        : () => Promise.reject(new Error("Codex background tasks are not available in hosted mode")),
    "feedback.saved",
  ), [mode, perform]);

  const setCodexWebsocketsEnabled = useCallback((enabled: boolean) => perform(
    "codex-websockets",
      mode === "remote"
      ? async () => {
        const previous = codexWebsocketsEnabled;
        await relayCommands.setRemoteCodexWebsockets(enabled);
        try {
          return await relayCommands.setCodexProfileWebsockets(enabled);
        } catch (error) {
          try {
            await relayCommands.setRemoteCodexWebsockets(previous);
          } catch {
            // Keep the original profile error; the remote action is best-effort rollback.
          }
          throw error;
        }
      }
      : mode === "local"
        ? () => relayCommands.setCodexWebsockets(enabled)
        : () => Promise.reject(new Error("Codex WebSockets are not available in hosted mode")),
    "feedback.saved",
  ), [codexWebsocketsEnabled, mode, perform]);

  const accountDisplayName = useCallback((accountId?: string | null, fallbackLabel?: string | null) => {
    return displayAccountIdentity({
      index: accountIndex,
      accountId,
      fallbackLabel,
      identitiesVisible: accountIdentitiesVisible,
      canReveal: canRevealAccountIdentities,
      mode,
      revealedIdentities: revealedAccountIdentities,
    });
  }, [accountIdentitiesVisible, accountIndex, canRevealAccountIdentities, mode, revealedAccountIdentities]);

  const displayRuntime = useMemo(
    () => projectRuntimeAccountLabels(runtime, accountDisplayName),
    [accountDisplayName, runtime],
  );
  const value = useMemo<RelayContextValue>(() => ({
    mode,
    setMode,
    page,
    setPage,
    runtime: displayRuntime,
    runtimeRevision,
    usageRevision,
    accountIdentitiesVisible,
    accountIdentitiesBusy,
    canRevealAccountIdentities,
    setAccountIdentitiesVisible,
    accountValueVisible,
    setAccountValueVisible,
    accountDisplayName,
    localUsagePage,
    loadLocalUsage,
    remoteUsage,
    remoteUsagePage,
    loadRemoteUsage,
    readyState,
    loading,
    busy,
    feedback,
    refresh,
    perform,
    activateCodexProfile,
    launchCodexProfile,
    clearFeedback,
    onboardingComplete,
    finishOnboarding,
    resetOnboarding,
    theme,
    setTheme,
    profileSwitchBackupPrompt,
    setProfileSwitchBackupPrompt,
    codexPoolOauthSelection,
    setCodexPoolOauthSelection,
    codexBackgroundTasksEnabled: displayRuntime?.gateway.codexBackgroundTasksEnabled ?? true,
    setCodexBackgroundTasksEnabled,
    setCodexWebsocketsEnabled,
    codexWebsocketsEnabled: displayRuntime?.gateway.codexWebsocketsEnabled ?? true,
  }), [mode, setMode, page, displayRuntime, runtimeRevision, usageRevision, accountIdentitiesVisible, accountIdentitiesBusy, canRevealAccountIdentities, setAccountIdentitiesVisible, accountValueVisible, setAccountValueVisible, accountDisplayName, localUsagePage, loadLocalUsage, remoteUsage, remoteUsagePage, loadRemoteUsage, readyState, loading, busy, feedback, refresh, perform, activateCodexProfile, launchCodexProfile, clearFeedback, onboardingComplete, finishOnboarding, resetOnboarding, theme, setTheme, profileSwitchBackupPrompt, setProfileSwitchBackupPrompt, codexPoolOauthSelection, setCodexPoolOauthSelection, setCodexBackgroundTasksEnabled, setCodexWebsocketsEnabled]);

  useEffect(() => {
    document.documentElement.lang = i18n.language.startsWith("ru") ? "ru" : "en";
  }, [i18n.language]);

  return <RelayContext.Provider value={value}>{children}</RelayContext.Provider>;
}
