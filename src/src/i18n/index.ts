import i18next from "i18next";
import { initReactI18next } from "react-i18next";
import { loadTranslations, normalizeLanguage } from "./resources";

const LANGUAGE_KEY = "relay.language";

export async function initI18n(systemLocale?: string | null) {
  const language = normalizeLanguage(localStorage.getItem(LANGUAGE_KEY) ?? systemLocale);
  if (i18next.isInitialized) {
    await setI18nLanguage(language);
    return i18next;
  }

  const translations = await loadTranslations(language);
  await i18next.use(initReactI18next).init({
    resources: { [language]: { translation: translations } },
    lng: language,
    fallbackLng: false,
    interpolation: {
      escapeValue: false,
    },
    returnNull: false,
  });
  i18next.on("languageChanged", (value) => localStorage.setItem(LANGUAGE_KEY, normalizeLanguage(value)));

  return i18next;
}

export async function setI18nLanguage(locale?: string | null) {
  const language = normalizeLanguage(locale);
  if (!i18next.hasResourceBundle(language, "translation")) {
    const translations = await loadTranslations(language);
    i18next.addResourceBundle(language, "translation", translations, true, true);
  }
  await i18next.changeLanguage(language);
}

export { normalizeLanguage };
