import { CalendarDays } from "lucide-react";
import { useTranslation } from "react-i18next";
import { subscriptionExpiryFormatter } from "../hooks/useRelativeTimeClock";
import { formatDetailedRemainingTime } from "../quotaFormatting";

/** One subscription date presentation for Connections and Pool account cards. */
export function AccountSubscriptionLine({ activeUntilMs, nowMs }: { activeUntilMs: number | null; nowMs: number }) {
  const { t, i18n } = useTranslation();
  if (activeUntilMs == null) return null;

  const expired = activeUntilMs != null && activeUntilMs <= nowMs;
  const date = subscriptionExpiryFormatter(i18n.resolvedLanguage ?? i18n.language).format(activeUntilMs);
  const remaining = formatDetailedRemainingTime(activeUntilMs, nowMs, t);

  return <div className={`account-subscription-line${expired ? " expired" : ""}`}><CalendarDays aria-hidden /><span>{date}</span>{remaining ? <><span className="account-subscription-separator" aria-hidden>·</span><span className="account-subscription-countdown">{remaining}</span></> : null}</div>;
}
