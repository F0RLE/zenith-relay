import { KeyRound } from "lucide-react";
import { useTranslation } from "react-i18next";
import type { AccountSummary } from "../api/types";
import { subscriptionExpiryFormatter } from "../hooks/useRelativeTimeClock";
import { formatDetailedRemainingTime } from "../quotaFormatting";

/** Informational only: reaching expiry never changes the account or its tokens. */
export function AccountTokenExpiry({ account, nowMs }: { account: AccountSummary; nowMs: number }) {
  const { t, i18n } = useTranslation();
  if (!account.credentialDiagnostics || account.recoveryRequired) return null;

  const expiry = account.credentialDiagnostics.expiresAtMs;
  const known = expiry != null && Number.isFinite(expiry) && Math.abs(expiry) <= 8.64e15;
  const remaining = known
    ? expiry <= nowMs
      ? t("accounts.tokenExpiry.expired")
      : formatDetailedRemainingTime(expiry, nowMs, t)
    : "—";
  const label = t("accounts.tokenExpiry.label", { remaining });
  const hint = known
    ? t("accounts.tokenExpiry.expires", {
      date: subscriptionExpiryFormatter(i18n.resolvedLanguage ?? i18n.language).format(expiry),
    })
    : t("accounts.tokenExpiry.unknown");

  return <div className="account-token-expiry" aria-label={label} data-relay-tooltip={hint}>
    <KeyRound aria-hidden />
    <span>{remaining}</span>
  </div>;
}
