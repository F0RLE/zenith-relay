import { ReactNode, useCallback, useEffect, useMemo, useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import { recordPerformance, setWindowBackgroundColor } from "../../../platform/desktop";
import { relayCommands, type UiState } from "../api/commands";
import type { LocalUsage, LocalUsagePage, PageId, ProfileActivation, ProfileBinding, RelayMode, RemoteUsage, RemoteUsagePage, RemoteUsageQuery, RuntimeSnapshot } from "../api/types";
import { useConfirm } from "../components/Ui";
import { buildAccountIdentityIndex, displayAccountIdentity } from "./accountIdentity";
import { sanitizeFeedbackError } from "./feedback";
import {
  RELAY_STORAGE_KEYS,
  readAccountValueVisibility,
  readCodexPoolOauthSelection,
  readRelayPreference,
  removeRelayPreference,
  writeAccountValueVisibility,
  writeRelayPreference,
} from "./relayPreferences";
import { useAccountIdentityReveal } from "./useAccountIdentityReveal";
import { RelayContext, type Feedback, type RelayContextValue } from "./relayStateContext";
import {
  ERROR_FEEDBACK_TIMEOUT_MS,
  ROUTING_REFRESH_INTERVAL_MS,
  RUNTIME_EVENT_REFRESH_DEBOUNCE_MS,
  RUNTIME_REFRESH_INTERVAL_MS,
  SUCCESS_FEEDBACK_TIMEOUT_MS,
  USAGE_EVENT_REFRESH_DEBOUNCE_MS,
  isRuntimeRefreshPage,
} from "./refreshPolicy";
import { projectRuntimeAccountLabels } from "./runtimeDisplay";
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
  const [localUsage, setLocalUsage] = useState<LocalUsage[]>([]);
  const [localUsagePage, setLocalUsagePage] = useState<LocalUsagePage | null>(null);
  const [remoteUsage, setRemoteUsage] = useState<RemoteUsage[]>([]);
  const [remoteUsagePage, setRemoteUsagePage] = useState<RemoteUsagePage | null>(null);
  const [readyState, setReadyState] = useState<UiState | null>(null);
  const [loading, setLoading] = useState(true);
  const [busy, setBusy] = useState<string | null>(null);
  const [feedback, setFeedback] = useState<Feedback>(null);
  const [onboardingComplete, setOnboardingComplete] = useState(() => readRelayPreference(RELAY_STORAGE_KEYS.onboarding, "0") === "1");
  const [theme, setThemeState] = useState<"system" | "light" | "dark">(() => readRelayPreference(RELAY_STORAGE_KEYS.theme, "system") as "system" | "light" | "dark");
  const [profileSwitchBackupPrompt, setProfileSwitchBackupPromptState] = useState(() => readRelayPreference(RELAY_STORAGE_KEYS.profileSwitchBackupPrompt, "1") !== "0");
  const [codexPoolOauthSelection, setCodexPoolOauthSelectionState] = useState(readCodexPoolOauthSelection);
  const [accountIdentitiesVisible, setAccountIdentitiesVisibleState] = useState(() => readRelayPreference(RELAY_STORAGE_KEYS.accountIdentitiesVisible, "0") === "1");
  const [accountValueVisible, setAccountValueVisibleState] = useState(readAccountValueVisibility);
  const [revealedAccountIdentities, setRevealedAccountIdentities] = useState<Record<string, string>>({});
  const localUsageRequest = useRef(0);
  const remoteUsageRequest = useRef(0);
  const operationRevision = useRef(0);
  const modeRef = useRef(mode);
  const pageRef = useRef(page);
  const stateRevision = useRef(1);
  const refreshedRevision = useRef(0);
  const backgroundRefreshPending = useRef(new Set<RelayMode>());
  const runtimeRefreshPage = useRef<PageId>("overview");
  const modeSwitchStartedAt = useRef<{ mode: RelayMode; startedAt: number } | null>(null);
  const pageOpenStartedAt = useRef<{ page: PageId; startedAt: number } | null>(null);
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
    setRuntime(loaded.snapshot);
    if (requestedMode === "local") {
      setRemoteUsage([]);
      setRemoteUsagePage(null);
    } else if (requestedMode === "remote") {
      setLocalUsage([]);
      setLocalUsagePage(null);
    } else {
      setLocalUsagePage(null);
      setRemoteUsage([]);
      setRemoteUsagePage(null);
    }
    refreshedRevision.current = requestedRevision;
    setRuntimeRevision((current) => current + 1);
  }, [mode]);

  const loadLocalUsage = useCallback(async (query: RemoteUsageQuery) => {
    const request = ++localUsageRequest.current;
    const usage = await relayCommands.localUsagePage(query);
    if (request === localUsageRequest.current) {
      setLocalUsage(usage.events);
      setLocalUsagePage(usage);
    }
    return usage;
  }, []);

  const loadRemoteUsage = useCallback(async (query: RemoteUsageQuery) => {
    const request = ++remoteUsageRequest.current;
    const usage = await relayCommands.remoteUsage(query);
    if (request === remoteUsageRequest.current) {
      setRemoteUsage(usage?.events ?? []);
      setRemoteUsagePage(usage);
    }
    return usage;
  }, []);

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
      .catch((error) => active && setFeedback({ kind: "error", key: "feedback.refreshFailed", error: sanitizeFeedbackError(error, "refresh_failed", t("feedback.refreshFailed")) }))
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
  }, [refresh, t]);

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
        setRuntime((snapshot) => snapshot ? {
          ...snapshot,
          gateway: { ...snapshot.gateway, routingOrder },
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
    if (!feedback) return;
    const timeout = window.setTimeout(
      () => setFeedback(null),
      feedback.kind === "success" ? SUCCESS_FEEDBACK_TIMEOUT_MS : ERROR_FEEDBACK_TIMEOUT_MS,
    );
    return () => window.clearTimeout(timeout);
  }, [feedback]);

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
    operationRevision.current += 1;
    ++localUsageRequest.current;
    ++remoteUsageRequest.current;
    writeRelayPreference(RELAY_STORAGE_KEYS.mode, next);
    setRuntime(null);
    setLocalUsage([]);
    setLocalUsagePage(null);
    setRemoteUsage([]);
    setRemoteUsagePage(null);
    setModeState(next);
    setPage("overview");
    setBusy(null);
    setFeedback(null);
  }, [setPage]);

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

  const perform = useCallback(async (id: string, work: () => Promise<unknown>, successKey?: string) => {
    const revision = ++operationRevision.current;
    setBusy(id);
    setFeedback(null);
    try {
      await work();
      if (revision !== operationRevision.current) return false;
      await refresh();
      if (revision !== operationRevision.current) return false;
      if (successKey) setFeedback({ kind: "success", key: successKey });
      return true;
    } catch (error) {
      if (revision !== operationRevision.current) return false;
      const code = typeof error === "object" && error && "code" in error ? String(error.code) : "general";
      const key = i18n.exists(`errors.${code}`) ? `errors.${code}` : "errors.general";
      setFeedback({ kind: "error", key, error: sanitizeFeedbackError(error, code, t(key)) });
      return false;
    } finally {
      if (revision === operationRevision.current) setBusy(null);
    }
  }, [i18n, refresh, t]);

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

  const finishOnboarding = useCallback((nextMode: RelayMode) => {
    writeRelayPreference(RELAY_STORAGE_KEYS.onboarding, "1");
    setOnboardingComplete(true);
    setMode(nextMode);
  }, [setMode]);

  const resetOnboarding = useCallback(() => {
    removeRelayPreference(RELAY_STORAGE_KEYS.onboarding);
    setOnboardingComplete(false);
    setPage("overview");
  }, []);

  const setTheme = useCallback((next: "system" | "light" | "dark") => {
    writeRelayPreference(RELAY_STORAGE_KEYS.theme, next);
    setThemeState(next);
  }, []);

  const setProfileSwitchBackupPrompt = useCallback((enabled: boolean) => {
    writeRelayPreference(RELAY_STORAGE_KEYS.profileSwitchBackupPrompt, enabled ? "1" : "0");
    setProfileSwitchBackupPromptState(enabled);
  }, []);

  const setCodexPoolOauthSelection = useCallback((selection: string) => {
    writeRelayPreference(RELAY_STORAGE_KEYS.codexPoolOauthSelection, selection);
    removeRelayPreference(RELAY_STORAGE_KEYS.legacyCodexPoolOauthSelection);
    setCodexPoolOauthSelectionState(selection);
  }, []);

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

  const setAccountIdentitiesVisible = useCallback((visible: boolean) => {
    writeRelayPreference(RELAY_STORAGE_KEYS.accountIdentitiesVisible, visible ? "1" : "0");
    setAccountIdentitiesVisibleState(visible);
    if (!visible) setRevealedAccountIdentities({});
  }, []);

  const setAccountValueVisible = useCallback((visible: boolean) => {
    writeAccountValueVisibility(visible);
    setAccountValueVisibleState(visible);
  }, []);

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
  const clearFeedback = useCallback(() => setFeedback(null), []);

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
    localUsage,
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
  }), [mode, setMode, page, displayRuntime, runtimeRevision, usageRevision, accountIdentitiesVisible, accountIdentitiesBusy, canRevealAccountIdentities, setAccountIdentitiesVisible, accountValueVisible, setAccountValueVisible, accountDisplayName, localUsage, localUsagePage, loadLocalUsage, remoteUsage, remoteUsagePage, loadRemoteUsage, readyState, loading, busy, feedback, refresh, perform, activateCodexProfile, launchCodexProfile, clearFeedback, onboardingComplete, finishOnboarding, resetOnboarding, theme, setTheme, profileSwitchBackupPrompt, setProfileSwitchBackupPrompt, codexPoolOauthSelection, setCodexPoolOauthSelection, setCodexBackgroundTasksEnabled, setCodexWebsocketsEnabled]);

  useEffect(() => {
    document.documentElement.lang = i18n.language.startsWith("ru") ? "ru" : "en";
  }, [i18n.language]);

  return <RelayContext.Provider value={value}>{children}</RelayContext.Provider>;
}
