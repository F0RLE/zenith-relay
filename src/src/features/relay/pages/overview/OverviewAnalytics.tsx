import type { ReactNode } from "react";
import { memo, useMemo } from "react";
import { Activity, CreditCard, Database, Gauge, Timer } from "lucide-react";
import { useTranslation } from "react-i18next";
import type { UsageTotals } from "../../api/types";
import { formatApproximateUsd, formatScaledUsd, pricedUsd } from "../../currencyFormatting";
import { OptionMenu, Tabs } from "../../components/Ui";
import { formatNumber } from "../../numberFormatting";
import { formatTokenSpeed, observedTokensPerSecond } from "../../usageSpeed";
import { emptyUsageTotals, formatCompactNumber, formatFullNumber } from "../../usageTotals";
import { fillBuckets, lineSegments, type Analytics, type Range, type WindowBucket } from "./overviewAnalyticsModel";

type AnalyticsPanelProps = {
  range: Range;
  setRange: (range: Range) => void;
  windows: WindowBucket[];
  analytics: Analytics | null;
  loading: boolean;
  error: boolean;
  scope: string;
  setScope: (scope: string) => void;
  scopeOptions: Array<{ value: string; label: string }>;
};

function AnalyticsPanel({
  range,
  setRange,
  windows,
  analytics,
  loading,
  error,
  scope,
  setScope,
  scopeOptions,
}: AnalyticsPanelProps) {
  const { t, i18n } = useTranslation();
  const locale = i18n.resolvedLanguage ?? i18n.language;
  const hasAnalytics = analytics !== null;
  const buckets = useMemo(() => analytics ? fillBuckets(windows, analytics.buckets) : windows.map(emptyUsageTotals), [analytics, windows]);
  const requestSeries = useMemo(() => buckets.map((totals) => totals.requests || null), [buckets]);
  const apiCostSeries = useMemo(() => buckets.map((totals) => pricedUsd(totals.apiEquivalent)), [buckets]);
  const generationSpeedSeries = useMemo(() => buckets.map((totals) => observedTokensPerSecond(totals.generationOutputTokens, totals.generationMs)), [buckets]);
  const endToEndSpeedSeries = useMemo(() => buckets.map((totals) => observedTokensPerSecond(totals.speedOutputTokens, totals.speedDurationMs)), [buckets]);
  const totals = useMemo(() => analytics?.totals ?? emptyUsageTotals(), [analytics]);
  const averageGenerationSpeed = observedTokensPerSecond(totals.generationOutputTokens, totals.generationMs);
  const averageE2eSpeed = observedTokensPerSecond(totals.speedOutputTokens, totals.speedDurationMs);
  const apiTotal = totals.apiEquivalent;
  const rangeTabs = useMemo(() => [{ id: "today", label: t("overview.ranges.today") }, { id: "week", label: t("overview.ranges.week") }, { id: "month", label: t("overview.ranges.month") }], [t]);

  return <section className={`overview-analytics ${loading ? "loading" : ""} ${hasAnalytics ? "has-data" : ""}`} aria-busy={loading}>
    <header className="overview-analytics-header">
      <h2>{t("overview.analytics")}</h2>
      <div className="overview-analytics-controls">
        <OptionMenu className="overview-scope-menu" label={t("overview.scopeLabel")} value={scope} onChange={setScope} options={scopeOptions} />
        <Tabs value={range} onChange={(selectedRange) => setRange(selectedRange as Range)} label={t("overview.period")} items={rangeTabs} />
      </div>
    </header>
    {error ? <p className="overview-analytics-message error-text" role="alert">{t("overview.analyticsUnavailable")}</p> : null}
    <div className="overview-chart-stack">
      <TokenUsageTrend buckets={buckets} totals={totals} windows={windows} loading={loading && !hasAnalytics} />
      <OverviewChart
        icon={<CreditCard aria-hidden />}
        title={t("usage.apiEquivalent")}
        summary={formatApproximateUsd(pricedUsd(apiTotal), locale)}
        seriesValues={apiCostSeries}
        windows={windows}
        variant="bars"
        tone="cost"
        formatValue={(chartValue) => formatApproximateUsd(chartValue, locale)}
        formatAxis={(axisValue) => formatScaledUsd(axisValue, locale)}
        loading={loading && !hasAnalytics}
      />
      <OverviewChart
        icon={<Activity aria-hidden />}
        title={t("usage.requests")}
        summary={formatCompactNumber(totals.requests, locale)}
        seriesValues={requestSeries}
        windows={windows}
        variant="bars"
        tone="requests"
        formatValue={(chartValue) => formatFullNumber(chartValue, locale)}
        formatAxis={(axisValue) => formatFullNumber(axisValue, locale)}
        loading={loading && !hasAnalytics}
      />
      <OverviewChart
        icon={<Gauge aria-hidden />}
        title={t("usage.generationSpeed")}
        summary={formatTokenSpeed(averageGenerationSpeed, locale, t("usage.tokensPerSecondUnit"))}
        seriesValues={generationSpeedSeries}
        windows={windows}
        variant="line"
        tone="speed"
        formatValue={(chartValue) => formatTokenSpeed(chartValue, locale, t("usage.tokensPerSecondUnit"))}
        formatAxis={(axisValue) => formatNumber(axisValue, locale, { maximumFractionDigits: 1 })}
        loading={loading && !hasAnalytics}
      />
      <OverviewChart
        icon={<Timer aria-hidden />}
        title={t("usage.summaryMetrics.e2eSpeed")}
        summary={formatTokenSpeed(averageE2eSpeed, locale, t("usage.tokensPerSecondUnit"))}
        seriesValues={endToEndSpeedSeries}
        windows={windows}
        variant="line"
        tone="e2e-speed"
        formatValue={(chartValue) => formatTokenSpeed(chartValue, locale, t("usage.tokensPerSecondUnit"))}
        formatAxis={(axisValue) => formatNumber(axisValue, locale, { maximumFractionDigits: 1 })}
        loading={loading && !hasAnalytics}
      />
    </div>
  </section>;
}

