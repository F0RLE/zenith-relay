import { en } from "./locales/en";
import { ru } from "./locales/ru";

export const fallbackLanguage = "en";
export const supportedLanguages = ["en", "ru"] as const;

export type SupportedLanguage = (typeof supportedLanguages)[number];

export const resources = {
  en: { translation: en },
  ru: { translation: ru },
} as const;

export function normalizeLanguage(locale?: string | null): SupportedLanguage {
  const language = locale?.toLowerCase().split(/[._-]/)[0];
  return language === "ru" ? "ru" : fallbackLanguage;
}
