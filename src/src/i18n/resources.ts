export const fallbackLanguage = "en";
export const supportedLanguages = ["en", "ru"] as const;

export type SupportedLanguage = (typeof supportedLanguages)[number];

export function normalizeLanguage(locale?: string | null): SupportedLanguage {
  const language = locale?.toLowerCase().split(/[._-]/)[0];
  return language === "ru" ? "ru" : fallbackLanguage;
}

export async function loadTranslations(language: SupportedLanguage) {
  if (language === "ru") return (await import("./locales/ru")).ru;
  return (await import("./locales/en")).en;
}
