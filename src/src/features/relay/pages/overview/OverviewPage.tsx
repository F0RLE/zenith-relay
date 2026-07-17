import { type ReactNode, useEffect, useMemo, useState } from "react";
import { Activity, ArrowRight, CircleAlert, CreditCard, Gauge, Play, Server, Square, Timer, Users } from "lucide-react";
import { useTranslation } from "react-i18next";
import type { UsageLogEntry } from "../../../../tauri";
import { relayCommands } from "../../api/commands";
import type { LocalUsage, RemoteUsage, UsageBucket, UsageTotals } from "../../api/types";
import { Button, EmptyState, PageHeader, StatusBadge, Tabs } from "../../components/Ui";
import { useRelayState } from "../../state/RelayStateProvider";
import { formatTokenSpeed } from "../../usageSpeed";

type Range = "today" | "week" | "month";
type WindowBucket = { startMs: number; endMs: number; label: string; fullLabel: string; showLabel: boolean };
type Analytics = { totals: UsageTotals; buckets: UsageBucket[] };
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
  const { mode, runtime, readyState, localUsage, localUsagePage, remoteUsage, remoteUsagePage, readyUsage, setPage, perform, busy } = useRelayState();
  const [range, setRange] = useState<Range>("today");
  const [analytics, setAnalytics] = useState<Analytics | null>(null);
  const [analyticsLoading, setAnalyticsLoading] = useState(false);
  const [analyticsError, setAnalyticsError] = useState(false);
  const locale = i18n.resolvedLanguage ?? i18n.language;
  const windows = useMemo(() => chartWindows(range, locale), [range, locale]);
  const running = mode === "zenith" ? Boolean(readyState?.providerActive) : Boolean(runtime?.gateway.running);

  useEffect(() => {
    let active = true;
    setAnalytics(null);
    setAnalyticsError(false);
    if (mode === "zenith") {
      setAnalytics(analyticsFromSamples(windows, readySamples(readyUsage)));
      setAnalyticsLoading(false);
      return () => { active = false; };
    }
    if (mode === "remote" && !runtime?.capabilities.features.includes("usage")) {
      setAnalyticsLoading(false);
      return () => { active = false; };
    }
    setAnalyticsLoading(true);
    const input = { page: 1, pageSize: 5, range: "custom" as const, fromMs: windows[0].startMs, toMs: windows[windows.length - 1].endMs, bucketMs: range === "today" ? HOUR_MS : DAY_MS };
    const request: Promise<Analytics | null> = mode === "local"
      ? relayCommands.localUsagePage(input).then((page) => analyticsFromPage(page.totals, page.buckets, localSamples(page.events), windows))
      : relayCommands.remoteUsage(input).then((page) => page ? analyticsFromPage(page.totals, page.buckets, remoteSamples(page.events), windows) : null);
    request
      .then((result) => {
        if (!active || !result) return;
        setAnalytics(result);
      })
      .catch(() => active && setAnalyticsError(true))
      .finally(() => active && setAnalyticsLoading(false));
    return () => { active = false; };
  }, [mode, range, windows, readyUsage, runtime?.capabilities.features]);

  const fallbackTotals = mode === "local" ? localUsagePage?.totals : mode === "remote" ? remoteUsagePage?.totals : totalsFromSamples(readySamples(readyUsage));
  const totals = analytics?.totals ?? fallbackTotals ?? emptyTotals();
  const requests = totals.requests;
  const models = mode === "zenith" ? new Set(readyUsage.map((item) => item.model).filter(Boolean)).size : runtime?.gateway.visibleModelIds.length ?? 0;
  const healthy = mode === "zenith" ? (running ? 1 : 0) : [...(runtime?.sources ?? []), ...(runtime?.accounts ?? [])].filter((item) => item.enabled).length;
  const errors = Math.max(0, totals.requests - totals.successfulRequests);
  const poolUsage = mode === "remote" ? remoteUsagePage?.events ?? remoteUsage : localUsagePage?.events ?? localUsage;
  const activity = mode === "zenith"
    ? readyUsage.slice(0, 5).map((item) => ({ id: item.id, success: item.status === "success", model: item.model, latency: item.responseTimeDisplay }))
    : poolUsage.slice(0, 5).map((item) => ({ id: item.id, success: item.success, model: item.resolvedModel ?? item.requestedModel, latency: `${item.latencyMs} ms` }));

  const primary = mode === "local" ? <Button variant="primary" busy={busy === "gateway"} icon={running ? <Square aria-hidden /> : <Play aria-hidden />} onClick={() => perform("gateway", () => running ? relayCommands.stopGateway() : relayCommands.startGateway(), running ? "feedback.stopped" : "feedback.started")}>{running ? t("gateway.stop") : t("gateway.start")}</Button> : mode === "remote" ? <Button variant="primary" icon={<Server aria-hidden />} onClick={() => setPage("connections")}>{runtime ? t("overview.openServer") : t("remote.connect")}</Button> : <Button variant="primary" icon={<ArrowRight aria-hidden />} onClick={() => setPage("connections")}>{running ? t("overview.openConnection") : t("readyApi.connect")}</Button>;

  return <section className="relay-page"><PageHeader title={t("nav.overview")} subtitle={t(`overview.subtitles.${mode}`)} actions={primary} />
    {!running && !runtime && mode !== "zenith" ? <EmptyState title={t("overview.emptyTitle")} description={t("overview.emptyDescription")} action={<Button variant="primary" onClick={() => setPage("connections")}>{t("overview.openConnections")}</Button>} /> : <>
      <div className="metric-band overview-metrics"><div><Activity aria-hidden /><span>{t("overview.requestsToday")}</span><strong>{formatCompactNumber(requests, locale)}</strong></div><div><Users aria-hidden /><span>{t("overview.healthy")}</span><strong>{healthy}</strong></div><div><ArrowRight aria-hidden /><span>{t("overview.models")}</span><strong>{models || "-"}</strong></div><div><CircleAlert aria-hidden /><span>{t("overview.errors")}</span><strong>{formatCompactNumber(errors, locale)}</strong></div></div>
      <AnalyticsPanel range={range} setRange={setRange} windows={windows} analytics={analytics} loading={analyticsLoading} error={analyticsError} />
      <section className="activity-section"><header><h2>{t("overview.activity")}</h2><Button variant="ghost" onClick={() => setPage("usage")}>{t("overview.viewUsage")}</Button></header>{activity.length ? <ul>{activity.map((item) => <li key={item.id}><StatusBadge status={item.success ? "ready" : "error"} label={item.success ? t("common.success") : t("common.failed")} /><code>{item.model ?? "-"}</code><span>{item.latency}</span></li>)}</ul> : <p className="muted">{t("usage.empty")}</p>}</section>
    </>}
  </section>;
}

