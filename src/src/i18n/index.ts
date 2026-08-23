import i18next from "i18next";
import { initReactI18next } from "react-i18next";
import { fallbackLanguage, normalizeLanguage, resources } from "./resources";

const LANGUAGE_KEY = "relay.language";

export async function initI18n(systemLocale?: string | null) {
  const language = normalizeLanguage(localStorage.getItem(LANGUAGE_KEY) ?? systemLocale);
  if (i18next.isInitialized) {
    await i18next.changeLanguage(language);
    return i18next;
  }

  await i18next.use(initReactI18next).init({
    resources,
    lng: language,
    fallbackLng: fallbackLanguage,
    interpolation: {
      escapeValue: false,
    },
    returnNull: false,
  });
  i18next.on("languageChanged", (value) => localStorage.setItem(LANGUAGE_KEY, normalizeLanguage(value)));

  return i18next;
}

export { normalizeLanguage };
