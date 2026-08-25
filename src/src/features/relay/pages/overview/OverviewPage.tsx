import { lazy, startTransition, Suspense, useEffect, useMemo, useRef, useState } from "react";
import { Activity, ArrowRight, CircleAlert, CreditCard, Gauge, Play, RefreshCw, Server, Square, Users } from "lucide-react";
import { useTranslation } from "react-i18next";
import { relayCommands } from "../../api/commands";
import type { LocalUsage, RemoteUsage, SourceStats, SourceSummary, UsageBucket, UsageTotals } from "../../api/types";
import { Button, EmptyState, OptionMenu, PageHeader } from "../../components/Ui";
import { formatProviderMicroUsd } from "../../poolFormatting";
import { useRelayState } from "../../state/RelayStateProvider";
import { emptyUsageTotals, formatCompactNumber } from "../../usageTotals";

const AnalyticsPanel = lazy(() => import("./OverviewAnalytics"));

type Range = "today" | "week" | "month";
type WindowBucket = { startMs: number; endMs: number; label: string; fullLabel: string; showLabel: boolean };
type Analytics = { totals: UsageTotals; buckets: UsageBucket[] };
type OverviewData = { analytics: Analytics };
type UsageSample = {
  createdAtMs: number;
  success: boolean;
  latencyMs: number;
  ttftMs: number | null;
  generationMs: number | null;
  inputTokens: number | null;
  cachedInputTokens: number | null;
  cacheWriteInputTokens?: number | null;
  reasoningTokens: number | null;
  outputTokens: number | null;
  totalTokens: number | null;
  apiEquivalentMicroUsd?: number | null;
};

const HOUR_MS = 60 * 60 * 1_000;
const DAY_MS = 24 * HOUR_MS;

export function OverviewPage() {
  const { t, i18n } = useTranslation();
  const { mode, runtime, runtimeRevision, setPage, perform, busy } = useRelayState();
  const [range, setRange] = useState<Range>("today");
  const [overviewData, setOverviewData] = useState<OverviewData | null>(null);
  const [analyticsLoading, setAnalyticsLoading] = useState(false);
  const [analyticsError, setAnalyticsError] = useState(false);
  const [chartsReady, setChartsReady] = useState(false);
  const analyticsScope = useRef<string | null>(null);
  const pendingAnalyticsRequests = useRef(new Map<string, Set<number>>());
  const nextAnalyticsRequestId = useRef(0);
  const mounted = useRef(false);
  const locale = i18n.resolvedLanguage ?? i18n.language;
  const now = new Date();
  const calendarDay = `${now.getFullYear()}-${now.getMonth()}-${now.getDate()}`;
  const windows = useMemo(() => chartWindows(range, locale), [range, locale, calendarDay]);
  const usageAvailable = mode !== "remote" || Boolean(runtime?.capabilities.features.includes("usage"));
  const windowStartMs = windows[0].startMs;
  const windowEndMs = windows[windows.length - 1].endMs;
  const usageScope = `${mode}:${range}:${windowStartMs}:${windowEndMs}`;
  const analytics = overviewData?.analytics ?? null;
  const running = Boolean(runtime?.gateway.running);

  useEffect(() => {
    mounted.current = true;
    return () => { mounted.current = false; };
  }, []);

  useEffect(() => {
    let secondFrame = 0;
    const firstFrame = requestAnimationFrame(() => {
      secondFrame = requestAnimationFrame(() => setChartsReady(true));
    });
    return () => {
      cancelAnimationFrame(firstFrame);
      cancelAnimationFrame(secondFrame);
    };
  }, []);

  useEffect(() => {
    let active = true;
    const scopeChanged = analyticsScope.current !== usageScope;
    analyticsScope.current = usageScope;
    if (scopeChanged) startTransition(() => setOverviewData(null));
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
    const pending = pendingAnalyticsRequests.current;
    const requestId = nextAnalyticsRequestId.current++;
    const scopeRequests = pending.get(usageScope) ?? new Set<number>();
    scopeRequests.add(requestId);
    pending.set(usageScope, scopeRequests);
    setAnalyticsLoading(true);
    const input = { page: 1, pageSize: 5, range: "custom" as const, fromMs: windowStartMs, toMs: windowEndMs, bucketMs: range === "today" ? HOUR_MS : DAY_MS };
    const request = mode === "local"
      ? relayCommands.localUsagePage(input).then((page) => ({ analytics: analyticsFromPage(page.totals, page.buckets, localSamples(page.events), windows) }))
      : relayCommands.remoteUsage(input).then((page) => page ? { analytics: analyticsFromPage(page.totals, page.buckets, remoteSamples(page.events), windows) } : null);
    request
      .then((result) => {
        if (!active) return;
        if (!result) {
          setAnalyticsError(true);
          return;
        }
        startTransition(() => setOverviewData(result));
      })
      .catch(() => active && setAnalyticsError(true))
      .finally(() => {
        const requests = pending.get(usageScope);
        requests?.delete(requestId);
        const remaining = requests?.size ?? 0;
        if (!remaining) pending.delete(usageScope);
        // A same-scope refresh may supersede this effect while its request is still in flight.
        if (mounted.current && analyticsScope.current === usageScope) setAnalyticsLoading(remaining > 0);
      });
    return () => { active = false; };
  }, [mode, range, runtimeRevision, usageAvailable, usageScope, windowEndMs, windowStartMs, windows]);

  if (mode === "zenith") return <DirectApiOverview sources={runtime?.sources ?? []} onOpen={() => setPage("connections")} />;

  const totals = analytics?.totals ?? emptyUsageTotals();
  const requests = totals.requests;
  const models = runtime?.gateway.visibleModelIds.length ?? 0;
  const healthy = [...(runtime?.sources ?? []), ...(runtime?.accounts ?? [])].filter((item) => item.enabled).length;
  const errors = Math.max(0, totals.requests - totals.successfulRequests);

  const primary = mode === "local" ? <Button variant="primary" busy={busy === "gateway"} icon={running ? <Square aria-hidden /> : <Play aria-hidden />} onClick={() => perform("gateway", () => running ? relayCommands.stopGateway() : relayCommands.startGateway(), running ? "feedback.stopped" : "feedback.started")}>{running ? t("gateway.stop") : t("gateway.start")}</Button> : <Button variant="primary" icon={<Server aria-hidden />} onClick={() => setPage("connections")}>{runtime ? t("overview.openServer") : t("remote.connect")}</Button>;

  return <section className="relay-page"><PageHeader title={t("nav.overview")} subtitle={t(`overview.subtitles.${mode}`)} actions={primary} />
    {!running && !runtime ? <EmptyState title={t("overview.emptyTitle")} description={t("overview.emptyDescription")} action={<Button variant="primary" onClick={() => setPage("connections")}>{t("overview.openConnections")}</Button>} /> : <>
      <div className="metric-band overview-metrics"><div><Activity aria-hidden /><span>{t("overview.requestsToday")}</span><strong>{formatCompactNumber(requests, locale)}</strong></div><div><Users aria-hidden /><span>{t("overview.healthy")}</span><strong>{healthy}</strong></div><div><ArrowRight aria-hidden /><span>{t("overview.models")}</span><strong>{models || "-"}</strong></div><div><CircleAlert aria-hidden /><span>{t("overview.errors")}</span><strong>{formatCompactNumber(errors, locale)}</strong></div></div>{chartsReady ? <Suspense fallback={<section className="overview-analytics loading" aria-busy="true"><div className="relay-loading">{t("common.loading")}</div></section>}><AnalyticsPanel range={range} setRange={setRange} windows={windows} analytics={analytics} loading={analyticsLoading} error={analyticsError} /></Suspense> : <section className="overview-analytics loading" aria-busy="true"><div className="relay-loading">{t("common.loading")}</div></section>}
    </>}
  </section>;
}

