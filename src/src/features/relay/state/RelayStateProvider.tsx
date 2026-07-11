import { createContext, ReactNode, useCallback, useContext, useEffect, useMemo, useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import { getSavedKeyStats, getSavedKeyUsageHistory, getState, KeyStats, UiState, UsageLogEntry } from "../../../tauri";
import { relayCommands } from "../api/commands";
import type { LocalUsage, PageId, QuotaWindowVisibility, RelayMode, RemoteUsage, RemoteUsagePage, RemoteUsageQuery, RuntimeSnapshot } from "../api/types";

type Feedback = { kind: "success" | "error"; key: string } | null;

type RelayContextValue = {
  mode: RelayMode;
  setMode: (mode: RelayMode) => void;
  page: PageId;
  setPage: (page: PageId) => void;
  runtime: RuntimeSnapshot | null;
  localUsage: LocalUsage[];
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
  clearFeedback: () => void;
  onboardingComplete: boolean;
  finishOnboarding: (mode: RelayMode) => void;
  resetOnboarding: () => void;
  theme: "system" | "light" | "dark";
  setTheme: (theme: "system" | "light" | "dark") => void;
  compact: boolean;
  setCompact: (compact: boolean) => void;
  quotaWindows: QuotaWindowVisibility;
  setQuotaWindowVisible: (kind: keyof QuotaWindowVisibility, visible: boolean) => void;
};

const RelayContext = createContext<RelayContextValue | null>(null);

export function RelayStateProvider({ children }: { children: ReactNode }) {
  const { i18n } = useTranslation();
  const [mode, setModeState] = useState<RelayMode>(() => stored("relay.mode", "local") as RelayMode);
  const [page, setPage] = useState<PageId>("overview");
  const [runtime, setRuntime] = useState<RuntimeSnapshot | null>(null);
  const [localUsage, setLocalUsage] = useState<LocalUsage[]>([]);
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
  const [quotaWindows, setQuotaWindows] = useState<QuotaWindowVisibility>(() => ({
    primary: stored("relay.quota.primary", "1") !== "0",
    secondary: stored("relay.quota.secondary", "1") !== "0",
  }));
  const remoteUsageRequest = useRef(0);

  const refresh = useCallback(async () => {
    if (mode === "local") {
      const [snapshot, usage] = await Promise.all([
        relayCommands.localState(),
        relayCommands.localUsage(100).catch(() => []),
      ]);
      setRuntime(snapshot);
      setLocalUsage(usage);
      setRemoteUsage([]);
      setRemoteUsagePage(null);
      return;
    }
    if (mode === "remote") {
      const [snapshot, usage] = await Promise.all([
        relayCommands.remoteState(),
        relayCommands.remoteUsage({ page: 1, pageSize: 50 }).catch(() => null),
      ]);
      setRuntime(snapshot);
      setLocalUsage([]);
      setRemoteUsage(usage?.events ?? []);
      setRemoteUsagePage(usage);
      return;
    }
    const state = await getState();
    setReadyState(state);
    setRuntime(null);
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
    document.documentElement.dataset.theme = theme;
    document.documentElement.dataset.compact = compact ? "true" : "false";
  }, [theme, compact]);

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

  const setQuotaWindowVisible = useCallback((kind: keyof QuotaWindowVisibility, visible: boolean) => {
    setQuotaWindows((current) => {
      const next = { ...current, [kind]: visible };
      if (!next.primary && !next.secondary) return current;
      localStorage.setItem(`relay.quota.${kind}`, visible ? "1" : "0");
      return next;
    });
  }, []);

  const value = useMemo<RelayContextValue>(() => ({
    mode,
    setMode,
    page,
    setPage,
    runtime,
    localUsage,
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
    clearFeedback: () => setFeedback(null),
    onboardingComplete,
    finishOnboarding,
    resetOnboarding,
    theme,
    setTheme,
    compact,
    setCompact,
    quotaWindows,
    setQuotaWindowVisible,
  }), [mode, setMode, page, runtime, localUsage, remoteUsage, remoteUsagePage, loadRemoteUsage, readyState, readyStats, readyUsage, loading, busy, feedback, refresh, perform, onboardingComplete, finishOnboarding, resetOnboarding, theme, setTheme, compact, setCompact, quotaWindows, setQuotaWindowVisible]);

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
