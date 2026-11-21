import type { Locale } from "../../i18n/catalog";
import { formatMoney } from "../usage/model";

export const executionLabel = { reserved: "receipt.hold", dispatched: "ownerUsage.permit", completed: "status.completed",
  incomplete: "receipt.incomplete", failed: "status.error", uncertain: "receipt.uncertain" } as const;
export const accountingLabel = { held: "status.held", pending: "report.awaitingAccounting",
  settled: "report.settled", released: "report.released" } as const;
export const issueLabel = { execution: "ownerUsage.causeUnknown", settlement: "report.awaitingAccounting",
  usage_evidence: "ownerUsage.usageUnconfirmed", credit_pending: "ownerUsage.creditPending" } as const;

export function utcStamp(at: number, locale: Locale, exact = false) {
  return new Intl.DateTimeFormat(locale, { timeZone: "UTC", year: "numeric", month: "short", day: "numeric",
    hour: "2-digit", minute: "2-digit", second: "2-digit", hourCycle: "h23", ...(exact ? { fractionalSecondDigits: 3 } as const : {}) })
    .format(at) + " UTC";
}

export const usd = (amount: bigint | null, locale: Locale) => amount === null ? "—" : formatMoney(amount, 6, locale) + " USD";
