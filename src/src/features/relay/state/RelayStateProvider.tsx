import { createContext, ReactNode, useCallback, useContext, useEffect, useMemo, useState } from "react";
import { useTranslation } from "react-i18next";
import { getSavedKeyStats, getSavedKeyUsageHistory, getState, KeyStats, UiState, UsageLogEntry } from "../../../tauri";
import { relayCommands } from "../api/commands";
import type { LocalUsage, PageId, RelayMode, RemoteUsage, RuntimeSnapshot } from "../api/types";

type Feedback = { kind: "success" | "error"; key: string } | null;

type RelayContextValue = {
  mode: RelayMode;
  setMode: (mode: RelayMode) => void;
  page: PageId;
  setPage: (page: PageId) => void;
  runtime: RuntimeSnapshot | null;
  localUsage: LocalUsage[];
  remoteUsage: RemoteUsage[];
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
};

const RelayContext = createContext<RelayContextValue | null>(null);

export function RelayStateProvider({ children }: { children: ReactNode }) {
  const { i18n } = useTranslation();
  const [mode, setModeState] = useState<RelayMode>(() => stored("relay.mode", "local") as RelayMode);
  const [page, setPage] = useState<PageId>("overview");
  const [runtime, setRuntime] = useState<RuntimeSnapshot | null>(null);
  const [localUsage, setLocalUsage] = useState<LocalUsage[]>([]);
  const [remoteUsage, setRemoteUsage] = useState<RemoteUsage[]>([]);
  const [readyState, setReadyState] = useState<UiState | null>(null);
  const [readyStats, setReadyStats] = useState<KeyStats | null>(null);
  const [readyUsage, setReadyUsage] = useState<UsageLogEntry[]>([]);
  const [loading, setLoading] = useState(true);
  const [busy, setBusy] = useState<string | null>(null);
  const [feedback, setFeedback] = useState<Feedback>(null);
  const [onboardingComplete, setOnboardingComplete] = useState(() => stored("relay.onboarding", "0") === "1");
  const [theme, setThemeState] = useState<"system" | "light" | "dark">(() => stored("relay.theme", "system") as "system" | "light" | "dark");
  const [compact, setCompactState] = useState(() => stored("relay.compact", "0") === "1");

  const refresh = useCallback(async () => {
    if (mode === "local") {
      const [snapshot, usage] = await Promise.all([
        relayCommands.localState(),
        relayCommands.localUsage(100).catch(() => []),
      ]);
      setRuntime(snapshot);
      setLocalUsage(usage);
      setRemoteUsage([]);
      return;
    }
    if (mode === "remote") {
      const [snapshot, usage] = await Promise.all([
        relayCommands.remoteState(),
        relayCommands.remoteUsage().catch(() => null),
      ]);
      setRuntime(snapshot);
      setLocalUsage([]);
      setRemoteUsage(usage?.events ?? []);
      return;
    }
    const state = await getState();
    setReadyState(state);
    setRuntime(null);
    setRemoteUsage([]);
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

  const value = useMemo<RelayContextValue>(() => ({
    mode,
    setMode,
    page,
    setPage,
    runtime,
    localUsage,
    remoteUsage,
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
  }), [mode, setMode, page, runtime, localUsage, remoteUsage, readyState, readyStats, readyUsage, loading, busy, feedback, refresh, perform, onboardingComplete, finishOnboarding, resetOnboarding, theme, setTheme, compact, setCompact]);

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
