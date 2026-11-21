import { Calendar, KeyRound } from "../../components/Icons";
import type { OwnerAccountPresentation } from "./accountPresentation";
import { ownerAccountValue, ownerQuotaPercent, ownerQuotaWindows, ownerRemainingTime } from "./accountPresentation";
import { useLocale } from "../../i18n/LocaleProvider";

const statusLabels = {
  pending: "account.quotasNotReceivedYet", updated: "account.limitsNotReported", refreshing: "account.refreshingQuotas",
  failed: "account.couldNotRefreshQuotas", requires_reauth: "account.signInAgain", not_available: "account.quotasUnavailable",
} as const;

/** Same section order as Desktop AccountCard and PoolMemberCard. */
export function OwnerAccountBody({ account, nowMs, valuesVisible = true }: { account?: OwnerAccountPresentation | undefined; nowMs: number; valuesVisible?: boolean }) {
  const { t, locale } = useLocale();
  const windows = ownerQuotaWindows(account, locale);
  const value = ownerAccountValue(account, nowMs, locale);
  const remainingTime = (target: number | null) => ownerRemainingTime(target, nowMs, locale);
  const expiry = account?.tokenExpiresAtMs ?? null;
  const until = account?.subscriptionUntilMs ?? null;
  const validUntil = until != null && Number.isFinite(until) && Math.abs(until) <= 8.64e15;
  const credits = account?.quota.availableCreditsMicroUnits;
  const hasCredits = account?.quota.providerCreditsUnlimited || credits != null && Number.isSafeInteger(credits) && credits > 0;
  return <>
    <div className="owner-account-quota">
      {windows.length ? windows.map(({ id, label, window }) => {
        const percent = ownerQuotaPercent(window.availableBasisPoints);
        const expired = window.resetAtMs != null && window.resetAtMs <= nowMs;
        return <div className="owner-quota-meter" key={id} data-level={percent == null ? "unknown" : percent <= 5 ? "critical" : percent <= 20 ? "low" : "normal"} data-stale={expired || undefined}
          data-relay-tooltip={expired ? t("account.expiredWindowHint") : undefined} tabIndex={expired ? 0 : undefined}>
          <span>{label}</span><strong>{percent == null ? "—" : `${percent}%`}</strong>
          <small>{expired ? t("account.awaitingRefresh") : remainingTime(window.resetAtMs)}</small>
          <div className="owner-quota-track" role="meter" aria-label={t("account.quotaRemaining", { label })} aria-valuemin={0} aria-valuemax={100} aria-valuenow={percent ?? undefined} aria-valuetext={percent == null ? t("common.unknown") : expired ? t("account.lastMeasurement", { percent }) : `${percent}%`}>
            {percent != null && <span style={{ width: `${percent}%` }} />}
          </div>
        </div>;
      }) : <div className="owner-quota-empty" role="status" data-state={account?.quotaStatus ?? "pending"}>{t(statusLabels[account?.quotaStatus ?? "pending"])}</div>}
    </div>
    {hasCredits && <div className="owner-provider-credits"><span>{t("account.providerCredits")}</span><strong>{account?.quota.providerCreditsUnlimited ? "∞" : (credits! / 1000000).toLocaleString(locale, { maximumFractionDigits: 1 })}</strong></div>}
    {account?.tokenExpiresAtMs !== undefined && account.quotaStatus !== "requires_reauth" && <div className="owner-account-token" data-relay-tooltip={t(expiry == null ? "account.tokenExpiryUnknown" : "account.tokenExpiryHint")} tabIndex={0} aria-label={t("account.tokenRemaining", { remaining: expiry != null && expiry <= nowMs ? t("account.expired") : remainingTime(expiry) })}><KeyRound aria-hidden="true" /><span>{expiry != null && expiry <= nowMs ? t("account.expired") : remainingTime(expiry)}</span></div>}
    {validUntil && <div className="owner-account-subscription" data-expired={until <= nowMs || undefined} data-relay-tooltip={t("account.validUntilUtc")} tabIndex={0}>
      <Calendar aria-hidden="true" /><span>{new Intl.DateTimeFormat(locale, { day: "2-digit", month: "2-digit", year: "numeric", hour: "2-digit", minute: "2-digit", timeZone: "UTC" }).format(until)}</span>
      <span aria-hidden="true">·</span><strong>{until <= nowMs ? t("account.expired") : remainingTime(until)}</strong>
    </div>}
    {valuesVisible && <dl className="owner-account-values">
      <div tabIndex={0} data-relay-tooltip={t("account.usageHint")}><dt>{t("account.apiEquivalentUsage")}</dt><dd>{value.used}</dd></div>
      <div tabIndex={0} data-relay-tooltip={t(value.direct ? "account.directBalanceHint" : "account.remainingHint")}><dt>{t(value.direct ? "account.apiEquivalentRemaining" : "account.weekRemaining")}</dt><dd>{value.remaining}</dd></div>
      <div tabIndex={0} data-relay-tooltip={t("account.paybackHint")} data-paid={value.paid || undefined}><dt>{t("account.payback")}</dt><dd>{value.payback}</dd></div>
    </dl>}
  </>;
}
