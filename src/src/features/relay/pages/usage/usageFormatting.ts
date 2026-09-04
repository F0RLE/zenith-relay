import type { UsageTotals } from "../../api/types";
import { formatMicroUsd } from "../../currencyFormatting";

export function formatUsageApiEquivalent(value: UsageTotals["apiEquivalent"], locale: string) {
  if (!value.pricedTokens && value.unpricedTokens) return "—";
  const amount = formatMicroUsd(value.microUsd, locale, {
    minimumFractionDigits: 2,
    maximumFractionDigits: 4,
  });
  return `≈${amount}`;
}
