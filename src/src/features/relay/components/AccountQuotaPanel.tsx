import { CheckCheck, Clock3, Loader2, LogIn, RefreshCw } from "lucide-react";
import { useTranslation } from "react-i18next";
import type { AccountSummary } from "../api/types";
import { accountQuotaRefreshState } from "../accountStatus";
import { isFastSupplementalQuota } from "../quotaFormatting";
import { QuotaStack } from "./Ui";

/** Keeps quota and reauthentication state identical on every account card. */
export function AccountQuotaPanel({ account, nowMs, onReauthenticate }: { account: AccountSummary; nowMs: number; onReauthenticate: (account: AccountSummary) => void }) {
  const { t } = useTranslation();
  const status = accountQuotaRefreshState(account);
  const hasQuota = Boolean(account.quota.primary || account.quota.secondary || account.quota.supplemental?.some((item) => !isFastSupplementalQuota(item)));

  if (status === "requires_reauth") {
    return <button type="button" className="account-quota-refresh-state requires_reauth is-action" onClick={() => onReauthenticate(account)}><LogIn aria-hidden /><span>{t("accounts.quotaRefreshStatus.requires_reauth")}</span></button>;
  }
  if (hasQuota) return <QuotaStack snapshot={account.quota} nowMs={nowMs} concise />;

  const icon = status === "refreshing"
    ? <Loader2 className="spin" aria-hidden />
    : status === "updated"
      ? <CheckCheck aria-hidden />
      : status === "failed"
        ? <RefreshCw aria-hidden />
        : <Clock3 aria-hidden />;
  return <div className={`account-quota-refresh-state ${status}`} role="status">{icon}<span>{t(`accounts.quotaRefreshStatus.${status}`)}</span></div>;
}
