import i18next from "i18next";
import { initReactI18next } from "react-i18next";
import { loadTranslations, normalizeLanguage } from "./resources";
import { createLanguageController, readLanguagePreference } from "./controller";

const controller = createLanguageController(i18next, loadTranslations);
let initialization: Promise<unknown> | undefined;
let explicitSelection = false;

export async function initI18n(systemLocale?: string | null) {
  const language = normalizeLanguage(readLanguagePreference() ?? systemLocale);
  if (!initialization) {
    i18next.use(initReactI18next);
    initialization = controller.initialize(language).catch(error => {
      initialization = undefined;
      throw error;
    });
    await initialization;
  } else {
    await initialization;
    if (!explicitSelection) await controller.change(normalizeLanguage(readLanguagePreference() ?? systemLocale), false);
  }
  return i18next;
}

export async function setI18nLanguage(locale?: string | null) {
  explicitSelection = true;
  await initialization;
  return controller.change(locale);
}

export { normalizeLanguage };
