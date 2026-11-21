import en from "./messages/en.json" with { type: "json" };
import type { Messages } from "./types";

interface LocaleDefinition {
  code: string;
  label: string;
  shortLabel: string;
  direction: "ltr" | "rtl";
  weekStart: number;
  load: () => Promise<Partial<Messages>>;
}

/** Only metadata is eager. New languages need a resource and one registry entry. */
export const locales = [
  { code: "ru", label: "Русский", shortLabel: "RU", direction: "ltr", weekStart: 1, load: () => import("./messages/ru.json").then(module => module.default) },
  { code: "en", label: "English", shortLabel: "EN", direction: "ltr", weekStart: 0, load: () => Promise.resolve(en) },
  { code: "zh-CN", label: "简体中文", shortLabel: "中文", direction: "ltr", weekStart: 1, load: () => import("./messages/zh-CN.json").then(module => module.default) },
] as const satisfies readonly LocaleDefinition[];

export type Locale = (typeof locales)[number]["code"];
export const defaultLocale: Locale = "en";
export const localeStorageKey = "relay-hosted-locale";

export function resolveLocale(value: unknown): Locale {
  return locales.find(locale => locale.code === value)?.code ?? defaultLocale;
}

export function localeDefinition(code: Locale) {
  return locales.find(locale => locale.code === code)!;
}
