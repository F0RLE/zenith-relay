import { defaultLocale, localeDefinition } from "../i18n/catalog";
import type { Locale } from "../i18n/catalog";

const dateFormatters = new Map<Locale, Intl.DateTimeFormat>();
function dateFormatter(locale: Locale) {
  let formatter = dateFormatters.get(locale);
  if (!formatter) {
    formatter = new Intl.DateTimeFormat(locale, { year: "numeric", month: "2-digit", day: "2-digit", timeZone: "UTC", calendar: "gregory", numberingSystem: "latn" });
    dateFormatters.set(locale, formatter);
  }
  return formatter;
}

/** Calendar-only arithmetic: ISO storage never passes through the local timezone. */
export function calendarDate(iso: string): Date {
  const [year, month, day] = iso.split("-").map(Number);
  const date = new Date(0);
  date.setUTCFullYear(year!, month! - 1, day!);
  return date;
}

export function calendarISO(date: Date): string {
  return `${String(date.getUTCFullYear()).padStart(4, "0")}-${String(date.getUTCMonth() + 1).padStart(2, "0")}-${String(date.getUTCDate()).padStart(2, "0")}`;
}

/** Empty is inherited policy; null means invalid input, not an empty policy. */
export function parseCalendarInput(text: string, locale: Locale = defaultLocale): string | null {
  const trimmed = text.trim();
  if (!trimmed) return "";
  // Locale order and separators come from Intl, not per-language branches.
  const fields: string[] = [];
  const pattern = dateFormatter(locale).formatToParts(calendarDate("2006-11-22")).map(part => {
    if (part.type === "literal") return part.value.replace(/[.*+?^${}()|[\]\\]/g, "\\$&");
    fields.push(part.type);
    return part.type === "year" ? "(\\d{4})" : "(\\d{1,2})";
  }).join("");
  const compact = fields.map(field => field === "year" ? "(\\d{4})" : "(\\d{2})").join("");
  const local = new RegExp(`^${pattern}$`).exec(trimmed) ?? new RegExp(`^${compact}$`).exec(trimmed);
  const parts = local && Object.fromEntries(fields.map((field, index) => [field, local[index + 1]!]));
  const iso = parts ? `${parts.year!.padStart(4, "0")}-${parts.month!.padStart(2, "0")}-${parts.day!.padStart(2, "0")}` : trimmed;
  if (!/^\d{4}-\d{2}-\d{2}$/.test(iso) || iso.startsWith("0000")) return null;
  return calendarISO(calendarDate(iso)) === iso ? iso : null;
}

export function displayCalendarDate(iso: string, locale: Locale = defaultLocale): string {
  return iso ? dateFormatter(locale).format(calendarDate(iso)) : "";
}

export function todayCalendarISO(): string {
  const now = new Date();
  return `${String(now.getFullYear()).padStart(4, "0")}-${String(now.getMonth() + 1).padStart(2, "0")}-${String(now.getDate()).padStart(2, "0")}`;
}

export function moveCalendarDays(iso: string, days: number): string {
  const date = calendarDate(iso);
  date.setUTCDate(date.getUTCDate() + days);
  return calendarISO(date);
}

export function moveCalendarMonths(iso: string, months: number): string {
  const date = calendarDate(iso), day = date.getUTCDate();
  date.setUTCDate(1);
  date.setUTCMonth(date.getUTCMonth() + months + 1);
  date.setUTCDate(0);
  date.setUTCDate(Math.min(day, date.getUTCDate()));
  return calendarISO(date);
}

export function calendarWeekStart(locale: Locale = defaultLocale) { return localeDefinition(locale).weekStart; }

export function calendarDays(iso: string, locale: Locale = defaultLocale): string[] {
  const first = `${iso.slice(0, 7)}-01`;
  const offset = (calendarDate(first).getUTCDay() - calendarWeekStart(locale) + 7) % 7;
  return Array.from({ length: 42 }, (_, index) => moveCalendarDays(first, index - offset));
}
