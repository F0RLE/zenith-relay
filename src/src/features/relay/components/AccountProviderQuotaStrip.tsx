import { useTranslation } from "react-i18next";
import type { AccountSummary } from "../api/types";
import { formatNumber } from "../numberFormatting";

const CREDIT_MICRO_UNITS = 1_000_000;

function availableCredits(value: number | null | undefined) {
  if (value == null || !Number.isSafeInteger(value) || value < 0) return null;
  return value / CREDIT_MICRO_UNITS;
}

/**
 * Renders an upstream credit ledger only when a provider explicitly
 * reported it. It is distinct from reset credits, API-equivalent estimates,
 * and any API-money balance.
 */
export function AccountProviderQuotaStrip({ account }: { account: AccountSummary }) {
  const { t, i18n } = useTranslation();
  const credits = availableCredits(account.quota.availableCreditsMicroUnits);
  const unlimited = account.quota.providerCreditsUnlimited === true;
  if (!unlimited && (credits == null || credits <= 0)) return null;

  return <dl className="account-provider-quota-strip" data-relay-tooltip={t("accounts.providerCredits.hint")}>
    <dt>{t("accounts.providerCredits.label")}</dt>
    <dd>{unlimited ? "∞" : formatNumber(credits!, i18n.resolvedLanguage ?? i18n.language, { maximumFractionDigits: 1 })}</dd>
  </dl>;
}
