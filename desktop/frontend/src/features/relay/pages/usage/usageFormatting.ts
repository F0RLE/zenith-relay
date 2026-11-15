import type { UsageTotals } from "../../api/types";
import { formatMicroUsd } from "../../currencyFormatting";

export function formatUsageApiEquivalent(apiEquivalent: UsageTotals["apiEquivalent"], locale: string, maximumFractionDigits = 4) {
  if (!apiEquivalent.pricedTokens && apiEquivalent.unpricedTokens && apiEquivalent.microUsd === 0) return "—";
  const amount = formatMicroUsd(apiEquivalent.microUsd, locale, {
    minimumFractionDigits: 2,
    maximumFractionDigits,
  });
  return `≈${amount}`;
}
