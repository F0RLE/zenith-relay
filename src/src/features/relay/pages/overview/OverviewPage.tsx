import { startTransition, useCallback, useEffect, useMemo, useRef, useState } from "react";
import { Activity, ArrowRight, CircleAlert, CreditCard, Gauge, Play, RefreshCw, Server, Square, Users } from "lucide-react";
import { useTranslation } from "react-i18next";
import { relayCommands } from "../../api/commands";
import type { SourceStats, SourceSummary } from "../../api/types";
import { Button, EmptyState, OptionMenu, PageHeader } from "../../components/Ui";
import { ApplicationPickerDialog } from "../../components/ApplicationPickerDialog";
import { formatProviderMicroUsd } from "../../poolFormatting";
import { useRelayState } from "../../state/RelayStateProvider";
import { useRelayUsageContext } from "../../state/relayStateContext";
import { emptyUsageTotals, formatCompactNumber, formatFullNumber } from "../../usageTotals";
import { sourceHost } from "../../sourceUrl";
import { getCachedOverviewAnalytics, isOverviewAnalyticsFresh, loadOverviewAnalytics } from "./overviewAnalyticsCache";
import { analyticsFromPage, chartWindows, DAY_MS, HOUR_MS, localSamples, remoteSamples, type Analytics, type AnalyticsScope, type Range } from "./overviewAnalyticsModel";
import AnalyticsPanel from "./OverviewAnalytics";

