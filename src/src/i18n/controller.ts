import type { i18n } from "i18next";
import type { TranslationResource } from "./resources";
import { fallbackLanguage, languageChain, locales, resolveLanguage, type LocaleDefinition } from "./registry";

export const LANGUAGE_KEY = "relay.language";

export function readLanguagePreference(): string | null {
  try { return localStorage.getItem(LANGUAGE_KEY); } catch { return null; }
}

export function createLanguageController(instance: i18n, load: (language: string) => Promise<TranslationResource>, available: readonly LocaleDefinition[] = locales) {
  let revision = 0;
  const pending = new Map<string, Promise<TranslationResource>>();
  const retained = new Set<string>();
  const resolve = (value?: string | null) => resolveLanguage(value, available);
  const chain = (value?: string | null) => languageChain(value, available);
  const ensure = (language: string) => {
    if (instance.hasResourceBundle(language, "translation")) return Promise.resolve(instance.getResourceBundle(language, "translation") as TranslationResource);
    let request = pending.get(language);
    if (!request) {
      request = load(language).finally(() => { pending.delete(language); });
      pending.set(language, request);
    }
    return request;
  };
  return {
    async initialize(language?: string | null) {
      await instance.init({
        resources: {}, lng: fallbackLanguage, supportedLngs: available.map(locale => locale.code),
        fallbackLng: code => chain(code).filter(candidate => candidate !== resolve(code)),
        load: "currentOnly", interpolation: { escapeValue: false }, returnNull: false, returnEmptyString: false,
      });
      try { return await this.change(language, false); }
      catch { return this.change(fallbackLanguage, false); }
    },
    async change(value?: string | null, persist = true): Promise<boolean> {
      const current = ++revision;
      const language = resolve(value);
      const required = chain(language);
      let bundles: TranslationResource[];
      try { bundles = await Promise.all(required.map(ensure)); }
      catch (error) { if (current !== revision) return false; throw error; }
      if (current !== revision) return false;
      required.forEach((code, index) => instance.addResourceBundle(code, "translation", bundles[index]!, true, true));
      for (const code of required) { retained.delete(code); retained.add(code); }
      // Bound i18next's store. ES-module caching is separately browser-owned.
      for (const code of retained) {
        if (retained.size <= Math.max(4, required.length)) break;
        if (!required.includes(code)) { instance.removeResourceBundle(code, "translation"); retained.delete(code); }
      }
      await instance.changeLanguage(language);
      if (current !== revision) return false;
      if (persist) {
        try { localStorage.setItem(LANGUAGE_KEY, language); } catch { /* Preference persistence is optional. */ }
      }
      return true;
    },
  };
}
