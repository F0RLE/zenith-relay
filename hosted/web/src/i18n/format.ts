import { useMemo } from "react";
import { useLocale } from "./LocaleProvider";
import * as display from "../features/usage/model";

/** Bind pure, exact formatters to the current React locale. */
export function useFormatters() {
  const { locale } = useLocale();
  return useMemo(() => ({
    formatCompact: (value: number) => new Intl.NumberFormat(locale, { notation: "compact", maximumFractionDigits: 1 }).format(value),
    formatPercent: (value: number, digits = 1) => new Intl.NumberFormat(locale, { style: "percent", maximumFractionDigits: digits }).format(value),
    formatInteger: (value: number | null) => display.formatInteger(value, locale),
    formatMoney: (value: bigint | null, digits = 6) => display.formatMoney(value, digits, locale),
    formatTime: (iso: string) => display.formatTime(iso, locale),
    formatDuration: (ms: number | null) => display.formatDuration(ms, locale),
    formatSpeed: (value: number | null) => display.formatSpeed(value, locale),
    supportSummary: (record: display.RequestRecord) => display.supportSummary(record, locale),
  }), [locale]);
}
