import type { ReactNode } from "react";
import { useTranslation } from "react-i18next";
import type { AccountSummary } from "../api/types";
import { canDisplayAccountQuota } from "../accountStatus";
import { accountLifetimeEconomics, accountPaybackHint, buildAccountValueProjection, formatAccountPayback, formatAccountUsageValue, formatRemainingApiEquivalent, remainingValueDeadlines, remainingValueHint } from "../accountEconomics";
import { useRelativeTimeClock } from "../hooks/useRelativeTimeClock";
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
  const economics = accountLifetimeEconomics(account);
  const showRemaining = canDisplayAccountQuota(account);
  const nowMs = useRelativeTimeClock(showRemaining ? remainingValueDeadlines(account.quotaWindowUsage) : []);
  const { purchaseCostMicroUsd: purchaseCost, remainingApiEquivalent, payback, approximate } = buildAccountValueProjection(
    economics.apiEquivalent,
    showRemaining ? account.quota : undefined,
    showRemaining ? account.quotaWindowUsage : undefined,
    economics.purchaseCostMicroUsd,
    nowMs,
  );
  const paybackTitle = accountPaybackHint(economics.apiEquivalent, purchaseCost, locale, t);

  return <dl className="account-value-strip" data-columns={showRemaining ? 3 : 2}>
    <AccountValueMetric title={t("accounts.accountValue.usedHint", { count: economics.apiEquivalent.unpricedTokens })}>
      <dt>{t("accounts.accountValue.used")}</dt>
      <dd>{formatAccountUsageValue(economics.apiEquivalent, locale)}</dd>
    </AccountValueMetric>
    {showRemaining ? <AccountValueMetric title={remainingValueHint(remainingApiEquivalent, locale, t)}>
      <dt>{t(remainingApiEquivalent && !remainingApiEquivalent.approximate ? "accounts.accountValue.remainingDirect" : "accounts.accountValue.remaining")}</dt>
      <dd>{formatRemainingApiEquivalent(remainingApiEquivalent, locale)}</dd>
    </AccountValueMetric> : null}
    <AccountValueMetric title={paybackTitle} {...(payback != null && payback >= 1 ? { state: "paid" } : {})}>
      <dt>{t("accounts.accountValue.payback")}</dt>
      <dd>{formatAccountPayback(payback, locale, approximate)}</dd>
    </AccountValueMetric>
  </dl>;
}