function AnalyticsPanel({ range, setRange, windows, analytics, loading, error }: { range: Range; setRange: (range: Range) => void; windows: WindowBucket[]; analytics: Analytics | null; loading: boolean; error: boolean }) {
  const { t, i18n } = useTranslation();
  const locale = i18n.resolvedLanguage ?? i18n.language;
  const buckets = analytics ? fillBuckets(windows, analytics.buckets) : windows.map(() => emptyTotals());
  const tokenValues = buckets.map((totals) => totals.totalTokens || null);
  const apiValues = buckets.map((totals) => totals.apiEquivalent.pricedTokens ? totals.apiEquivalent.microUsd / 1_000_000 : null);
  const responseValues = buckets.map((totals) => totals.ttftSamples ? totals.ttftMs / totals.ttftSamples : null);
  const speedValues = buckets.map((totals) => totals.generationMs && totals.generationOutputTokens ? totals.generationOutputTokens * 1_000 / totals.generationMs : null);
  const totals = analytics?.totals ?? emptyTotals();
  const averageResponse = totals.ttftSamples ? totals.ttftMs / totals.ttftSamples : null;
  const averageSpeed = totals.generationMs && totals.generationOutputTokens ? totals.generationOutputTokens * 1_000 / totals.generationMs : null;
  const apiTotal = totals.apiEquivalent;
  const rangeTabs = [{ id: "today", label: t("overview.ranges.today") }, { id: "week", label: t("overview.ranges.week") }, { id: "month", label: t("overview.ranges.month") }];

  return <section className={`overview-analytics ${loading ? "loading" : ""}`} aria-busy={loading}>
    <header className="overview-analytics-header"><div><h2>{t("overview.analytics")}</h2><p>{t("overview.analyticsHint")}</p></div><Tabs value={range} onChange={(value) => setRange(value as Range)} label={t("overview.period")} items={rangeTabs} /></header>
    {error ? <p className="overview-analytics-message error-text" role="alert">{t("overview.analyticsUnavailable")}</p> : null}
    <div className="overview-chart-stack">
      <OverviewChart icon={<Activity aria-hidden />} title={t("overview.tokenUsage")} hint={t("overview.tokenUsageHint")} summary={formatCompactNumber(totals.totalTokens, locale)} values={tokenValues} windows={windows} variant="bars" tone="tokens" formatValue={(value) => t("overview.tokenValue", { value: formatFullNumber(value, locale) })} formatAxis={(value) => formatCompactNumber(value, locale)} loading={loading} />
      <OverviewChart icon={<CreditCard aria-hidden />} title={t("usage.apiEquivalent")} hint={t("overview.apiEquivalentHint")} summary={formatApiEquivalent(apiTotal.pricedTokens ? apiTotal.microUsd / 1_000_000 : null, locale, apiTotal.unpricedTokens > 0)} values={apiValues} windows={windows} variant="bars" tone="cost" formatValue={(value) => formatApiEquivalent(value, locale)} formatAxis={(value) => formatUsd(value, locale)} loading={loading} />
      <OverviewChart icon={<Timer aria-hidden />} title={t("overview.responseSpeed")} hint={t("overview.responseSpeedHint")} summary={formatDuration(averageResponse, locale)} values={responseValues} windows={windows} variant="line" tone="latency" formatValue={(value) => formatDuration(value, locale)} formatAxis={(value) => formatDuration(value, locale)} loading={loading} />
      <OverviewChart icon={<Gauge aria-hidden />} title={t("overview.generationSpeed")} hint={t("overview.generationSpeedHint")} summary={formatTokenSpeed(averageSpeed, locale, t("usage.tokensPerSecondUnit"))} values={speedValues} windows={windows} variant="line" tone="speed" formatValue={(value) => formatTokenSpeed(value, locale, t("usage.tokensPerSecondUnit"))} formatAxis={(value) => new Intl.NumberFormat(locale, { maximumFractionDigits: 1 }).format(value)} loading={loading} />
    </div>
  </section>;
}