export function OverviewPage() {
  const { t, i18n } = useTranslation();
  const { mode, runtime, setPage, perform, busy } = useRelayState();
  const { revision: usageRevision } = useRelayUsageContext();
  const [applicationDialog, setApplicationDialog] = useState(false);
  const [range, setRange] = useState<Range>("today");
  const [analyticsScopeSelection, setAnalyticsScopeSelection] = useState<AnalyticsScope>(() => {
    const stored = localStorage.getItem("relay.overviewAnalyticsScope");
    return stored?.startsWith("source:") || stored?.startsWith("account:") ? stored as AnalyticsScope : "";
  });
  const [analyticsLoading, setAnalyticsLoading] = useState(false);
  const [analyticsError, setAnalyticsError] = useState(false);
  const locale = i18n.resolvedLanguage ?? i18n.language;
  const now = new Date();
  const calendarDay = `${now.getFullYear()}-${now.getMonth()}-${now.getDate()}`;
  const windows = useMemo(() => chartWindows(range, locale), [range, locale, calendarDay]);
  const usageAvailable = mode !== "remote" || Boolean(runtime?.capabilities.features.includes("usage"));
  const runtimeReady = runtime !== null;
  const windowStartMs = windows[0]?.startMs ?? 0;
  const windowEndMs = windows[windows.length - 1]?.endMs ?? windowStartMs;
  const usageScope = `${mode}:${range}:${analyticsScopeSelection}:${windowStartMs}:${windowEndMs}`;
  const [overviewData, setOverviewData] = useState<Analytics | null>(() => getCachedOverviewAnalytics(usageScope));
  const analyticsScope = useRef<string | null>(null);
  const appliedUsageRevision = useRef(usageRevision);
  const mounted = useRef(false);
  const analyticsScopeQuery = analyticsScopeSelection
    ? analyticsScopeSelection.slice(analyticsScopeSelection.indexOf(":") + 1)
    : undefined;
  const analyticsScopeOptions = useMemo(() => [
    { value: "", label: t("overview.scopeAll") },
    ...(runtime?.sources ?? []).map((source) => ({ value: `source:${source.id}`, label: `${t("overview.scopeApi")} · ${source.name}` })),
    ...(runtime?.accounts ?? []).map((account) => ({ value: `account:${account.id}`, label: `${t("overview.scopeAccount")} · ${account.label}` })),
  ], [runtime?.accounts, runtime?.sources, t]);
  const analytics = overviewData;
  const running = Boolean(runtime?.gateway.running);
  const setAnalyticsScope = useCallback((value: string) => {
    const next = value as AnalyticsScope;
    setAnalyticsScopeSelection(next);
    localStorage.setItem("relay.overviewAnalyticsScope", next);
  }, []);
  const connectOpenCode = async (launchAfterConnect: boolean) => {
    const connected = await perform("opencode-connect", relayCommands.connectOpenCode, "feedback.saved");
    if (connected && launchAfterConnect) await perform("opencode-launch", relayCommands.restartOpenCode, "feedback.launched");
  };

  useEffect(() => {
    if (!runtime) return;
    if (analyticsScopeSelection && !analyticsScopeOptions.some((option) => option.value === analyticsScopeSelection)) {
      setAnalyticsScopeSelection("");
      localStorage.removeItem("relay.overviewAnalyticsScope");
    }
  }, [analyticsScopeOptions, analyticsScopeSelection, runtime]);

  useEffect(() => {
    mounted.current = true;
    return () => { mounted.current = false; };
  }, []);

  useEffect(() => {
    let active = true;
    const scopeChanged = analyticsScope.current !== usageScope;
    analyticsScope.current = usageScope;
    if (scopeChanged) {
      // Re-entry can reuse the last complete snapshot immediately. The same
      // scope is refreshed below, so the cached value is only a stale view.
      const cached = getCachedOverviewAnalytics(usageScope);
      if (cached) startTransition(() => setOverviewData(cached));
    }
    setAnalyticsError(false);
    const stopLoading = () => {
      setAnalyticsLoading(false);
    };
    if (mode === "zenith") {
      stopLoading();
      return () => { active = false; };
    }
    if (!runtime) {
      stopLoading();
      return () => { active = false; };
    }
    if (!usageAvailable) {
      startTransition(() => setOverviewData(null));
      stopLoading();
      return () => { active = false; };
    }
    const usageChanged = appliedUsageRevision.current !== usageRevision;
    appliedUsageRevision.current = usageRevision;
    if (!usageChanged && isOverviewAnalyticsFresh(usageScope)) {
      stopLoading();
      return () => { active = false; };
    }
    setAnalyticsLoading(true);
    const input = {
      page: 1,
      pageSize: 1,
      range: "custom" as const,
      fromMs: windowStartMs,
      toMs: windowEndMs,
      bucketMs: range === "today" ? HOUR_MS : DAY_MS,
      includeEvents: false,
      includeModels: false,
      includePoolMembers: false,
      ...(analyticsScopeQuery ? { sourceOrAccountQuery: analyticsScopeQuery } : {}),
    };
    const load = () => mode === "local"
      ? relayCommands.localUsagePage(input).then((page) => analyticsFromPage(page.totals, page.buckets, localSamples(page.events), windows))
      : relayCommands.remoteUsage(input).then((page) => page ? analyticsFromPage(page.totals, page.buckets, remoteSamples(page.events), windows) : null);
    loadOverviewAnalytics(usageScope, load)
      .then((result) => {
        if (!result) {
          if (active) setAnalyticsError(true);
          return;
        }
        if (!active) return;
        startTransition(() => setOverviewData(result));
      })
      .catch(() => active && setAnalyticsError(true))
      .finally(() => {
        // A same-scope refresh may supersede this effect while its request is still in flight.
        if (mounted.current && analyticsScope.current === usageScope) setAnalyticsLoading(false);
      });
    return () => { active = false; };
  }, [mode, range, runtimeReady, usageRevision, usageAvailable, usageScope, analyticsScopeQuery, windowEndMs, windowStartMs, windows]);

  if (mode === "zenith") return <DirectApiOverview sources={runtime?.sources ?? []} onOpen={() => setPage("connections")} perform={perform} />;

  const totals = analytics?.totals ?? emptyUsageTotals();
  const requests = totals.requests;
  const models = runtime?.gateway.visibleModelIds.length ?? 0;
  const healthy = [...(runtime?.sources ?? []), ...(runtime?.accounts ?? [])].filter((item) => item.enabled).length;
  const errors = Math.max(0, totals.requests - totals.successfulRequests);

  const primary = mode === "local" ? <><Button variant="primary" busy={busy === "gateway"} icon={running ? <Square aria-hidden /> : <Play aria-hidden />} onClick={() => perform("gateway", () => running ? relayCommands.stopGateway() : relayCommands.startGateway(), running ? "feedback.stopped" : "feedback.started")}>{running ? t("gateway.stop") : t("gateway.start")}</Button><Button variant="secondary" busy={busy === "chatgpt-launch" || busy === "opencode-connect" || busy === "opencode-launch"} icon={<Play aria-hidden />} disabled={!running} title={!running ? t("gateway.start") : t("overview.launchApplication")} onClick={() => setApplicationDialog(true)}>{t("overview.launchApplication")}</Button></> : <Button variant="primary" icon={<Server aria-hidden />} onClick={() => setPage("connections")}>{runtime ? t("overview.openServer") : t("remote.connect")}</Button>;

  return <section className="relay-page"><PageHeader title={t("nav.overview")} subtitle={t(`overview.subtitles.${mode}`)} actions={primary} />
    {!running && !runtime ? <EmptyState title={t("overview.emptyTitle")} description={t("overview.emptyDescription")} action={<Button variant="primary" onClick={() => setPage("connections")}>{t("overview.openConnections")}</Button>} /> : <>
       <div className="metric-band overview-metrics"><div><Activity aria-hidden /><span>{t("overview.requestsToday")}</span><strong>{formatCompactNumber(requests, locale)}</strong></div><div><Users aria-hidden /><span>{t("overview.healthy")}</span><strong>{healthy}</strong></div><div><ArrowRight aria-hidden /><span>{t("overview.models")}</span><strong>{models || "-"}</strong></div><div><CircleAlert aria-hidden /><span>{t("overview.errors")}</span><strong>{formatCompactNumber(errors, locale)}</strong></div></div><AnalyticsPanel range={range} setRange={setRange} windows={windows} analytics={analytics} loading={analyticsLoading} error={analyticsError} scope={analyticsScopeSelection} setScope={setAnalyticsScope} scopeOptions={analyticsScopeOptions} />
    </>}
    {applicationDialog ? <ApplicationPickerDialog title={t("overview.applicationPickerTitle")} onClose={() => setApplicationDialog(false)} onChatGPT={(launchAfterConnect) => { if (launchAfterConnect) void perform("chatgpt-launch", relayCommands.launchManagedCodex, "feedback.launched"); }} onOpenCode={(launchAfterConnect) => void connectOpenCode(launchAfterConnect)} /> : null}
  </section>;
}
function DirectApiOverview({ sources, onOpen, perform }: { sources: SourceSummary[]; onOpen: () => void; perform: (id: string, work: () => Promise<unknown>, successKey?: string) => Promise<boolean> }) {
  const { t, i18n } = useTranslation();
  const { busy } = useRelayState();
  const [selection, setSelection] = useState(() => localStorage.getItem("relay.directSourceId") ?? "");
  const [stats, setStats] = useState<SourceStats | null>(null);
  const [statsLoading, setStatsLoading] = useState(false);
  const [statsError, setStatsError] = useState(false);
  const [statsRevision, setStatsRevision] = useState(0);
  const lastStatsSourceId = useRef<string | null>(null);
  const source = sources.find((item) => item.id === selection) ?? sources[0] ?? null;

  useEffect(() => {
    if (!source) {
      lastStatsSourceId.current = null;
      setStats(null);
      setStatsError(false);
      setStatsLoading(false);
      return;
    }
    let active = true;
    const sourceChanged = lastStatsSourceId.current !== source.id;
    lastStatsSourceId.current = source.id;
    if (sourceChanged) setStats(null);
    setStatsError(false);
    setStatsLoading(true);
    void relayCommands.localSourceStats(source.id)
      .then((value) => { if (active) setStats(value); })
      .catch(() => { if (active) setStatsError(true); })
      .finally(() => { if (active) setStatsLoading(false); });
    return () => { active = false; };
  }, [source?.id, statsRevision]);

  const select = (sourceId: string) => {
    localStorage.setItem("relay.directSourceId", sourceId);
    setSelection(sourceId);
  };
  const refreshSourceData = async () => {
    if (!source) return;
    await perform("source-data-refresh", () => relayCommands.refreshSourceData(source.id), "feedback.refreshed");
    setStatsRevision((value) => value + 1);
  };
  const locale = i18n.resolvedLanguage ?? i18n.language;
  const display = (value: string | null | undefined) => value || (statsLoading ? "…" : "—");
  const money = (value: number | null | undefined) => value == null ? null : formatProviderMicroUsd(value, locale);
  const requests = stats?.requests == null ? null : formatFullNumber(stats.requests, locale);
  const totalTokens = stats?.totalTokens == null ? null : formatFullNumber(stats.totalTokens, locale);
  const sourceRefreshBusy = busy === "source-data-refresh";
  const actions = <><Button variant="secondary" icon={<RefreshCw aria-hidden />} busy={statsLoading || sourceRefreshBusy} disabled={!source || sourceRefreshBusy} onClick={() => void refreshSourceData()}>{t("common.refresh")}</Button><Button variant="primary" icon={<ArrowRight aria-hidden />} onClick={onOpen}>{t("overview.openConnections")}</Button></>;

  return <section className="relay-page"><PageHeader title={t("nav.overview")} subtitle={t("overview.subtitles.zenith")} actions={actions} />
    {!source ? <EmptyState title={t("sources.emptyTitle")} description={t("sources.emptyDescription")} action={<Button variant="primary" onClick={onOpen}>{t("sources.add")}</Button>} /> : <div className="direct-api-overview">
      <div className="direct-api-toolbar"><div><strong>{source.name}</strong><code>{source.baseUrl}</code></div><OptionMenu className="direct-api-source-menu" label={t("overview.selectedSource")} value={source.id} onChange={select} options={sources.map((item) => ({ value: item.id, label: `${item.name} · ${sourceHost(item.baseUrl)}` }))} /></div>
      <div className="metric-band direct-api-metrics"><div><CreditCard aria-hidden /><span>{t("overview.balance")}</span><strong>{display(money(stats?.balanceMicroUsd))}</strong></div><div><Activity aria-hidden /><span>{t("usage.requests")}</span><strong>{display(requests)}</strong></div><div><ArrowRight aria-hidden /><span>{t("overview.spent")}</span><strong>{display(money(stats?.spentMicroUsd))}</strong></div><div><Gauge aria-hidden /><span>{t("overview.totalTokens")}</span><strong>{display(totalTokens)}</strong></div></div>
      {statsError ? <p className="direct-api-stats-note error-text" role="alert">{t("overview.sourceStatsUnavailable")}</p> : stats?.provider === "unsupported" ? <p className="direct-api-stats-note">{t("overview.sourceStatsUnsupported")}</p> : null}
      <section className="direct-api-models"><header><div><h2>{t("overview.availableModels")}</h2><p>{t("overview.availableModelsHint")}</p></div><strong>{source.models.length}</strong></header><ul>{source.models.map((model) => <li key={model}><code>{model}</code></li>)}</ul></section>
    </div>}
  </section>;
}
