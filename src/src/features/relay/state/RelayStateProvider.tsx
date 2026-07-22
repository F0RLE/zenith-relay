import { createContext, ReactNode, useCallback, useContext, useEffect, useMemo, useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import { getState, onStateChanged, setWindowBackgroundColor, type KeyStats, type UiState, type UsageLogEntry } from "../../../tauri";
import { relayCommands } from "../api/commands";
import type { AccountSummary, LocalUsage, LocalUsagePage, PageId, ProfileActivation, ProfileBinding, RelayMode, RemoteUsage, RemoteUsagePage, RemoteUsageQuery, RuntimeSnapshot } from "../api/types";

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
  accountIdentitiesVisible: boolean;
  accountIdentitiesBusy: boolean;
  canRevealAccountIdentities: boolean;
  setAccountIdentitiesVisible: (visible: boolean) => void;
  accountDisplayName: (accountId?: string | null, fallbackLabel?: string | null) => string | null;
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
  codexPoolOauthSelection: string;
  setCodexPoolOauthSelection: (selection: string) => void;
};

const RelayContext = createContext<RelayContextValue | null>(null);

export function RelayStateProvider({ children }: { children: ReactNode }) {
  const { i18n } = useTranslation();
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
  const [codexPoolOauthSelection, setCodexPoolOauthSelectionState] = useState(storedCodexPoolOauthSelection);
  const [accountIdentitiesVisible, setAccountIdentitiesVisibleState] = useState(() => stored("relay.accountIdentitiesVisible", "0") === "1");
  const [accountIdentitiesBusy, setAccountIdentitiesBusy] = useState(false);
  const [revealedAccountIdentities, setRevealedAccountIdentities] = useState<Record<string, string>>({});
  const localUsageRequest = useRef(0);
  const remoteUsageRequest = useRef(0);
  const canRevealAccountIdentities = mode === "local" || (mode === "remote" && Boolean(runtime?.capabilities.features.includes("account_identity_reveal")));
  const revealableAccountIds = canRevealAccountIdentities ? (runtime?.accounts ?? []).filter((account) => account.secretAvailable).map((account) => account.id) : [];
  const revealableAccountSignature = revealableAccountIds.join("\0");

  const refresh = useCallback(async () => {
    if (mode === "local") {
      const request = ++localUsageRequest.current;
      ++remoteUsageRequest.current;
      const usage = relayCommands.localUsagePage({ page: 1, pageSize: 100, range: "daily" }).catch(() => null);
      const snapshot = await relayCommands.localState();
      setRuntime(snapshot);
      setRemoteUsage([]);
      setRemoteUsagePage(null);
      void usage.then((usagePage) => {
        if (request !== localUsageRequest.current || !usagePage) return;
        setLocalUsage(usagePage.events);
        setLocalUsagePage(usagePage);
      });
      return;
    }
    if (mode === "remote") {
      const request = ++remoteUsageRequest.current;
      ++localUsageRequest.current;
      const usage = relayCommands.remoteUsage({ page: 1, pageSize: 50, range: "daily" }).catch(() => null);
      const snapshot = await relayCommands.remoteState();
      setRuntime(snapshot);
      setLocalUsage([]);
      setLocalUsagePage(null);
      void usage.then((usagePage) => {
        if (request !== remoteUsageRequest.current || !usagePage) return;
        setRemoteUsage(usagePage.events);
        setRemoteUsagePage(usagePage);
      });
      return;
    }
    ++localUsageRequest.current;
    ++remoteUsageRequest.current;
    const [state, snapshot] = await Promise.all([getState(), relayCommands.localState()]);
    setReadyState(state);
    setRuntime(snapshot);
    setLocalUsagePage(null);
    setRemoteUsage([]);
    setRemoteUsagePage(null);
    setReadyStats(null);
    setReadyUsage([]);
  }, [mode]);

  const loadLocalUsage = useCallback(async (query: RemoteUsageQuery) => {
    const request = ++localUsageRequest.current;
    const usage = await relayCommands.localUsagePage(query);
    if (request !== localUsageRequest.current) return;
    setLocalUsage(usage.events);
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
      .finally(() => {
        if (!active) return;
        setLoading(false);
        if (performance.getEntriesByName("zenith:interactive", "mark").length) return;
        requestAnimationFrame(() => requestAnimationFrame(() => {
          performance.mark("zenith:interactive");
          performance.measure("zenith:interactive", "zenith:html-start", "zenith:interactive");
        }));
      });
    return () => {
      active = false;
    };
  }, [refresh]);

  useEffect(() => {
    if (!accountIdentitiesVisible) {
      setAccountIdentitiesBusy(false);
      return;
    }
    if (!canRevealAccountIdentities || !revealableAccountSignature) {
      const prefix = `${mode}:`;
      setRevealedAccountIdentities((current) => Object.fromEntries(Object.entries(current).filter(([key]) => !key.startsWith(prefix))));
      setAccountIdentitiesBusy(false);
      return;
    }
    let active = true;
    const accountIds = revealableAccountSignature.split("\0");
    const prefix = `${mode}:`;
    setAccountIdentitiesBusy(true);
    void Promise.allSettled(accountIds.map((accountId) => mode === "local"
      ? relayCommands.revealLocalAccountIdentity(accountId)
      : relayCommands.revealRemoteAccountIdentity(accountId)))
      .then((results) => {
        if (!active) return;
        setRevealedAccountIdentities((current) => {
          const next = Object.fromEntries(Object.entries(current).filter(([key]) => !key.startsWith(prefix)));
          for (const result of results) {
            if (result.status === "fulfilled") next[`${prefix}${result.value.accountId}`] = result.value.identity;
          }
          return next;
        });
      })
      .finally(() => { if (active) setAccountIdentitiesBusy(false); });
    return () => { active = false; };
  }, [accountIdentitiesVisible, canRevealAccountIdentities, mode, revealableAccountSignature]);

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
    let active = true;
    let queued = false;
    let unlisten: (() => void) | undefined;
    void onStateChanged(() => {
      if (!active || queued || document.visibilityState !== "visible") return;
      queued = true;
      window.setTimeout(() => {
        if (!active) return;
        void refresh().catch(() => undefined).finally(() => { queued = false; });
      }, 200);
    }).then((stop) => {
      if (active) unlisten = stop;
      else stop();
    });
    return () => {
      active = false;
      unlisten?.();
    };
  }, [refresh]);

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

  const activateCodexProfile = useCallback(async (
    id: string,
    work: () => Promise<ProfileActivation>,
    launchAfter = false,
  ) => {
    const activated = await perform(id, work, launchAfter ? undefined : "feedback.profileAttached");
    return activated && (!launchAfter || await launchAttachedCodex());
  }, [launchAttachedCodex, perform]);

  const launchCodexProfile = useCallback(async (_binding: ProfileBinding) => {
    const stopped = await perform("profile-stop", relayCommands.stopManagedCodex);
    return stopped && launchAttachedCodex();
  }, [launchAttachedCodex, perform]);

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

  const setCodexPoolOauthSelection = useCallback((selection: string) => {
    localStorage.setItem("relay.codexPoolOauthSelection", selection);
    localStorage.removeItem("relay.codexPoolOauthAccountId");
    setCodexPoolOauthSelectionState(selection);
  }, []);

  const setAccountIdentitiesVisible = useCallback((visible: boolean) => {
    localStorage.setItem("relay.accountIdentitiesVisible", visible ? "1" : "0");
    setAccountIdentitiesVisibleState(visible);
    if (!visible) setRevealedAccountIdentities({});
  }, []);

  const accountDisplayName = useCallback((accountId?: string | null, fallbackLabel?: string | null) => {
    const accounts = runtime?.accounts ?? [];
    const matches = accountId
      ? accounts.filter((account) => account.id === accountId)
      : fallbackLabel ? accounts.filter((account) => account.label === fallbackLabel) : [];
    const account = matches.length === 1 ? matches[0] : null;
    if (!account) return fallbackLabel ?? null;
    return accountIdentitiesVisible && canRevealAccountIdentities && account.secretAvailable ? revealedAccountIdentities[`${mode}:${account.id}`] ?? account.label : account.label;
  }, [accountIdentitiesVisible, canRevealAccountIdentities, mode, revealedAccountIdentities, runtime?.accounts]);

  const displayRuntime = useMemo<RuntimeSnapshot | null>(() => runtime ? {
    ...runtime,
    accounts: runtime.accounts.map((account): AccountSummary => {
      const displayName = accountDisplayName(account.id, account.label) ?? account.label;
      return { ...account, label: displayName, identityHint: displayName };
    }),
  } : null, [accountDisplayName, runtime]);

  const value = useMemo<RelayContextValue>(() => ({
    mode,
    setMode,
    page,
    setPage,
    runtime: displayRuntime,
    accountIdentitiesVisible,
    accountIdentitiesBusy,
    canRevealAccountIdentities,
    setAccountIdentitiesVisible,
    accountDisplayName,
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
    codexPoolOauthSelection,
    setCodexPoolOauthSelection,
  }), [mode, setMode, page, displayRuntime, accountIdentitiesVisible, accountIdentitiesBusy, canRevealAccountIdentities, setAccountIdentitiesVisible, accountDisplayName, localUsage, localUsagePage, loadLocalUsage, remoteUsage, remoteUsagePage, loadRemoteUsage, readyState, readyStats, readyUsage, loading, busy, feedback, refresh, perform, activateCodexProfile, launchCodexProfile, onboardingComplete, finishOnboarding, resetOnboarding, theme, setTheme, codexPoolOauthSelection, setCodexPoolOauthSelection]);

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
