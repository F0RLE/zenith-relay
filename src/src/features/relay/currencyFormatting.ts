import { getNumberFormatter } from "./numberFormatting";

export type UsdFractionDigits = Pick<Intl.NumberFormatOptions, "minimumFractionDigits" | "maximumFractionDigits">;

/** Formats USD consistently while callers retain control over their precision policy. */
export function formatUsd(value: number, locale: string, fractionDigits: UsdFractionDigits = {}) {
  return getNumberFormatter(locale, {
    style: "currency",
    currency: "USD",
    ...fractionDigits,
  }).format(value);
}

export function formatMicroUsd(value: number, locale: string, fractionDigits: UsdFractionDigits = {}) {
  return formatUsd(value / 1_000_000, locale, fractionDigits);
}

/** Converts a priced API-equivalent total to USD. Unpriced-only totals stay empty. */
export function pricedUsd(value: { microUsd: number; pricedTokens: number }): number | null {
  if (value.pricedTokens <= 0) return null;
  return value.microUsd / 1_000_000;
}

/** Chart-scale USD: more digits for sub-dollar amounts, two digits above one dollar. */
export function formatScaledUsd(value: number, locale: string) {
  return formatUsd(value, locale, {
    minimumFractionDigits: 2,
    maximumFractionDigits: value < 0.01 ? 6 : value < 1 ? 4 : 2,
  });
}

export function formatApproximateUsd(value: number | null, locale: string) {
  return value == null ? "—" : `≈${formatScaledUsd(value, locale)}`;
}
