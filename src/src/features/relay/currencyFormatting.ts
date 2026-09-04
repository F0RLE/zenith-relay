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
