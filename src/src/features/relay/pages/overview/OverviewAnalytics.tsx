import type { ReactNode } from "react";
import { Activity, CreditCard, Database, Gauge, Timer } from "lucide-react";
import { useTranslation } from "react-i18next";
import type { UsageTotals } from "../../api/types";
import { OptionMenu, Tabs } from "../../components/Ui";
import { formatTokenSpeed } from "../../usageSpeed";
import { emptyUsageTotals, formatCompactNumber, formatFullNumber } from "../../usageTotals";
import { fillBuckets, formatApiEquivalent, formatUsd, lineSegments, type Analytics, type Range, type WindowBucket } from "./overviewAnalyticsModel";

export default function AnalyticsPanel({ range, setRange, windows, analytics, loading, error, scope, setScope, scopeOptions }: { range: Range; setRange: (range: Range) => void; windows: WindowBucket[]; analytics: Analytics | null; loading: boolean; error: boolean; scope: string; setScope: (scope: string) => void; scopeOptions: Array<{ value: string; label: string }> }) {
  const { t, i18n } = useTranslation();
  const locale = i18n.resolvedLanguage ?? i18n.language;
  const hasAnalytics = analytics !== null;
  const buckets = analytics ? fillBuckets(windows, analytics.buckets) : windows.map(emptyUsageTotals);
  const requestValues = buckets.map((totals) => totals.requests || null);
  const apiValues = buckets.map((totals) => totals.apiEquivalent.pricedTokens ? totals.apiEquivalent.microUsd / 1_000_000 : null);
  const generationSpeedValues = buckets.map((totals) => totals.generationMs && totals.generationOutputTokens ? totals.generationOutputTokens * 1_000 / totals.generationMs : null);
  const e2eSpeedValues = buckets.map((totals) => totals.speedDurationMs && totals.speedOutputTokens ? totals.speedOutputTokens * 1_000 / totals.speedDurationMs : null);
  const totals = analytics?.totals ?? emptyUsageTotals();
  const averageGenerationSpeed = totals.generationMs && totals.generationOutputTokens ? totals.generationOutputTokens * 1_000 / totals.generationMs : null;
  const averageE2eSpeed = totals.speedDurationMs && totals.speedOutputTokens ? totals.speedOutputTokens * 1_000 / totals.speedDurationMs : null;
  const apiTotal = totals.apiEquivalent;
  const rangeTabs = [{ id: "today", label: t("overview.ranges.today") }, { id: "week", label: t("overview.ranges.week") }, { id: "month", label: t("overview.ranges.month") }];

  return <section className={`overview-analytics ${loading ? "loading" : ""} ${hasAnalytics ? "has-data" : ""}`} aria-busy={loading}>
    <header className="overview-analytics-header"><h2>{t("overview.analytics")}</h2><div className="overview-analytics-controls"><OptionMenu className="overview-scope-menu" label={t("overview.scopeLabel")} value={scope} onChange={setScope} options={scopeOptions} /><Tabs value={range} onChange={(value) => setRange(value as Range)} label={t("overview.period")} items={rangeTabs} /></div></header>
    {error ? <p className="overview-analytics-message error-text" role="alert">{t("overview.analyticsUnavailable")}</p> : null}
    <div className="overview-chart-stack">
      <TokenUsageTrend buckets={buckets} totals={totals} windows={windows} loading={loading && !hasAnalytics} />
      <OverviewChart icon={<CreditCard aria-hidden />} title={t("usage.apiEquivalent")} summary={formatApiEquivalent(apiTotal.pricedTokens ? apiTotal.microUsd / 1_000_000 : null, locale)} values={apiValues} windows={windows} variant="bars" tone="cost" formatValue={(value) => formatApiEquivalent(value, locale)} formatAxis={(value) => formatUsd(value, locale)} loading={loading && !hasAnalytics} />
      <OverviewChart icon={<Activity aria-hidden />} title={t("usage.requests")} summary={formatCompactNumber(totals.requests, locale)} values={requestValues} windows={windows} variant="bars" tone="requests" formatValue={(value) => formatFullNumber(value, locale)} formatAxis={(value) => formatCompactNumber(value, locale)} loading={loading && !hasAnalytics} />
      <OverviewChart icon={<Gauge aria-hidden />} title={t("usage.generationSpeed")} summary={formatTokenSpeed(averageGenerationSpeed, locale, t("usage.tokensPerSecondUnit"))} values={generationSpeedValues} windows={windows} variant="line" tone="speed" formatValue={(value) => formatTokenSpeed(value, locale, t("usage.tokensPerSecondUnit"))} formatAxis={(value) => new Intl.NumberFormat(locale, { maximumFractionDigits: 1 }).format(value)} loading={loading && !hasAnalytics} />
      <OverviewChart icon={<Timer aria-hidden />} title={t("usage.summaryMetrics.e2eSpeed")} summary={formatTokenSpeed(averageE2eSpeed, locale, t("usage.tokensPerSecondUnit"))} values={e2eSpeedValues} windows={windows} variant="line" tone="e2e-speed" formatValue={(value) => formatTokenSpeed(value, locale, t("usage.tokensPerSecondUnit"))} formatAxis={(value) => new Intl.NumberFormat(locale, { maximumFractionDigits: 1 }).format(value)} loading={loading && !hasAnalytics} />
    </div>
  </section>;
}
function TokenUsageTrend({ buckets, totals, windows, loading }: { buckets: UsageTotals[]; totals: UsageTotals; windows: WindowBucket[]; loading: boolean }) {
  const { t, i18n } = useTranslation();
  const locale = i18n.resolvedLanguage ?? i18n.language;
  const tokenSeries = [
    { key: "input", label: t("overview.tokenTrend.input"), color: "input", values: buckets.map((totals) => totals.requests > 0 ? totals.inputTokens : null) },
    { key: "output", label: t("overview.tokenTrend.output"), color: "output", values: buckets.map((totals) => totals.requests > 0 ? totals.outputTokens : null) },
    { key: "cacheWrite", label: t("overview.tokenTrend.cacheWrite"), color: "cache-write", values: buckets.map((totals) => totals.cacheWriteInputSamples ? totals.cacheWriteInputTokens ?? 0 : null) },
    { key: "cacheRead", label: t("overview.tokenTrend.cacheRead"), color: "cache-read", values: buckets.map((totals) => totals.cachedInputSamples ? totals.cachedInputTokens : null) },
  ];
  const maxTokens = Math.max(0, ...tokenSeries.flatMap((series) => series.values.filter((value): value is number => value != null))) || 1;
  const cacheRateValues = buckets.map((totals) => totals.requests > 0 && totals.inputTokens > 0 && totals.cachedInputSamples ? Math.min(100, totals.cachedInputTokens / totals.inputTokens * 100) : null);
  const cacheTotals = buckets.reduce((result, totals) => {
    if (totals.cachedInputSamples > 0 && totals.inputTokens > 0) {
      result.inputTokens += totals.inputTokens;
      result.cachedInputTokens += Math.min(totals.cachedInputTokens, totals.inputTokens);
    }
    return result;
  }, { inputTokens: 0, cachedInputTokens: 0 });
  const averageCacheRate = cacheTotals.inputTokens > 0
    ? Math.min(100, cacheTotals.cachedInputTokens / cacheTotals.inputTokens * 100)
    : null;
  const hasData = tokenSeries.some((series) => series.values.some((value) => value != null && value > 0));
  return <article className="overview-chart tokens overview-token-trend">
    <header className="overview-token-trend-header">
      <div className="overview-chart-title"><Database aria-hidden /><span><strong>{t("overview.tokenUsage")}</strong></span></div>
      <div className="overview-token-trend-summary"><strong className="overview-chart-summary">{loading ? "—" : formatCompactNumber(totals.totalTokens, locale)}</strong><small>{loading || averageCacheRate == null ? "—" : `${averageCacheRate.toFixed(0)}% ${t("overview.tokenTrend.cacheRateShort")}`}</small></div>
    </header>
    <div className="overview-token-trend-legend" aria-label={t("overview.tokenTrend.legend")}>
      {tokenSeries.map((series) => <span key={series.key} className={`is-${series.color}`}><i aria-hidden />{series.label}</span>)}
      <span className="is-cache-rate"><i aria-hidden />{t("overview.tokenTrend.cacheRate")}</span>
    </div>
    <div className="overview-token-trend-body">
      <div className="overview-token-trend-axis" aria-hidden><span>{formatCompactNumber(maxTokens, locale)}</span><span>{formatCompactNumber(maxTokens / 2, locale)}</span><span>0</span></div>
      <div className="overview-token-trend-plot">
        <div className="overview-token-trend-canvas">
          <svg aria-hidden viewBox="0 0 100 100" preserveAspectRatio="none"><path className="overview-chart-grid" d="M0 0H100 M0 50H100 M0 100H100" />{tokenSeries.map((series) => lineSegments(series.values, maxTokens).map((path, index) => <path className={`overview-token-trend-line is-${series.color}`} d={path} key={`${series.key}-${index}`} />))}{lineSegments(cacheRateValues, 100).map((path, index) => <path className="overview-token-trend-line is-cache-rate" d={path} key={`cache-rate-${index}`} />)}</svg>
          <ol className="overview-token-trend-points" style={{ gridTemplateColumns: `repeat(${windows.length}, minmax(0, 1fr))` }}>
            {windows.map((window, index) => <li key={window.startMs}>{tokenSeries.map((series) => { const value = series.values[index]; return value == null ? null : <span key={series.key} tabIndex={0} className={`overview-token-trend-dot is-${series.color}`} style={{ top: `${(1 - value / maxTokens) * 100}%` }} aria-label={`${window.fullLabel}: ${series.label} ${formatCompactNumber(value, locale)}`}><span role="tooltip">{window.fullLabel}<strong>{series.label}: {formatCompactNumber(value, locale)}</strong></span></span>; })}{cacheRateValues[index] == null ? null : <span tabIndex={0} className="overview-token-trend-dot is-cache-rate" style={{ top: `${100 - cacheRateValues[index]}%` }} aria-label={`${window.fullLabel}: ${t("overview.tokenTrend.cacheRate")} ${cacheRateValues[index].toFixed(0)}%`}><span role="tooltip">{window.fullLabel}<strong>{t("overview.tokenTrend.cacheRate")}: {cacheRateValues[index].toFixed(0)}%</strong></span></span>}</li>)}
          </ol>
          {!loading && !hasData ? <span className="overview-chart-empty">{t("overview.noMeasurements")}</span> : null}
        </div>
        <div className="overview-chart-x-axis" style={{ gridTemplateColumns: `repeat(${windows.length}, minmax(0, 1fr))` }} aria-hidden>{windows.map((window) => <span key={window.startMs} data-visible={window.showLabel}>{window.label}</span>)}</div>
      </div>
      <div className="overview-token-trend-rate-axis" aria-hidden><span>100%</span><span>50%</span><span>0%</span></div>
    </div>
  </article>;
}
function OverviewChart({ icon, title, summary, values, windows, variant, tone, formatValue, formatAxis, loading }: { icon: ReactNode; title: string; summary: string; values: Array<number | null>; windows: WindowBucket[]; variant: "bars" | "line"; tone: string; formatValue: (value: number) => string; formatAxis: (value: number) => string; loading: boolean }) {
  const { t } = useTranslation();
  const measured = values.filter((value): value is number => value != null);
  const max = Math.max(0, ...measured) || 1;
  const hasData = measured.some((value) => value > 0);
  const segments = lineSegments(values, max);
  return <article className={`overview-chart ${tone}`}>
    <header><div className="overview-chart-title">{icon}<span><strong>{title}</strong></span></div><strong className="overview-chart-summary">{loading ? "—" : summary}</strong></header>
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
