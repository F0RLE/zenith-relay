import { FormEvent, ReactNode, useCallback, useEffect, useRef, useState } from "react";
import { BarChart3, CreditCard, History, Settings } from "lucide-react";
import { useTranslation } from "react-i18next";
import { HistoryPanel } from "./components/HistoryPanel";
import { SettingsPanel } from "./components/SettingsPanel";
import { StatsGrid } from "./components/StatsGrid";
import { TitleBar } from "./components/TitleBar";
import { Toolbar } from "./components/Toolbar";
import { TopUpPanel } from "./components/TopUpPanel";
import {
  createTopUpIntentAndOpen,
  getKeyStats,
  getKeyUsageHistory,
  getPlatform,
  getState,
  KeyStats,
  launchCodex,
  onKeyStatsChanged,
  onStateChanged,
  onUsageHistoryChanged,
  Platform,
  PreparedTopUpAmount,
  prepareTopUpAmount,
  resetKey,
  saveKey,
  updateAndRelaunch,
  UsageLogEntry,
  UiState,
} from "./tauri";
import "./styles.css";

const initialState: UiState = {
  providerActive: false,
  codexRunning: false,
  savedApiKey: "",
};
type AppTab = "stats" | "history" | "topUp" | "settings";
const initialTopUpAmount: PreparedTopUpAmount = {
  amountCents: 0,
  amountUsd: 0,
  valid: false,
};

