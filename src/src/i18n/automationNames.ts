// Persisted default names are recognized across languages without loading both
// full translation bundles. Custom names stay exactly as the user entered them.
export const automationDefaultNames = {
  en: {
    defaultName: "Start quota countdown",
    weeklyDefaultName: "Reset weekly quota",
  },
  ru: {
    defaultName: "Запустить отсчёт квоты",
    weeklyDefaultName: "Сбросить недельную квоту",
  },
} as const;
