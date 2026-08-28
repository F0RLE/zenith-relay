import type { UsageTotals } from "../../api/types";

export function formatUsageApiEquivalent(value: UsageTotals["apiEquivalent"], locale: string) {
  if (!value.pricedTokens && value.unpricedTokens) return "-";
  const amount = new Intl.NumberFormat(locale, {
    style: "currency",
    currency: "USD",
    minimumFractionDigits: 2,
    maximumFractionDigits: 4,
  }).format(value.microUsd / 1_000_000);
  return `≈${amount}`;
}
