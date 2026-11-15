import { localeDefinition, resolveLanguage } from "./registry";
export { fallbackLanguage, supportedLanguages } from "./registry";
export const normalizeLanguage = resolveLanguage;
export type SupportedLanguage = string;
export type TranslationResource = { [key: string]: string | readonly string[] | TranslationResource };

/** One registry entry and a data file; no component or loader branches per locale. */
export async function loadTranslations(language: SupportedLanguage): Promise<TranslationResource> {
  const locale = localeDefinition(language);
  const module = locale.format === "json"
    ? await import(`./locales/${locale.code}.json`)
    : await import(`./locales/${locale.code}.ts`);
  return module.default ?? module[locale.code];
}
