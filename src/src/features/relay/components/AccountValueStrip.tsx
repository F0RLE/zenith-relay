import type { ReactNode } from "react";
import { useTranslation } from "react-i18next";
import type { AccountSummary } from "../api/types";
import { buildAccountValueProjection, formatAccountPayback } from "../accountEconomics";
import { formatAccountValueMicroUsd } from "../poolFormatting";
import { useTooltip } from "./Ui";

function AccountValueMetric({ title, state, children }: { title: string; state?: string; children: ReactNode }) {
  const tooltip = useTooltip<HTMLDivElement>(title);
  return <>
    <div
      ref={tooltip.anchorRef}
      data-state={state}
      aria-describedby={tooltip.describedBy}
      onMouseEnter={tooltip.show}
      onMouseLeave={tooltip.hideAfterHover}
      onPointerDown={tooltip.pointerStart}
    >
      {children}
    </div>
    {tooltip.tooltip}
  </>;
}

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

  return <dl className="account-value-strip" data-columns={remainingApiEquivalent ? 3 : 2}>
    <AccountValueMetric title={t("accounts.accountValue.usedHint", { count: account.apiEquivalent.unpricedTokens })}>
      <dt>{t("accounts.accountValue.used")}</dt>
      <dd>{formatAccountValueMicroUsd(account.apiEquivalent.microUsd, locale, approximate)}</dd>
    </AccountValueMetric>
    {remainingApiEquivalent ? <AccountValueMetric title={t("accounts.accountValue.remainingHint")}>
      <dt>{t("accounts.accountValue.remaining")}</dt>
      <dd>{formatAccountValueMicroUsd(remainingApiEquivalent.microUsd, locale, remainingApiEquivalent.approximate)}</dd>
    </AccountValueMetric> : null}
    <AccountValueMetric title={paybackTitle} {...(payback != null && payback >= 1 ? { state: "paid" } : {})}>
      <dt>{t("accounts.accountValue.payback")}</dt>
      <dd>{formatAccountPayback(payback, locale, approximate)}</dd>
    </AccountValueMetric>
  </dl>;
}