function DirectApiOverview({ sources, onOpen }: { sources: SourceSummary[]; onOpen: () => void }) {
  const { t, i18n } = useTranslation();
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
  const locale = i18n.resolvedLanguage ?? i18n.language;
  const display = (value: string | null | undefined) => value || (statsLoading ? "…" : "—");
  const money = (value: number | null | undefined) => value == null ? null : formatProviderMicroUsd(value, locale);
  const requests = stats?.requests == null ? null : new Intl.NumberFormat(locale).format(stats.requests);
  const totalTokens = stats?.totalTokens == null ? null : new Intl.NumberFormat(locale).format(stats.totalTokens);
  const actions = <><Button variant="secondary" icon={<RefreshCw aria-hidden />} busy={statsLoading} disabled={!source} onClick={() => setStatsRevision((value) => value + 1)}>{t("common.refresh")}</Button><Button variant="primary" icon={<ArrowRight aria-hidden />} onClick={onOpen}>{t("overview.openConnections")}</Button></>;

  return <section className="relay-page"><PageHeader title={t("nav.overview")} subtitle={t("overview.subtitles.zenith")} actions={actions} />
    {!source ? <EmptyState title={t("sources.emptyTitle")} description={t("sources.emptyDescription")} action={<Button variant="primary" onClick={onOpen}>{t("sources.add")}</Button>} /> : <div className="direct-api-overview">
      <div className="direct-api-toolbar"><div><strong>{source.name}</strong><code>{source.baseUrl}</code></div><OptionMenu className="direct-api-source-menu" label={t("overview.selectedSource")} value={source.id} onChange={select} options={sources.map((item) => ({ value: item.id, label: `${item.name} · ${sourceHost(item.baseUrl)}` }))} /></div>
      <div className="metric-band direct-api-metrics"><div><CreditCard aria-hidden /><span>{t("overview.balance")}</span><strong>{display(money(stats?.balanceMicroUsd))}</strong></div><div><Activity aria-hidden /><span>{t("usage.requests")}</span><strong>{display(requests)}</strong></div><div><ArrowRight aria-hidden /><span>{t("overview.spent")}</span><strong>{display(money(stats?.spentMicroUsd))}</strong></div><div><Gauge aria-hidden /><span>{t("overview.totalTokens")}</span><strong>{display(totalTokens)}</strong></div></div>
      {statsError ? <p className="direct-api-stats-note error-text" role="alert">{t("overview.sourceStatsUnavailable")}</p> : stats?.provider === "unsupported" ? <p className="direct-api-stats-note">{t("overview.sourceStatsUnsupported")}</p> : null}
      <section className="direct-api-models"><header><div><h2>{t("overview.availableModels")}</h2><p>{t("overview.availableModelsHint")}</p></div><strong>{source.models.length}</strong></header><ul>{source.models.map((model) => <li key={model}><code>{model}</code></li>)}</ul></section>
    </div>}
  </section>;
}