function OverviewChart({ icon, title, hint, summary, values, windows, variant, tone, formatValue, formatAxis, loading }: { icon: ReactNode; title: string; hint: string; summary: string; values: Array<number | null>; windows: WindowBucket[]; variant: "bars" | "line"; tone: string; formatValue: (value: number) => string; formatAxis: (value: number) => string; loading: boolean }) {
  const { t } = useTranslation();
  const measured = values.filter((value): value is number => value != null);
  const max = Math.max(0, ...measured) || 1;
  const hasData = measured.some((value) => value > 0);
  const segments = lineSegments(values, max);
  return <article className={`overview-chart ${tone}`}>
    <header><div className="overview-chart-title">{icon}<span><strong>{title}</strong><small>{hint}</small></span></div><strong className="overview-chart-summary">{loading ? "—" : summary}</strong></header>
    <div className="overview-chart-body">
      <div className="overview-chart-y-axis" aria-hidden><span>{formatAxis(max)}</span><span>{formatAxis(max / 2)}</span><span>0</span></div>
      <div className="overview-chart-plot">
        <div className="overview-chart-canvas">
          <svg aria-hidden viewBox="0 0 100 100" preserveAspectRatio="none"><path className="overview-chart-grid" d="M0 0H100 M0 50H100 M0 100H100" />{variant === "line" ? segments.map((path, index) => <path className="overview-chart-line" d={path} key={index} />) : null}</svg>
          <ol className={`overview-chart-points ${variant}`} style={{ gridTemplateColumns: `repeat(${values.length}, minmax(0, 1fr))` }}>
            {values.map((value, index) => {
              const ratio = value == null ? 0 : value / max;
              const label = value == null ? t("common.unknown") : formatValue(value);
              return <li key={windows[index].startMs}>{variant === "bars" && value != null ? <span tabIndex={0} className="overview-chart-bar" style={{ height: `${Math.max(3, ratio * 100)}%` }} aria-label={`${windows[index].fullLabel}: ${label}`}><span role="tooltip">{windows[index].fullLabel}<strong>{label}</strong></span></span> : null}{variant === "line" && value != null ? <span tabIndex={0} className="overview-chart-dot" style={{ top: `${(1 - ratio) * 100}%` }} aria-label={`${windows[index].fullLabel}: ${label}`}><span role="tooltip">{windows[index].fullLabel}<strong>{label}</strong></span></span> : null}</li>;
            })}
          </ol>
          {!loading && !hasData ? <span className="overview-chart-empty">{t("overview.noMeasurements")}</span> : null}
        </div>
        <div className="overview-chart-x-axis" style={{ gridTemplateColumns: `repeat(${windows.length}, minmax(0, 1fr))` }} aria-hidden>{windows.map((window) => <span key={window.startMs} data-visible={window.showLabel}>{window.label}</span>)}</div>
      </div>
    </div>
  </article>;
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

function fillBuckets(windows: WindowBucket[], buckets: UsageBucket[]) {
  const byStart = new Map(buckets.map((bucket) => [bucket.startMs, bucket.totals]));
  return windows.map((window) => byStart.get(window.startMs) ?? emptyTotals());
}

function lineSegments(values: Array<number | null>, max: number) {
  const segments: string[] = [];
  let current = "";
  values.forEach((value, index) => {
    if (value == null) {
      if (current) segments.push(current);
      current = "";
      return;
    }
    const x = (index + 0.5) / values.length * 100;
    const y = (1 - value / max) * 100;
    current += `${current ? " L" : "M"}${x.toFixed(2)} ${y.toFixed(2)}`;
  });
  if (current) segments.push(current);
  return segments;
}

function analyticsFromSamples(windows: WindowBucket[], samples: UsageSample[]): Analytics {
  return { totals: totalsFromSamples(samples.filter((sample) => sample.createdAtMs >= windows[0].startMs && sample.createdAtMs <= windows[windows.length - 1].endMs)), buckets: bucketsFromSamples(windows, samples) };
}

function analyticsFromPage(totals: UsageTotals | undefined, buckets: UsageBucket[] | undefined, samples: UsageSample[], windows: WindowBucket[]): Analytics {
  return { totals: totals ?? totalsFromSamples(samples), buckets: buckets?.length ? buckets : bucketsFromSamples(windows, samples) };
}

function bucketsFromSamples(windows: WindowBucket[], samples: UsageSample[]) {
  return windows.map((window) => ({ startMs: window.startMs, totals: totalsFromSamples(samples.filter((sample) => sample.createdAtMs >= window.startMs && sample.createdAtMs <= window.endMs)) }));
}

function totalsFromSamples(samples: UsageSample[]) {
  return samples.reduce<UsageTotals>((totals, sample) => {
    const visibleOutput = sample.success ? Math.max(0, (sample.outputTokens ?? 0) - (sample.reasoningTokens ?? 0)) : 0;
    totals.requests += 1;
    totals.successfulRequests += Number(sample.success);
    totals.latencyMs += sample.latencyMs;
    if (sample.ttftMs != null) { totals.ttftMs += sample.ttftMs; totals.ttftSamples += 1; }
    if (sample.success && sample.generationMs != null && sample.generationMs > 0) { totals.generationMs += sample.generationMs; totals.generationSamples += 1; totals.generationOutputTokens += visibleOutput; }
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
    if (visibleOutput > 0 && sample.latencyMs > 0) { totals.speedOutputTokens += visibleOutput; totals.speedDurationMs += sample.latencyMs; }
    return totals;
  }, emptyTotals());
}

function localSamples(events: LocalUsage[]): UsageSample[] {
  return events.map((item) => ({ ...item, createdAtMs: Date.parse(item.createdAt) }));
}

function remoteSamples(events: RemoteUsage[]): UsageSample[] {
  return events.map((item) => ({ ...item, ttftMs: item.ttftMs ?? null, generationMs: item.generationMs ?? null }));
}

function readySamples(events: UsageLogEntry[]): UsageSample[] {
  return events.map((item) => ({ createdAtMs: Date.parse(item.createdAt), success: item.status === "success", latencyMs: item.streamDurationMs ?? item.timeToFirstByteMs ?? 0, ttftMs: item.timeToFirstByteMs ?? null, generationMs: item.streamDurationMs ?? null, inputTokens: item.inputTokens, cachedInputTokens: item.cachedInputTokens, cacheWriteInputTokens: null, reasoningTokens: item.reasoningTokens, outputTokens: item.outputTokens, totalTokens: item.totalTokens, apiEquivalentMicroUsd: item.displayCostMicrousd ?? item.costMicrousd ?? (Number.isFinite(item.costCents) ? item.costCents * 10_000 : null) }));
}

function emptyTotals(): UsageTotals {
  return { requests: 0, successfulRequests: 0, latencyMs: 0, ttftMs: 0, ttftSamples: 0, generationMs: 0, generationSamples: 0, generationOutputTokens: 0, inputTokens: 0, cachedInputTokens: 0, cachedInputSamples: 0, cacheWriteInputTokens: 0, cacheWriteInputSamples: 0, reasoningTokens: 0, outputTokens: 0, totalTokens: 0, speedOutputTokens: 0, speedDurationMs: 0, apiEquivalent: { microUsd: 0, pricedTokens: 0, unpricedTokens: 0 } };
}

function formatCompactNumber(value: number, locale: string) {
  return new Intl.NumberFormat(locale, { notation: Math.abs(value) >= 1_000 ? "compact" : "standard", maximumFractionDigits: 1 }).format(value);
}

function formatFullNumber(value: number, locale: string) {
  return new Intl.NumberFormat(locale, { maximumFractionDigits: 0 }).format(value);
}

function formatDuration(value: number | null, locale: string) {
  if (value == null) return "—";
  return new Intl.NumberFormat(locale, { style: "unit", unit: value >= 1_000 ? "second" : "millisecond", unitDisplay: "short", maximumFractionDigits: 1 }).format(value >= 1_000 ? value / 1_000 : value);
}

function formatApiEquivalent(value: number | null, locale: string, partial = false) {
  return value == null ? "—" : `≈${formatUsd(value, locale)}${partial ? "*" : ""}`;
}

function formatUsd(value: number, locale: string) {
  return new Intl.NumberFormat(locale, { style: "currency", currency: "USD", minimumFractionDigits: 2, maximumFractionDigits: value < 0.01 ? 6 : value < 1 ? 4 : 2 }).format(value);
}
