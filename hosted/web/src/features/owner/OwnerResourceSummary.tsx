import type { ReactNode } from "react";
import { ArrowUpRight, Wallet } from "../../components/Icons";
import type { OwnerConnection } from "./model";
import { ownerAccountTotals } from "./accountPresentation";
import { useLocale } from "../../i18n/LocaleProvider";
import type { MessageKey } from "../../i18n/types";

export type ResourceStat = { label: MessageKey; value: number; icon: ReactNode };

export function OwnerResourceSummary({ stats, accounts, nowMs, valuesVisible }: {
  stats: ResourceStat[]; accounts: OwnerConnection[]; nowMs: number; valuesVisible: boolean;
}) {
  const { t, locale } = useLocale();
  const total = ownerAccountTotals(accounts.filter(item => item.kind === "account"), nowMs, locale);
  return <div className="owner-resource-summary-band" data-values={valuesVisible || undefined}>
    <dl className="owner-resource-summary" aria-label={t("account.connectionInventory")}>{stats.map(stat => <div key={stat.label} tabIndex={0} data-relay-tooltip={t(stat.label)}>
      <dt>{stat.icon}<span className="sr-only">{t(stat.label)}</span></dt><dd>{stat.value.toLocaleString(locale)}</dd>
    </div>)}</dl>
    {valuesVisible && <dl className="owner-resource-economics" aria-label={t("account.accountApiEquivalents")}>
      <div tabIndex={0} data-relay-tooltip={t("account.summaryUsageHint", { known: total.usageCount, total: total.accountCount })}>
        <dt><span aria-hidden="true">$</span><span className="sr-only">{t("account.apiEquivalentUsage")}</span></dt><dd>{total.used}</dd>
      </div>
      <div tabIndex={0} data-relay-tooltip={t("account.summaryRemainingHint", { known: total.remainingCount, total: total.remainingTotalCount })}>
        <dt><Wallet aria-hidden="true" /><span className="sr-only">{t("account.apiEquivalentRemaining")}</span></dt><dd>{total.remaining}</dd>
      </div>
      <div tabIndex={0} data-paid={total.paid || undefined} data-relay-tooltip={t("account.summaryPaybackHint", { known: total.purchaseCount, total: total.accountCount })}>
        <dt><ArrowUpRight aria-hidden="true" /><span className="sr-only">{t("account.payback")}</span></dt><dd>{total.payback}</dd>
      </div>
    </dl>}
  </div>;
}
