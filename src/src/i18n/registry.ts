import definitions from "./registry.json";

export interface LocaleDefinition {
  code: string;
  nativeName: string;
  englishName: string;
  direction: "ltr" | "rtl";
  fallbacks: readonly string[];
  helpLocale: string;
  format: "ts" | "json";
  aliases?: readonly string[];
}

export const fallbackLanguage = "en";
export const locales = definitions as readonly LocaleDefinition[];
export const supportedLanguages: readonly string[] = locales.map(({ code }) => code);

/** Accept OS spelling without losing script or region subtags. */
export function canonicalLocale(value?: string | null): string | null {
  if (!value || value.length > 100) return null;
  try {
    return Intl.getCanonicalLocales((value.split(/[.@]/)[0] ?? "").replaceAll("_", "-"))[0] ?? null;
  } catch {
    return null;
  }
}

export function resolveLanguage(value?: string | null, available: readonly LocaleDefinition[] = locales): string {
  const code = canonicalLocale(value);
  if (code) {
    // Explicit fallbacks belong to the registry. Never guess a different script
    // (e.g. zh-Hant -> zh-Hans) from a shared language prefix.
    const parts = code.split("-");
    while (parts.length) {
      const candidate = parts.join("-");
      const match = available.find(locale => locale.code === candidate || locale.aliases?.includes(candidate));
      if (match) return match.code;
      parts.pop();
    }
  }
  return fallbackLanguage;
}

export function localeDefinition(value?: string | null): LocaleDefinition {
  return locales.find(locale => locale.code === resolveLanguage(value))!;
}

export function languageChain(value?: string | null, available: readonly LocaleDefinition[] = locales): string[] {
  const result: string[] = [];
  const visit = (code: string) => {
    if (result.includes(code)) return;
    result.push(code);
    available.find(locale => locale.code === code)?.fallbacks.forEach(visit);
  };
  visit(resolveLanguage(value, available));
  visit(fallbackLanguage);
  return result;
}
