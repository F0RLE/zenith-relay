import { pathToFileURL } from "node:url";
import { en } from "../../src/src/i18n/locales/en.ts";

/** Print a translation starter; this never registers or publishes a language. */
export function localeTemplate(code, source = en) {
  const locale = Intl.getCanonicalLocales(code)[0];
  if (!locale) throw new Error("A canonical locale code is required");
  const categories = new Intl.PluralRules(locale).resolvedOptions().pluralCategories;
  const plural = /_(zero|one|two|few|many|other)$/;
  const visit = value => {
    if (typeof value === "string") return value;
    if (Array.isArray(value)) return value.map(visit);
    const result = {};
    for (const [key, child] of Object.entries(value)) {
      if (!plural.test(key)) result[key] = visit(child);
      else {
        const base = key.replace(plural, "");
        for (const category of categories) result[`${base}_${category}`] = value[`${base}_${category}`] ?? value[`${base}_other`];
      }
    }
    return result;
  };
  return visit(source);
}

if (import.meta.url === pathToFileURL(process.argv[1]).href) {
  try { console.log(JSON.stringify(localeTemplate(process.argv[2]), null, 2)); }
  catch (error) { console.error(error.message); process.exitCode = 1; }
}
