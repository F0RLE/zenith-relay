import { useTranslation } from "react-i18next";
import type { AccountSummary } from "../api/types";
import { subscriptionExpiryFormatter } from "../hooks/useRelativeTimeClock";

export function AccountCredentialDiagnostics({ account }: { account: AccountSummary }) {
  const { t, i18n } = useTranslation();
  const diagnostic = account.credentialDiagnostics;
  if (account.oauthClientKind !== "excel_bps" || !diagnostic || account.remoteLocation) return null;
  const expiry = diagnostic.expiresAtMs;
  const date = expiry != null && Number.isFinite(expiry) && Math.abs(expiry) <= 8.64e15
    ? subscriptionExpiryFormatter(i18n.resolvedLanguage ?? i18n.language).format(expiry)
    : null;
  return <div className="account-credential-diagnostics" data-status={diagnostic.status}>
    <strong>{t("accounts.credentials.title")}: {t(`accounts.credentials.states.${diagnostic.status}`)}</strong>
    {date ? <span>{t("accounts.credentials.expires", { date })}</span> : null}
    {diagnostic.recovery !== "none" ? <span>{t(`accounts.credentials.recovery.${diagnostic.recovery}`)}</span> : null}
    <small>{t("accounts.credentials.localEvidence")}</small>
  </div>;
}
