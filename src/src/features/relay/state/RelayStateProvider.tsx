import { createContext, ReactNode, useCallback, useContext, useEffect, useMemo, useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import { getSavedKeyStats, getSavedKeyUsageHistory, getState, KeyStats, UiState, UsageLogEntry } from "../../../tauri";
import { relayCommands } from "../api/commands";
import type { HistoryRepairPreview, LocalUsage, LocalUsagePage, PageId, ProfileActivation, ProfileBinding, RelayMode, RemoteUsage, RemoteUsagePage, RemoteUsageQuery, RuntimeSnapshot } from "../api/types";
import { useConfirm } from "../components/Ui";

type Feedback = { kind: "success" | "error"; key: string } | null;

const RUNTIME_REFRESH_INTERVAL_MS = 60_000;
const ROUTING_REFRESH_INTERVAL_MS = 2_000;
const SUCCESS_FEEDBACK_TIMEOUT_MS = 4_000;
const ERROR_FEEDBACK_TIMEOUT_MS = 8_000;

type RelayContextValue = {
  mode: RelayMode;
  setMode: (mode: RelayMode) => void;
  page: PageId;
  setPage: (page: PageId) => void;
  runtime: RuntimeSnapshot | null;
  localUsage: LocalUsage[];
  localUsagePage: LocalUsagePage | null;
  loadLocalUsage: (query: RemoteUsageQuery) => Promise<void>;
  remoteUsage: RemoteUsage[];
  remoteUsagePage: RemoteUsagePage | null;
  loadRemoteUsage: (query: RemoteUsageQuery) => Promise<void>;
  readyState: UiState | null;
  readyStats: KeyStats | null;
  readyUsage: UsageLogEntry[];
  loading: boolean;
  busy: string | null;
  feedback: Feedback;
  refresh: () => Promise<void>;
  perform: (id: string, work: () => Promise<unknown>, successKey?: string) => Promise<boolean>;
  activateCodexProfile: (id: string, work: () => Promise<ProfileActivation>, launchAfter?: boolean) => Promise<boolean>;
  launchCodexProfile: (binding: ProfileBinding) => Promise<boolean>;
  clearFeedback: () => void;
  onboardingComplete: boolean;
  finishOnboarding: (mode: RelayMode) => void;
  resetOnboarding: () => void;
  theme: "system" | "light" | "dark";
  setTheme: (theme: "system" | "light" | "dark") => void;
  compact: boolean;
  setCompact: (compact: boolean) => void;
  snapshotBeforeSwitch: boolean;
  setSnapshotBeforeSwitch: (enabled: boolean) => void;
  codexPoolOauthSelection: string;
  setCodexPoolOauthSelection: (selection: string) => void;
};

const RelayContext = createContext<RelayContextValue | null>(null);

export function RelayStateProvider({ children }: { children: ReactNode }) {
  const { i18n, t } = useTranslation();
  const confirm = useConfirm();
  const [mode, setModeState] = useState<RelayMode>(() => stored("relay.mode", "local") as RelayMode);
  const [page, setPage] = useState<PageId>("overview");
  const [runtime, setRuntime] = useState<RuntimeSnapshot | null>(null);
  const [localUsage, setLocalUsage] = useState<LocalUsage[]>([]);
  const [localUsagePage, setLocalUsagePage] = useState<LocalUsagePage | null>(null);
  const [remoteUsage, setRemoteUsage] = useState<RemoteUsage[]>([]);
  const [remoteUsagePage, setRemoteUsagePage] = useState<RemoteUsagePage | null>(null);
  const [readyState, setReadyState] = useState<UiState | null>(null);
  const [readyStats, setReadyStats] = useState<KeyStats | null>(null);
  const [readyUsage, setReadyUsage] = useState<UsageLogEntry[]>([]);
  const [loading, setLoading] = useState(true);
  const [busy, setBusy] = useState<string | null>(null);
  const [feedback, setFeedback] = useState<Feedback>(null);
  const [onboardingComplete, setOnboardingComplete] = useState(() => stored("relay.onboarding", "0") === "1");
  const [theme, setThemeState] = useState<"system" | "light" | "dark">(() => stored("relay.theme", "system") as "system" | "light" | "dark");
  const [compact, setCompactState] = useState(() => stored("relay.compact", "0") === "1");
  const [snapshotBeforeSwitch, setSnapshotBeforeSwitchState] = useState(() => stored("relay.snapshotBeforeSwitch", "1") === "1");
  const [codexPoolOauthSelection, setCodexPoolOauthSelectionState] = useState(storedCodexPoolOauthSelection);
  const localUsageRequest = useRef(0);
  const remoteUsageRequest = useRef(0);

  const refresh = useCallback(async () => {
    if (mode === "local") {
      const [snapshot, usagePage] = await Promise.all([
        relayCommands.localState(),
        relayCommands.localUsagePage({ page: 1, pageSize: 100, range: "daily" }).catch(() => null),
      ]);
      setRuntime(snapshot);
      setLocalUsage(usagePage?.events ?? []);
      setLocalUsagePage(usagePage);
      setRemoteUsage([]);
      setRemoteUsagePage(null);
      return;
    }
    if (mode === "remote") {
      const [snapshot, usage] = await Promise.all([
        relayCommands.remoteState(),
        relayCommands.remoteUsage({ page: 1, pageSize: 50, range: "daily" }).catch(() => null),
      ]);
      setRuntime(snapshot);
      setLocalUsage([]);
      setLocalUsagePage(null);
      setRemoteUsage(usage?.events ?? []);
      setRemoteUsagePage(usage);
      return;
    }
    const state = await getState();
    setReadyState(state);
    setRuntime(null);
    setLocalUsagePage(null);
    setRemoteUsage([]);
    setRemoteUsagePage(null);
    if (state.hasSavedApiKey) {
      const [stats, history] = await Promise.all([
        getSavedKeyStats().catch(() => null),
        getSavedKeyUsageHistory().then((value) => value.usage).catch(() => []),
      ]);
      setReadyStats(stats);
      setReadyUsage(history);
    } else {
      setReadyStats(null);
      setReadyUsage([]);
    }
  }, [mode]);

  const loadLocalUsage = useCallback(async (query: RemoteUsageQuery) => {
    const request = ++localUsageRequest.current;
    const usage = await relayCommands.localUsagePage(query);
    if (request !== localUsageRequest.current) return;
    setLocalUsagePage(usage);
  }, []);

  const loadRemoteUsage = useCallback(async (query: RemoteUsageQuery) => {
    const request = ++remoteUsageRequest.current;
    const usage = await relayCommands.remoteUsage(query);
    if (request !== remoteUsageRequest.current) return;
    setRemoteUsage(usage?.events ?? []);
    setRemoteUsagePage(usage);
  }, []);

  useEffect(() => {
    let active = true;
    setLoading(true);
    refresh()
      .catch(() => active && setFeedback({ kind: "error", key: "feedback.refreshFailed" }))
      .finally(() => active && setLoading(false));
    return () => {
      active = false;
    };
  }, [refresh]);

  useEffect(() => {
    if (page !== "pool" || !runtime?.gateway.running || mode === "zenith") return;
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
    const refreshVisibleRuntime = () => {
      if (document.visibilityState === "visible") void refresh().catch(() => undefined);
    };
    const interval = window.setInterval(refreshVisibleRuntime, RUNTIME_REFRESH_INTERVAL_MS);
    window.addEventListener("focus", refreshVisibleRuntime);
    document.addEventListener("visibilitychange", refreshVisibleRuntime);
    return () => {
      window.clearInterval(interval);
      window.removeEventListener("focus", refreshVisibleRuntime);
      document.removeEventListener("visibilitychange", refreshVisibleRuntime);
    };
  }, [refresh]);

  useEffect(() => {
    document.documentElement.dataset.theme = theme;
    document.documentElement.dataset.compact = compact ? "true" : "false";
  }, [theme, compact]);

  useEffect(() => {
    if (!feedback) return;
    const timeout = window.setTimeout(
      () => setFeedback(null),
      feedback.kind === "success" ? SUCCESS_FEEDBACK_TIMEOUT_MS : ERROR_FEEDBACK_TIMEOUT_MS,
    );
    return () => window.clearTimeout(timeout);
  }, [feedback]);

  const setMode = useCallback((next: RelayMode) => {
    localStorage.setItem("relay.mode", next);
    setModeState(next);
    setPage("overview");
    setFeedback(null);
  }, []);

  const perform = useCallback(async (id: string, work: () => Promise<unknown>, successKey?: string) => {
    setBusy(id);
    setFeedback(null);
    try {
      await work();
      await refresh();
      if (successKey) setFeedback({ kind: "success", key: successKey });
      return true;
    } catch (error) {
      const code = typeof error === "object" && error && "code" in error ? String(error.code) : "general";
      setFeedback({ kind: "error", key: `errors.${code}` });
      return false;
    } finally {
      setBusy(null);
    }
  }, [refresh]);

  const launchAttachedCodex = useCallback(() => perform(
    "profile-launch",
    relayCommands.launchManagedCodex,
    "feedback.launched",
  ), [perform]);

  const inspectProfileHistory = useCallback(async (binding: ProfileBinding, launchAfter: boolean) => {
    const result: { current: HistoryRepairPreview | null } = { current: null };
    const target = binding.credentialKind === "local_gateway" ? "zenith_relay_local" : "openai";
    const previewed = await perform("history-repair-preview", async () => {
      result.current = await relayCommands.previewHistoryRepair([binding.profileDir], target);
    });
    const preview = result.current;
    if (!previewed || !preview) return launchAfter ? launchAttachedCodex() : false;
    if (preview.rolloutRecordCount + preview.sqliteRowCount > 0) {
      if (preview.codexRunning && !await perform("history-repair-stop", relayCommands.stopManagedCodex)) return false;
      const repaired = await perform(
        "history-repair-apply",
        () => relayCommands.applyHistoryRepair(preview.sessionId),
        launchAfter ? undefined : "feedback.saved",
      );
      if (!repaired) return false;
    }
    return launchAfter ? launchAttachedCodex() : true;
  }, [launchAttachedCodex, perform]);

  const offerProfileSnapshot = useCallback(async () => {
    if (!snapshotBeforeSwitch || !await confirm(t("profiles.switchSnapshotConfirm"))) return true;
    const date = new Intl.DateTimeFormat(i18n.language, { dateStyle: "short", timeStyle: "short" }).format(new Date());
    return perform(
      "profile-snapshot-switch",
      () => relayCommands.createProfileSnapshot(t("profiles.switchSnapshotName", { date })),
      "feedback.snapshotCreated",
    );
  }, [confirm, i18n.language, perform, snapshotBeforeSwitch, t]);

  const activateCodexProfile = useCallback(async (
    id: string,
    work: () => Promise<ProfileActivation>,
    launchAfter = false,
  ) => {
    if (!await offerProfileSnapshot()) return false;
    const result: { current: ProfileActivation | null } = { current: null };
    const activated = await perform(id, async () => {
      result.current = await work();
    }, launchAfter ? undefined : "feedback.profileAttached");
    const activation = result.current;
    if (!activated || !activation) return false;
    if (activation.repairRecommended || launchAfter) {
      return inspectProfileHistory(activation.binding, launchAfter);
    }
    return true;
  }, [inspectProfileHistory, offerProfileSnapshot, perform]);

  const launchCodexProfile = useCallback(async (binding: ProfileBinding) => {
    const stopped = await perform("profile-stop", relayCommands.stopManagedCodex);
    return stopped && inspectProfileHistory(binding, true);
  }, [inspectProfileHistory, perform]);

  const finishOnboarding = useCallback((nextMode: RelayMode) => {
    localStorage.setItem("relay.onboarding", "1");
    setOnboardingComplete(true);
    setMode(nextMode);
  }, [setMode]);

  const resetOnboarding = useCallback(() => {
    localStorage.removeItem("relay.onboarding");
    setOnboardingComplete(false);
    setPage("overview");
  }, []);

  const setTheme = useCallback((next: "system" | "light" | "dark") => {
    localStorage.setItem("relay.theme", next);
    setThemeState(next);
  }, []);

  const setCompact = useCallback((next: boolean) => {
    localStorage.setItem("relay.compact", next ? "1" : "0");
    setCompactState(next);
  }, []);

  const setSnapshotBeforeSwitch = useCallback((enabled: boolean) => {
    localStorage.setItem("relay.snapshotBeforeSwitch", enabled ? "1" : "0");
    setSnapshotBeforeSwitchState(enabled);
  }, []);

  const setCodexPoolOauthSelection = useCallback((selection: string) => {
    localStorage.setItem("relay.codexPoolOauthSelection", selection);
    localStorage.removeItem("relay.codexPoolOauthAccountId");
    setCodexPoolOauthSelectionState(selection);
  }, []);

  const value = useMemo<RelayContextValue>(() => ({
    mode,
    setMode,
    page,
    setPage,
    runtime,
    localUsage,
    localUsagePage,
    loadLocalUsage,
    remoteUsage,
    remoteUsagePage,
    loadRemoteUsage,
    readyState,
    readyStats,
    readyUsage,
    loading,
    busy,
    feedback,
    refresh,
    perform,
    activateCodexProfile,
    launchCodexProfile,
    clearFeedback: () => setFeedback(null),
    onboardingComplete,
    finishOnboarding,
    resetOnboarding,
    theme,
    setTheme,
    compact,
    setCompact,
    snapshotBeforeSwitch,
    setSnapshotBeforeSwitch,
    codexPoolOauthSelection,
    setCodexPoolOauthSelection,
  }), [mode, setMode, page, runtime, localUsage, localUsagePage, loadLocalUsage, remoteUsage, remoteUsagePage, loadRemoteUsage, readyState, readyStats, readyUsage, loading, busy, feedback, refresh, perform, activateCodexProfile, launchCodexProfile, onboardingComplete, finishOnboarding, resetOnboarding, theme, setTheme, compact, setCompact, snapshotBeforeSwitch, setSnapshotBeforeSwitch, codexPoolOauthSelection, setCodexPoolOauthSelection]);

  useEffect(() => {
    document.documentElement.lang = i18n.language.startsWith("ru") ? "ru" : "en";
  }, [i18n.language]);

  return <RelayContext.Provider value={value}>{children}</RelayContext.Provider>;
}

export function useRelayState() {
  const value = useContext(RelayContext);
  if (!value) throw new Error("RelayStateProvider is missing");
  return value;
}

function stored(key: string, fallback: string) {
  try {
    return localStorage.getItem(key) ?? fallback;
  } catch {
    return fallback;
  }
}

function storedCodexPoolOauthSelection() {
  const selection = stored("relay.codexPoolOauthSelection", "") || stored("relay.codexPoolOauthAccountId", "") || "auto";
  try {
    localStorage.setItem("relay.codexPoolOauthSelection", selection);
    localStorage.removeItem("relay.codexPoolOauthAccountId");
  } catch {
    return selection;
  }
  return selection;
}