function chartWindows(range: Range, locale: string, now = new Date()): WindowBucket[] {
  const today = new Date(now.getFullYear(), now.getMonth(), now.getDate());
  const count = range === "today" ? 24 : range === "week" ? 7 : 30;
  const bucketMs = range === "today" ? HOUR_MS : DAY_MS;
  const startMs = today.getTime() - (range === "today" ? 0 : (count - 1) * DAY_MS);
  const hour = new Intl.DateTimeFormat(locale, { hour: "2-digit", hourCycle: "h23" });
  const weekday = new Intl.DateTimeFormat(locale, { weekday: "short" });
  const day = new Intl.DateTimeFormat(locale, { day: "numeric" });
  const full = new Intl.DateTimeFormat(locale, range === "today" ? { day: "numeric", month: "short", hour: "2-digit", minute: "2-digit", hourCycle: "h23" } : { day: "numeric", month: "long" });
  return Array.from({ length: count }, (_, index) => {
    const bucketStart = startMs + index * bucketMs;
    const date = new Date(bucketStart);
    return {
      startMs: bucketStart,
      endMs: bucketStart + bucketMs - 1,
      label: range === "today" ? hour.format(date) : range === "week" ? weekday.format(date) : day.format(date),
      fullLabel: full.format(date),
      showLabel: range === "week" || index % (range === "today" ? 4 : 5) === 0 || index === count - 1,
    };
  });
}

function analyticsFromPage(totals: UsageTotals | undefined, buckets: UsageBucket[] | undefined, samples: UsageSample[], windows: WindowBucket[]): Analytics {
  return { totals: totals ?? totalsFromSamples(samples), buckets: buckets?.length ? buckets : bucketsFromSamples(windows, samples) };
}

function bucketsFromSamples(windows: WindowBucket[], samples: UsageSample[]) {
  return windows.map((window) => ({ startMs: window.startMs, totals: totalsFromSamples(samples.filter((sample) => sample.createdAtMs >= window.startMs && sample.createdAtMs <= window.endMs)) }));
}

function totalsFromSamples(samples: UsageSample[]) {
  return samples.reduce<UsageTotals>((totals, sample) => {
    const outputTokens = sample.success ? Math.max(0, sample.outputTokens ?? 0) : 0;
    totals.requests += 1;
    totals.successfulRequests += Number(sample.success);
    totals.latencyMs += sample.latencyMs;
    if (sample.ttftMs != null) { totals.ttftMs += sample.ttftMs; totals.ttftSamples += 1; }
    totals.inputTokens += sample.inputTokens ?? 0;
    if (sample.cachedInputTokens != null) { totals.cachedInputTokens += sample.cachedInputTokens; totals.cachedInputSamples += 1; }
    if (sample.cacheWriteInputTokens != null) { totals.cacheWriteInputTokens = (totals.cacheWriteInputTokens ?? 0) + sample.cacheWriteInputTokens; totals.cacheWriteInputSamples = (totals.cacheWriteInputSamples ?? 0) + 1; }
    totals.reasoningTokens += sample.reasoningTokens ?? 0;
    totals.outputTokens += sample.outputTokens ?? 0;
    totals.totalTokens += sample.totalTokens ?? 0;
    if (sample.apiEquivalentMicroUsd != null) {
      totals.apiEquivalent.microUsd += sample.apiEquivalentMicroUsd;
      totals.apiEquivalent.pricedTokens += sample.totalTokens ?? 0;
    } else {
      totals.apiEquivalent.unpricedTokens += sample.totalTokens ?? 0;
    }
    if (outputTokens > 0 && sample.latencyMs > 0) { totals.speedOutputTokens += outputTokens; totals.speedDurationMs += sample.latencyMs; }
    return totals;
  }, emptyUsageTotals());
}

function localSamples(events: LocalUsage[]): UsageSample[] {
  return events.map((item) => ({ ...item, createdAtMs: Date.parse(item.createdAt) }));
}

function remoteSamples(events: RemoteUsage[]): UsageSample[] {
  return events.map((item) => ({ ...item, ttftMs: item.ttftMs ?? null, generationMs: item.generationMs ?? null }));
}

function sourceHost(value: string) {
  try {
    return new URL(value).host;
  } catch {
    return value;
  }
}
