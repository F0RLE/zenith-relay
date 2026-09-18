import { Activity, ArrowRight, CircleAlert, CreditCard, Gauge } from "lucide-react";
import { useTranslation } from "react-i18next";
import type { SourceStatsAmount, SourceSummary } from "../api/types";
import { formatApiEquivalent } from "../poolFormatting";
import { formatFullNumber } from "../usageTotals";
import { formatSourceAmount, sourceStatsAmounts, sourceStatsStatus, type SourceStatsState } from "../sourceStatsModel";

export function SourceStatsPanel({ source, state, overview = false }: { source: SourceSummary; state?: SourceStatsState; overview?: boolean }) {
  const { t, i18n } = useTranslation();
  const locale = i18n.resolvedLanguage ?? i18n.language;
  const stats = state?.value;
  const amounts = sourceStatsAmounts(stats);
  const status = stats ? sourceStatsStatus(stats) : state?.failed ? "unavailable" : "available";
  const available = stats != null && status === "available";
  const stale = available && state?.failed;
  const error = state?.error ?? (state?.failed ? "unavailable" : status);
  const formatAmounts = (field: "balanceMicros" | "spentMicros") => amounts
    .filter((amount) => amount[field] != null)
    .map((amount: SourceStatsAmount) => `${formatSourceAmount(amount[field]!, amount.currency, locale)}${amount.currency === "CREDITS" ? ` ${t("providerStats.units")}` : ""}`);
  const balances = formatAmounts("balanceMicros");
  const spending = formatAmounts("spentMicros");
  const balance = available && stats.balanceUnlimited ? [t("providerStats.unlimited")]
    : balances.length ? balances
      : [state?.loading ? t("providerStats.loading") : available ? t("providerStats.notReported") : t(`providerStats.status.${error}`)];
  const metrics = [
    { key: "balance", label: t(`providerStats.balance.${stats?.balanceKind ?? "wallet"}`), values: balance, icon: CreditCard, muted: !balances.length && !stats?.balanceUnlimited },
    { key: "spend", label: t(spending.length ? "overview.spent" : "providerStats.relayEstimate"), values: spending.length ? spending : [formatApiEquivalent(source.apiEquivalent.microUsd, locale)], icon: ArrowRight,
      hint: spending.length ? undefined : t("pool.apiEquivalentHint", { count: source.apiEquivalent.unpricedTokens }) },
    ...(available && stats.requests != null ? [{ key: "requests", label: t("usage.requests"), values: [formatFullNumber(stats.requests, locale)], icon: Activity }] : []),
    ...(overview && available && stats.totalTokens != null
      ? [{ key: "tokens", label: t("overview.totalTokens"), values: [formatFullNumber(stats.totalTokens, locale)], icon: Gauge }]
      : [{ key: "models", label: t("common.models"), values: [formatFullNumber(source.models.length, locale)], icon: Gauge }]),
  ];
  return <div className={`source-stats-panel${overview ? " source-stats-overview" : ""}`} aria-busy={state?.loading || undefined}>
    <dl className={overview ? "metric-band direct-api-metrics source-stats-metrics" : "pool-source-stats"}>
      {metrics.map(({ key, label, values, icon: Icon, muted, hint }) => <div key={key} data-metric={key} data-relay-tooltip={hint}>
        {overview ? <Icon aria-hidden /> : null}<dt>{label}</dt>
        <dd data-muted={muted ? "true" : undefined}>{values.map((value, index) => <span key={index}>{value}</span>)}</dd>
      </div>)}
    </dl>
    {stale ? <div className="source-stats-caption" data-warning="true" data-relay-tooltip={t(`providerStats.status.${error}`)} role="status">
      <span><CircleAlert aria-hidden />{t("providerStats.stale")}</span>
    </div> : null}
  </div>;
}
