import type { ReactNode } from "react";
import { Activity, CreditCard, Gauge, Timer } from "lucide-react";
import { useTranslation } from "react-i18next";
import type { UsageBucket, UsageTotals } from "../../api/types";
import { Tabs } from "../../components/Ui";
import { formatTokenSpeed } from "../../usageSpeed";

type Range = "today" | "week" | "month";
type WindowBucket = { startMs: number; endMs: number; label: string; fullLabel: string; showLabel: boolean };
type Analytics = { totals: UsageTotals; buckets: UsageBucket[] };

export default function AnalyticsPanel({ range, setRange, windows, analytics, loading, error }: { range: Range; setRange: (range: Range) => void; windows: WindowBucket[]; analytics: Analytics | null; loading: boolean; error: boolean }) {
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
