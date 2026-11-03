import { CheckCheck, Clock3, Info, Loader2, LogIn, RefreshCw } from "lucide-react";
import { useTranslation } from "react-i18next";
import type { AccountSummary } from "../api/types";
import { accountQuotaRefreshState } from "../accountStatus";
import { accountMetadataHint, accountMetadataIsStale, accountMetadataSources } from "../accountRoutes";
import { isFastSupplementalQuota } from "../quotaFormatting";
import { QuotaStack } from "./Ui";
import { useRelayState } from "../state/RelayStateProvider";

/** Keeps quota and reauthentication state identical on every account card. */
export function AccountQuotaPanel({ account, nowMs, onReauthenticate }: { account: AccountSummary; nowMs: number; onReauthenticate: (account: AccountSummary, automatic?: boolean) => void }) {
  const { t, i18n } = useTranslation();
  const { runtime } = useRelayState();
  const canAutomate = account.oauthClientKind === "excel_bps";
  const status = accountQuotaRefreshState(account);
  const source = accountMetadataSources(account, runtime?.accounts ?? []).quota;
  const linked = source.id !== account.id;
  const hasQuota = Boolean(source.quota.primary || source.quota.secondary || source.quota.supplemental?.some((supplementalQuota) => !isFastSupplementalQuota(supplementalQuota)));

  if (status === "requires_reauth") {
    return <button type="button" className="account-quota-refresh-state requires_reauth is-action" data-relay-tooltip={canAutomate ? t("accounts.automaticSignInShortcut") : undefined} onClick={() => onReauthenticate(account)} onContextMenu={(event) => {
      if (!canAutomate) return;
      event.preventDefault();
      event.stopPropagation();
      onReauthenticate(account, true);
    }}><LogIn aria-hidden /><span>{t("accounts.quotaRefreshStatus.requires_reauth")}</span></button>;
  }

  if (hasQuota) return <QuotaStack snapshot={source.quota} nowMs={nowMs} concise
    hint={linked ? accountMetadataHint(source, "quota", nowMs, t, i18n.language) : undefined}
    stale={linked && accountMetadataIsStale(source, "quota", nowMs)} />;

  const icon = status === "not_available"
    ? <Info aria-hidden />
    : status === "refreshing"
    ? <Loader2 className="spin" aria-hidden />
    : status === "updated"
      ? <CheckCheck aria-hidden />
      : status === "failed"
        ? <RefreshCw aria-hidden />
        : <Clock3 aria-hidden />;
  return <div className={`account-quota-refresh-state ${status}`} role="status">{icon}<span>{account.quota.limitReached ? t("pool.memberStatus.quotaWait") : t(`accounts.quotaRefreshStatus.${status}`)}</span></div>;
}
