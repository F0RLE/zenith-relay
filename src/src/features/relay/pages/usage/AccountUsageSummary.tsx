import { Activity, CreditCard } from "lucide-react";
import { useTranslation } from "react-i18next";
import type { AccountSummary, UsageTotals } from "../../api/types";
import { buildAccountValueProjection, formatAccountPayback } from "../../accountEconomics";
import { formatAccountValueMicroUsd } from "../../poolFormatting";
import { formatDetailedRemainingTime, formatQuotaRemaining, formatWindowDuration } from "../../quotaFormatting";
import { UsageMetric } from "./UsageMetric";
import { formatUsageApiEquivalent } from "./usageFormatting";

export function AccountUsageSummary({ account, totals }: { account: AccountSummary; totals: UsageTotals }) {
  const { t, i18n } = useTranslation();
  const locale = i18n.resolvedLanguage ?? i18n.language;
  const nowMs = Date.now();
  const { purchaseCostMicroUsd: purchaseCost, remainingApiEquivalent, payback, approximate } = buildAccountValueProjection(
    totals.apiEquivalent,
    account.quota,
    account.quotaWindowUsage,
    account.purchaseCostMicroUsd,
  );
  const paybackTitle = purchaseCost == null
    ? t("accounts.accountValue.purchaseMissing")
    : t("accounts.accountValue.paybackHint", {
      used: formatAccountValueMicroUsd(totals.apiEquivalent.microUsd, locale),
      purchase: formatAccountValueMicroUsd(purchaseCost, locale),
    });
  const windows = (["primary", "secondary"] as const)
    .map((kind) => ({ kind, quota: account.quota[kind] }))
    .filter(({ quota }) => Boolean(quota));

  return <section className="usage-account-value" aria-label={t("usage.accountUsage", { account: account.label })}>
    <header><div><span>{t("usage.selectedAccount")}</span><strong>{account.label}</strong></div><details><summary>{t("usage.howCalculated")}</summary><p>{t("usage.calculationHint")}</p></details></header>
    <div className="usage-account-metrics">
      <UsageMetric icon={<CreditCard aria-hidden />} label={t("accounts.accountValue.used")} value={formatUsageApiEquivalent(totals.apiEquivalent, locale)} title={t("accounts.accountValue.usedHint", { count: totals.apiEquivalent.unpricedTokens })} />
      {remainingApiEquivalent ? <UsageMetric icon={<CreditCard aria-hidden />} label={t("accounts.accountValue.remaining")} value={formatAccountValueMicroUsd(remainingApiEquivalent.microUsd, locale, remainingApiEquivalent.approximate)} title={t("accounts.accountValue.remainingHint")} /> : null}
      <UsageMetric icon={<CreditCard aria-hidden />} label={t("accounts.accountValue.purchaseCost")} value={purchaseCost == null ? "—" : formatAccountValueMicroUsd(purchaseCost, locale)} detail={purchaseCost == null ? t("accounts.accountValue.purchaseMissing") : undefined} />
      <UsageMetric icon={<Activity aria-hidden />} label={t("accounts.accountValue.payback")} value={formatAccountPayback(payback, locale, approximate)} title={paybackTitle} />
    </div>
    {windows.length ? <div className="relay-table-wrap"><table className="relay-table usage-window-table"><thead><tr><th>{t("usage.window")}</th><th>{t("usage.remaining")}</th><th>{t("usage.reset")}</th></tr></thead><tbody>{windows.map(({ kind, quota }) => <tr key={kind}><th scope="row">{formatWindowDuration(quota?.windowMinutes ?? null, locale, t("usage.window"))}</th><td>{formatQuotaRemaining(quota?.availableBasisPoints ?? null, locale)}</td><td>{quota?.resetAtMs == null ? "—" : formatDetailedRemainingTime(quota.resetAtMs, nowMs, t)}</td></tr>)}</tbody></table></div> : null}
  </section>;
}
