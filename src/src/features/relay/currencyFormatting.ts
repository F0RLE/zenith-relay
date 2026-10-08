import { getNumberFormatter } from "./numberFormatting";

export type UsdFractionDigits = Pick<Intl.NumberFormatOptions, "minimumFractionDigits" | "maximumFractionDigits">;

/** Formats USD consistently while callers retain control over their precision policy. */
export function formatUsd(usdAmount: number, locale: string, fractionDigits: UsdFractionDigits = {}) {
  return getNumberFormatter(locale, {
    style: "currency",
    currency: "USD",
    ...fractionDigits,
  }).format(usdAmount);
}

export function formatMicroUsd(microUsdAmount: number, locale: string, fractionDigits: UsdFractionDigits = {}) {
  return formatUsd(microUsdAmount / 1_000_000, locale, fractionDigits);
}

/** Converts a priced API-equivalent total to USD. Unpriced-only totals stay empty. */
export function pricedUsd(priceSummary: { microUsd: number; pricedTokens: number }): number | null {
  if (priceSummary.pricedTokens <= 0) return null;
  return priceSummary.microUsd / 1_000_000;
}

/** Chart-scale USD: more digits for sub-dollar amounts, two digits above one dollar. */
export function formatScaledUsd(usdAmount: number, locale: string) {
  return formatUsd(usdAmount, locale, {
    minimumFractionDigits: 2,
    maximumFractionDigits: usdAmount < 0.01 ? 6 : usdAmount < 1 ? 4 : 2,
  });
}

export function formatApproximateUsd(usdAmount: number | null, locale: string) {
  return usdAmount == null ? "—" : `≈${formatScaledUsd(usdAmount, locale)}`;
}
