import { useTranslation } from "react-i18next";
import type { AccountSummary } from "../api/types";
import { buildAccountValueProjection } from "../accountEconomics";
import { formatAccountValueMicroUsd } from "../poolFormatting";

export function AccountValueStrip({ account }: { account: AccountSummary }) {
  const { t, i18n } = useTranslation();
  const locale = i18n.resolvedLanguage ?? i18n.language;
  const { purchaseCostMicroUsd: purchaseCost, remainingApiEquivalent, payback, approximate } = buildAccountValueProjection(
    account.apiEquivalent,
    account.quota,
    account.quotaWindowUsage,
    account.purchaseCostMicroUsd,
  );
  const paybackTitle = purchaseCost == null
    ? t("accounts.accountValue.purchaseMissing")
    : t("accounts.accountValue.paybackHint", {
      used: formatAccountValueMicroUsd(account.apiEquivalent.microUsd, locale),
      purchase: formatAccountValueMicroUsd(purchaseCost, locale),
    });

  return <dl className="account-value-strip" data-columns={remainingApiEquivalent == null ? 2 : 3}>
    <div title={t("accounts.accountValue.usedHint", { count: account.apiEquivalent.unpricedTokens })}>
      <dt>{t("accounts.accountValue.used")}</dt>
      <dd>{formatAccountValueMicroUsd(account.apiEquivalent.microUsd, locale, approximate)}</dd>
    </div>
    {remainingApiEquivalent ? <div title={t("accounts.accountValue.remainingHint")}>
      <dt>{t("accounts.accountValue.remaining")}</dt>
      <dd>{formatAccountValueMicroUsd(remainingApiEquivalent.microUsd, locale, remainingApiEquivalent.approximate)}</dd>
    </div> : null}
    <div title={paybackTitle} data-state={payback != null && payback >= 1 ? "paid" : undefined}>
      <dt>{t("accounts.accountValue.payback")}</dt>
      <dd>{payback == null ? "—" : `${approximate ? "≈" : ""}${new Intl.NumberFormat(locale, { style: "percent", maximumFractionDigits: 0 }).format(payback)}`}</dd>
    </div>
  </dl>;
}
