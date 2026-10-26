import type { ReactNode } from "react";
import { useTranslation } from "react-i18next";
import type { AccountSummary } from "../api/types";
import { accountPaybackHint, buildAccountValueProjection, formatAccountPayback, remainingValueDeadlines, remainingValueHint } from "../accountEconomics";
import { useRelativeTimeClock } from "../hooks/useRelativeTimeClock";
import { formatAccountValueMicroUsd } from "../poolFormatting";
import { useTooltip } from "./Ui";

function AccountValueMetric({ title, state, children }: { title: string; state?: string; children: ReactNode }) {
  const tooltip = useTooltip<HTMLDivElement>(title);
  return <>
    <div
      ref={tooltip.anchorRef}
      tabIndex={0}
      data-state={state}
      aria-describedby={tooltip.describedBy}
      onMouseEnter={tooltip.show}
      onMouseLeave={tooltip.hideAfterHover}
      onFocus={tooltip.show}
      onBlur={tooltip.hideAfterHover}
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
  const nowMs = useRelativeTimeClock(remainingValueDeadlines(account.quotaWindowUsage));
  const { purchaseCostMicroUsd: purchaseCost, remainingApiEquivalent, payback, approximate } = buildAccountValueProjection(
    account.apiEquivalent,
    account.quota,
    account.quotaWindowUsage,
    account.purchaseCostMicroUsd,
    nowMs,
  );
  const paybackTitle = accountPaybackHint(account.apiEquivalent, purchaseCost, locale, t);

  return <dl className="account-value-strip" data-columns={3}>
    <AccountValueMetric title={t("accounts.accountValue.usedHint", { count: account.apiEquivalent.unpricedTokens })}>
      <dt>{t("accounts.accountValue.used")}</dt>
      <dd>{formatAccountValueMicroUsd(account.apiEquivalent.microUsd, locale, approximate)}</dd>
    </AccountValueMetric>
    <AccountValueMetric title={remainingValueHint(remainingApiEquivalent, locale, t)}>
      <dt>{t(remainingApiEquivalent && !remainingApiEquivalent.approximate ? "accounts.accountValue.remainingDirect" : "accounts.accountValue.remaining")}</dt>
      <dd>{remainingApiEquivalent ? formatAccountValueMicroUsd(remainingApiEquivalent.microUsd, locale, remainingApiEquivalent.approximate) : "—"}</dd>
    </AccountValueMetric>
    <AccountValueMetric title={paybackTitle} {...(payback != null && payback >= 1 ? { state: "paid" } : {})}>
      <dt>{t("accounts.accountValue.payback")}</dt>
      <dd>{formatAccountPayback(payback, locale, approximate)}</dd>
    </AccountValueMetric>
  </dl>;
}