export default memo(AnalyticsPanel);
function TokenUsageTrend({ buckets, totals, windows, loading }: { buckets: UsageTotals[]; totals: UsageTotals; windows: WindowBucket[]; loading: boolean }) {
  const { t, i18n } = useTranslation();
  const locale = i18n.resolvedLanguage ?? i18n.language;
  const tokenSeries = [
    { key: "input", label: t("overview.tokenTrend.input"), color: "input", points: buckets.map((totals) => totals.requests > 0 ? totals.inputTokens : null) },
    { key: "output", label: t("overview.tokenTrend.output"), color: "output", points: buckets.map((totals) => totals.requests > 0 ? totals.outputTokens : null) },
    { key: "cacheWrite", label: t("overview.tokenTrend.cacheWrite"), color: "cache-write", points: buckets.map((totals) => totals.cacheWriteInputSamples ? totals.cacheWriteInputTokens ?? 0 : null) },
    { key: "cacheRead", label: t("overview.tokenTrend.cacheRead"), color: "cache-read", points: buckets.map((totals) => totals.cachedInputSamples ? totals.cachedInputTokens : null) },
  ];
  const maxTokens = Math.max(0, ...tokenSeries.flatMap((series) => series.points.filter((tokenValue): tokenValue is number => tokenValue != null))) || 1;
  const cacheRateValues = buckets.map((totals) => totals.requests > 0 && totals.inputTokens > 0 && totals.cachedInputSamples ? Math.min(100, totals.cachedInputTokens / totals.inputTokens * 100) : null);
  const cacheTotals = buckets.reduce((tokenTotals, totals) => {
    if (totals.cachedInputSamples > 0 && totals.inputTokens > 0) {
      tokenTotals.inputTokens += totals.inputTokens;
      tokenTotals.cachedInputTokens += Math.min(totals.cachedInputTokens, totals.inputTokens);
    }
    return tokenTotals;
  }, { inputTokens: 0, cachedInputTokens: 0 });
  const averageCacheRate = cacheTotals.inputTokens > 0
    ? Math.min(100, cacheTotals.cachedInputTokens / cacheTotals.inputTokens * 100)
    : null;
  const hasData = tokenSeries.some((series) => series.points.some((tokenValue) => tokenValue != null && tokenValue > 0));
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
          <svg aria-hidden viewBox="0 0 100 100" preserveAspectRatio="none">
            <path className="overview-chart-grid" d="M0 0H100 M0 50H100 M0 100H100" />
            {tokenSeries.map((series) => lineSegments(series.points, maxTokens).map((path, index) => (
              <path className={`overview-token-trend-line is-${series.color}`} d={path} key={`${series.key}-${index}`} />
            )))}
            {lineSegments(cacheRateValues, 100).map((path, index) => (
              <path className="overview-token-trend-line is-cache-rate" d={path} key={`cache-rate-${index}`} />
            ))}
          </svg>
          <ol className="overview-token-trend-points" style={{ gridTemplateColumns: `repeat(${windows.length}, minmax(0, 1fr))` }}>
            {windows.map((window, index) => {
              const cacheRate = cacheRateValues[index];
              return (
                <li key={window.startMs}>
                  {tokenSeries.map((series) => {
                    const tokenValue = series.points[index];
                    if (tokenValue == null) return null;
                    const amount = formatCompactNumber(tokenValue, locale);
                    return (
                      <span
                        key={series.key}
                        tabIndex={0}
                        className={`overview-token-trend-dot is-${series.color}`}
                        style={{ top: `${(1 - tokenValue / maxTokens) * 100}%` }}
                        aria-label={`${window.fullLabel}: ${series.label} ${amount}`}
                      >
                        <span role="tooltip">{window.fullLabel}<strong>{series.label}: {amount}</strong></span>
                      </span>
                    );
                  })}
                  {cacheRate == null ? null : (
                    <span
                      tabIndex={0}
                      className="overview-token-trend-dot is-cache-rate"
                      style={{ top: `${100 - cacheRate}%` }}
                      aria-label={`${window.fullLabel}: ${t("overview.tokenTrend.cacheRate")} ${cacheRate.toFixed(0)}%`}
                    >
                      <span role="tooltip">{window.fullLabel}<strong>{t("overview.tokenTrend.cacheRate")}: {cacheRate.toFixed(0)}%</strong></span>
                    </span>
                  )}
                </li>
              );
            })}
          </ol>
          {!loading && !hasData ? <span className="overview-chart-empty">{t("overview.noMeasurements")}</span> : null}
        </div>
        <div className="overview-chart-x-axis" style={{ gridTemplateColumns: `repeat(${windows.length}, minmax(0, 1fr))` }} aria-hidden>{windows.map((window) => <span key={window.startMs} data-visible={window.showLabel}>{window.label}</span>)}</div>
      </div>
      <div className="overview-token-trend-rate-axis" aria-hidden><span>100%</span><span>50%</span><span>0%</span></div>
    </div>
  </article>;
}
function OverviewChart({
  icon,
  title,
  summary,
  seriesValues,
  windows,
  variant,
  tone,
  formatValue,
  formatAxis,
  loading,
}: {
  icon: ReactNode;
  title: string;
  summary: string;
  seriesValues: Array<number | null>;
  windows: WindowBucket[];
  variant: "bars" | "line";
  tone: string;
  formatValue: (chartValue: number) => string;
  formatAxis: (axisValue: number) => string;
  loading: boolean;
}) {
  const { t } = useTranslation();
  const measured = seriesValues.filter((chartValue): chartValue is number => chartValue != null);
  const max = Math.max(0, ...measured) || 1;
  const hasData = measured.some((chartValue) => chartValue > 0);
  const segments = lineSegments(seriesValues, max);
  return <article className={`overview-chart ${tone}`}>
    <header><div className="overview-chart-title">{icon}<span><strong>{title}</strong></span></div><strong className="overview-chart-summary">{loading ? "—" : summary}</strong></header>
    <div className="overview-chart-body">
      <div className="overview-chart-y-axis" aria-hidden><span>{formatAxis(max)}</span><span>{formatAxis(max / 2)}</span><span>0</span></div>
      <div className="overview-chart-plot">
        <div className="overview-chart-canvas">
          <svg aria-hidden viewBox="0 0 100 100" preserveAspectRatio="none">
            <path className="overview-chart-grid" d="M0 0H100 M0 50H100 M0 100H100" />
            {variant === "line" ? segments.map((path, index) => <path className="overview-chart-line" d={path} key={index} />) : null}
          </svg>
          <ol className={`overview-chart-points ${variant}`} style={{ gridTemplateColumns: `repeat(${seriesValues.length}, minmax(0, 1fr))` }}>
            {seriesValues.map((chartValue, index) => {
              const window = windows[index];
              if (!window) return null;
              const ratio = chartValue == null ? 0 : chartValue / max;
              const label = chartValue == null ? t("common.unknown") : formatValue(chartValue);
              return (
                <li key={window.startMs}>
                  {variant === "bars" && chartValue != null ? (
                    <span tabIndex={0} className="overview-chart-bar" style={{ height: `${Math.max(3, ratio * 100)}%` }} aria-label={`${window.fullLabel}: ${label}`}>
                      <span role="tooltip">{window.fullLabel}<strong>{label}</strong></span>
                    </span>
                  ) : null}
                  {variant === "line" && chartValue != null ? (
                    <span tabIndex={0} className="overview-chart-dot" style={{ top: `${(1 - ratio) * 100}%` }} aria-label={`${window.fullLabel}: ${label}`}>
                      <span role="tooltip">{window.fullLabel}<strong>{label}</strong></span>
                    </span>
                  ) : null}
                </li>
              );
            })}
          </ol>
          {!loading && !hasData ? <span className="overview-chart-empty">{t("overview.noMeasurements")}</span> : null}
        </div>
        <div className="overview-chart-x-axis" style={{ gridTemplateColumns: `repeat(${windows.length}, minmax(0, 1fr))` }} aria-hidden>{windows.map((window) => <span key={window.startMs} data-visible={window.showLabel}>{window.label}</span>)}</div>
      </div>
    </div>
  </article>;
}
