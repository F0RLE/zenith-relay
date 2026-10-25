import { useMemo } from "react";
import { DollarSign, TrendingUp, Wallet } from "lucide-react";
import { useTranslation } from "react-i18next";
import { formatAccountPayback } from "../../accountEconomics";
import { formatAccountValueMicroUsd } from "../../poolFormatting";
import type { PoolMember } from "../../poolHelpers";
import { formatUsageApiEquivalent } from "../usage/usageFormatting";
import { poolEconomics } from "./poolEconomics";

export function PoolEconomicsStrip({ members }: { members: readonly PoolMember[] }) {
  const { t, i18n } = useTranslation();
  const locale = i18n.resolvedLanguage ?? i18n.language;
  const economics = useMemo(() => poolEconomics(members), [members]);
  return <div className="pool-economics relay-status-summary" role="group" aria-label={t("pool.economicsLabel")}>
    <div role="group" tabIndex={0} aria-label={t("accounts.accountValue.used")} data-metric="api-used" data-relay-tooltip={`${t("accounts.accountValue.used")}. ${t("pool.economicsUsedHint", { count: economics.used.unpricedTokens })}`}>
      <DollarSign aria-hidden />
      <strong>{formatUsageApiEquivalent(economics.used, locale, 2)}</strong>
    </div>
    <div role="group" tabIndex={0} aria-label={t("accounts.accountValue.remaining")} data-metric="api-remaining" data-relay-tooltip={`${t("accounts.accountValue.remaining")}. ${t("pool.economicsRemainingHint", { known: economics.remainingCount, total: economics.accountCount })}`}>
      <Wallet aria-hidden />
      <strong>{economics.remainingMicroUsd != null ? formatAccountValueMicroUsd(economics.remainingMicroUsd, locale, economics.remainingApproximate) : "—"}</strong>
    </div>
    <div role="group" tabIndex={0} aria-label={t("pool.totalPayback")} data-metric="payback" data-paid={economics.payback != null && economics.payback >= 1} data-relay-tooltip={`${t("pool.totalPayback")}. ${t("pool.economicsPaybackHint", {
      known: economics.purchaseCount, total: economics.accountCount,
      used: formatAccountValueMicroUsd(economics.recoveredMicroUsd, locale),
      purchase: formatAccountValueMicroUsd(economics.purchaseMicroUsd, locale),
    })}`}>
      <TrendingUp aria-hidden />
      <strong>{formatAccountPayback(economics.payback, locale, economics.paybackApproximate)}</strong>
    </div>
  </div>;
}