export function App() {
  const [platform, setPlatform] = useState<Platform>("windows");
  const [state, setState] = useState<UiState>(initialState);
  const [apiKey, setApiKey] = useState("");
  const [keyVisible, setKeyVisible] = useState(false);
  const [saved, setSaved] = useState(false);
  const [busy, setBusy] = useState(false);
  const [updating, setUpdating] = useState(false);
  const [keyStats, setKeyStats] = useState<KeyStats | null>(null);
  const [statsLoading, setStatsLoading] = useState(false);
  const [statsError, setStatsError] = useState(false);
  const [topUpLoading, setTopUpLoading] = useState(false);
  const [topUpError, setTopUpError] = useState(false);
  const [topUpAmount, setTopUpAmount] = useState("25");
  const [preparedTopUpAmount, setPreparedTopUpAmount] = useState(initialTopUpAmount);
  const [activeTab, setActiveTab] = useState<AppTab>("stats");
  const [history, setHistory] = useState<UsageLogEntry[]>([]);
  const [historyLoading, setHistoryLoading] = useState(false);
  const [historyError, setHistoryError] = useState(false);
  const [historyCanLoadMore, setHistoryCanLoadMore] = useState(false);
  const updatingRef = useRef(false);
  const startupUpdateCheckedRef = useRef(false);
  const usageVersionRef = useRef<number | null>(null);
  const { t } = useTranslation();

  const savedKey = state.savedApiKey.trim();
  const editedKey = apiKey.trim();
  const keyAlreadySaved = editedKey.length > 0 && editedKey === savedKey;
  const canSave = editedKey.length > 0 && !keyAlreadySaved && !busy;
  const saveButtonSaved = saved || keyAlreadySaved;
  const canLaunch = state.providerActive && !busy;
  const canReset = (state.providerActive || Boolean(state.savedApiKey)) && !busy;
  const statsKey = state.savedApiKey.trim();
  const hasStatsKey = Boolean(statsKey);

  async function refreshState() {
    const next = await getState();
    setState(next);
    setApiKey((current) => current || next.savedApiKey || "");
    return next;
  }

  async function refreshStats(keyOverride?: string, silent = false) {
    const key = (keyOverride ?? state.savedApiKey).trim();
    if (!key) {
      setKeyStats(null);
      return;
    }

    if (!silent) setStatsLoading(true);
    setStatsError(false);
    try {
      setKeyStats(await getKeyStats(key));
    } catch {
      setStatsError(true);
    } finally {
      if (!silent) setStatsLoading(false);
    }
  }

  async function refreshHistory(keyOverride?: string, sinceId?: number, silent = false) {
    const key = (keyOverride ?? state.savedApiKey).trim();
    if (!key) {
      setHistory([]);
      usageVersionRef.current = null;
      setHistoryCanLoadMore(false);
      setHistoryError(false);
      return;
    }

    if (!silent) setHistoryLoading(true);
    setHistoryError(false);
    try {
      const result = await getKeyUsageHistory(key, sinceId);
      setHistory((current) => (sinceId ? [...current, ...result.usage] : result.usage));
      if (!sinceId) {
        usageVersionRef.current = result.usage[0]?.id ?? null;
      }
      setHistoryCanLoadMore(result.usage.length >= result.limit);
    } catch {
      setHistoryError(true);
    } finally {
      if (!silent) setHistoryLoading(false);
    }
  }

  const installUpdate = useCallback(
    async (automatic = false) => {
      if (updatingRef.current) return;

      updatingRef.current = true;
      setUpdating(true);
      if (!automatic) setBusy(true);
      try {
        const result = await updateAndRelaunch(() => undefined);
        if (result === "none" && !automatic) {
          // No update available when manually triggered
          console.log("No updates available");
        }
      } catch (err) {
        console.error("Update failed:", err);
      } finally {
        updatingRef.current = false;
        setUpdating(false);
        if (!automatic) setBusy(false);
      }
    },
    [],
  );

  useEffect(() => {
    getPlatform().then(setPlatform).catch(() => setPlatform("windows"));
    refreshState()
      .then((next) => {
        refreshStats(next.savedApiKey).catch(() => undefined);
        refreshHistory(next.savedApiKey).catch(() => undefined);
      })
      .catch(() => undefined);

    // Check for updates silently on startup
    if (!startupUpdateCheckedRef.current) {
      startupUpdateCheckedRef.current = true;
      installUpdate(true).catch(() => undefined);
    }
    const unsubscribeState = onStateChanged(() => {
      refreshState()
        .then((next) => {
          refreshStats(next.savedApiKey).catch(() => undefined);
          refreshHistory(next.savedApiKey).catch(() => undefined);
        })
        .catch(() => undefined);
    });
    const unsubscribeStats = onKeyStatsChanged((stats) => {
      setKeyStats(stats);
      setStatsError(false);
    });
    const unsubscribeHistory = onUsageHistoryChanged((result) => {
      setHistoryError(false);
      if (result.usage.length === 0) return;
      setHistory((current) => {
        const seen = new Set(current.map((entry) => entry.id));
        return [...result.usage.filter((entry) => !seen.has(entry.id)), ...current];
      });
      usageVersionRef.current = Math.max(
        usageVersionRef.current ?? 0,
        ...result.usage.map((entry) => entry.id),
      );
    });
    return () => {
      unsubscribeState.then((fn) => fn()).catch(() => undefined);
      unsubscribeStats.then((fn) => fn()).catch(() => undefined);
      unsubscribeHistory.then((fn) => fn()).catch(() => undefined);
    };
  }, [installUpdate]);

  useEffect(() => {
    prepareTopUpAmount(topUpAmount)
      .then(setPreparedTopUpAmount)
      .catch(() => setPreparedTopUpAmount(initialTopUpAmount));
  }, []);

  useEffect(() => {
    if (!hasStatsKey) {
      setKeyStats(null);
      setStatsError(false);
      setHistory([]);
      usageVersionRef.current = null;
      setHistoryCanLoadMore(false);
      return undefined;
    }
    return undefined;
  }, [hasStatsKey, statsKey]);

  async function handleSave(event: FormEvent<HTMLFormElement>) {
    event.preventDefault();
    const nextKey = apiKey.trim();
    if (!nextKey) return;

    setBusy(true);
    setSaved(false);
    try {
      await saveKey(nextKey);
      await refreshState();
      await refreshStats(nextKey);
      await refreshHistory(nextKey);
      setSaved(true);
    } finally {
      setBusy(false);
    }
  }

  async function handleLaunch() {
    if (!state.providerActive) return;

    setBusy(true);
    try {
      await launchCodex();
      await refreshState();
    } finally {
      setBusy(false);
    }
  }

  async function handleReset() {
    if (!canReset) return;

    setBusy(true);
    setSaved(false);
    try {
      await resetKey();
      setApiKey("");
      setKeyStats(null);
      setHistory([]);
      setHistoryCanLoadMore(false);
      await refreshState();
    } finally {
      setBusy(false);
    }
  }

  async function handleTopUpAmountChange(value: string) {
    setTopUpAmount(value);
    try {
      setPreparedTopUpAmount(await prepareTopUpAmount(value));
    } catch {
      setPreparedTopUpAmount({ amountCents: 0, amountUsd: 0, valid: false });
    }
  }

  async function handleTopUp() {
    const key = statsKey;
    if (!key || !preparedTopUpAmount.valid) return;

    setTopUpLoading(true);
    setTopUpError(false);
    try {
      await createTopUpIntentAndOpen(key, preparedTopUpAmount.amountCents);
    } catch {
      setTopUpError(true);
    } finally {
      setTopUpLoading(false);
    }
  }

  return (
    <main className={`app platform-${platform}`} aria-label={t("app.label")}>
      <TitleBar platform={platform} />
      <section className="panel">
        <Toolbar
          apiKey={apiKey}
          canLaunch={canLaunch}
          canSave={canSave}
          codexRunning={state.codexRunning}
          keyVisible={keyVisible}
          saved={saveButtonSaved}
          onApiKeyChange={(value) => {
            setApiKey(value);
            setSaved(false);
          }}
          onKeyVisibleChange={setKeyVisible}
          onLaunch={handleLaunch}
          onSubmit={handleSave}
        />

        <nav className="tabs" aria-label={t("tabs.label")}>
          <TabButton
            active={activeTab === "stats"}
            icon={<BarChart3 aria-hidden />}
            label={t("tabs.stats")}
            onClick={() => setActiveTab("stats")}
          />
          <TabButton
            active={activeTab === "history"}
            icon={<History aria-hidden />}
            label={t("tabs.history")}
            onClick={() => setActiveTab("history")}
          />
          <TabButton
            active={activeTab === "topUp"}
            icon={<CreditCard aria-hidden />}
            label={t("tabs.topUp")}
            onClick={() => setActiveTab("topUp")}
          />
          <TabButton
            active={activeTab === "settings"}
            icon={<Settings aria-hidden />}
            label={t("tabs.settings")}
            onClick={() => setActiveTab("settings")}
          />
        </nav>

        <section className="tab-content">
          {activeTab === "stats" ? <StatsGrid keyStats={keyStats} /> : null}
          {activeTab === "history" ? (
            <HistoryPanel
              entries={history}
              error={historyError}
              loading={historyLoading}
              canLoadMore={historyCanLoadMore}
              onLoadLatest={() => refreshHistory()}
              onLoadMore={() => refreshHistory(undefined, history[history.length - 1]?.id)}
            />
          ) : null}
          {activeTab === "topUp" ? (
            <TopUpPanel
              amount={topUpAmount}
              disabled={!hasStatsKey}
              error={topUpError}
              loading={topUpLoading}
              preparedAmount={preparedTopUpAmount}
              onAmountChange={handleTopUpAmountChange}
              onTopUp={handleTopUp}
            />
          ) : null}
          {activeTab === "settings" ? (
            <SettingsPanel
              canReset={canReset}
              onReset={handleReset}
            />
          ) : null}
        </section>
      </section>
    </main>
  );
}

function TabButton({
  active,
  icon,
  label,
  onClick,
}: {
  active: boolean;
  icon: ReactNode;
  label: string;
  onClick: () => void;
}) {
  return (
    <button className={active ? "active" : ""} type="button" onClick={onClick}>
      {icon}
      <span>{label}</span>
    </button>
  );
}
