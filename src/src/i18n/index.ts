import i18next from "i18next";
import { initReactI18next } from "react-i18next";
import { fallbackLanguage, normalizeLanguage, resources } from "./resources";

export async function initI18n(systemLocale?: string | null) {
  if (i18next.isInitialized) {
    await i18next.changeLanguage(normalizeLanguage(systemLocale));
    return i18next;
  }

  await i18next.use(initReactI18next).init({
    resources,
    lng: normalizeLanguage(systemLocale),
    fallbackLng: fallbackLanguage,
    interpolation: {
      escapeValue: false,
    },
    returnNull: false,
  });

  return i18next;
}

export { normalizeLanguage };
