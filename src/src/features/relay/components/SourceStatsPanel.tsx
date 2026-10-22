import { useTranslation } from "react-i18next";
import type { SourceStatsAmount, SourceSummary } from "../api/types";
import { formatApiEquivalent } from "../poolFormatting";
import { formatFullNumber } from "../usageTotals";
import { formatSourceAmount, sourceStatsAmounts, sourceStatsStatus, type SourceStatsState } from "../sourceStatsModel";

type SourceStatMetric = {
  key: string;
  label: string;
  displayValues: string[];
  muted?: boolean;
  hint?: string | undefined;
};

export function SourceStatsPanel({ source, state }: { source: SourceSummary; state?: SourceStatsState }) {
  const { t, i18n } = useTranslation();
  const locale = i18n.resolvedLanguage ?? i18n.language;
  const stats = state?.stats ?? source.providerStats;
  const amounts = sourceStatsAmounts(stats);
  const status = stats ? sourceStatsStatus(stats) : state?.failed ? "unavailable" : source.refreshState?.balance === "unsupported" ? "unsupported" : undefined;
  const available = stats != null && status === "available";
  const stale = available && (stats?.stale || state?.failed);
  const error = stats?.refreshError ?? state?.error ?? (stale || state?.failed ? "unavailable" : status);
  const formatAmounts = (field: "balanceMicros" | "spentMicros") => amounts
    .filter((amount) => amount[field] != null)
    .map((amount: SourceStatsAmount) => `${formatSourceAmount(amount[field]!, amount.currency, locale)}${amount.currency === "CREDITS" ? ` ${t("providerStats.units")}` : ""}`);
  const balances = formatAmounts("balanceMicros");
  const spending = formatAmounts("spentMicros");
  const balance = available && stats.balanceUnlimited ? [t("providerStats.unlimited")]
    : balances.length ? balances
      : [state?.loading ? t("providerStats.loading") : !status || available ? t("providerStats.notReported") : t(`providerStats.status.${error}`)];
  const balanceMetric: SourceStatMetric = { key: "balance", label: t(`providerStats.balance.${stats?.balanceKind ?? "wallet"}`), displayValues: balance, muted: !balances.length && !stats?.balanceUnlimited };
  const usageMetrics: SourceStatMetric[] = [
    { key: "spend", label: t(spending.length ? "overview.spent" : "providerStats.relayEstimate"), displayValues: spending.length ? spending : [formatApiEquivalent(source.apiEquivalent.microUsd, locale)],
      hint: spending.length ? undefined : t("pool.apiEquivalentHint", { count: source.apiEquivalent.unpricedTokens }) },
    ...(available && stats.requests != null ? [{ key: "requests", label: t("usage.requests"), displayValues: [formatFullNumber(stats.requests, locale)] }] : []),
  ];
  const metrics: SourceStatMetric[] = [
    balanceMetric,
    ...usageMetrics,
    { key: "models", label: t("common.models"), displayValues: [formatFullNumber(source.models.length, locale)] },
  ];
  return <div className="source-stats-panel" aria-busy={state?.loading || undefined}>
    <dl className="pool-source-stats">
      {metrics.map(({ key, label, displayValues, muted, hint }) => <div key={key} data-metric={key} data-relay-tooltip={hint}>
        <dt>{label}</dt>
        <dd data-muted={muted ? "true" : undefined}>{displayValues.map((displayValue, index) => <span key={index}>{displayValue}</span>)}</dd>
      </div>)}
    </dl>
  </div>;
}
