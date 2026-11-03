import type { en } from "./locales/en";
import type { TranslationResource } from "./resources";
import { canonicalLocale, type LocaleDefinition } from "./registry";

type LeafKeys<T> = { [K in keyof T & string]: T[K] extends string | readonly unknown[] ? K : T[K] extends object ? `${K}.${LeafKeys<T[K]>}` : never }[keyof T & string];
export type MessageKey = LeafKeys<typeof en>;
type TranslationShape<T> = { [K in keyof T as K extends `${infer Base}_other` ? K | `${Base}_${Intl.LDMLPluralRule}` : K]?: K extends keyof T ? T[K] extends string ? string : T[K] extends readonly unknown[] ? readonly string[] : TranslationShape<T[K]> : string };
/** `satisfies Translation` checks TS dictionaries without importing them at startup. */
export type Translation = TranslationShape<typeof en>;

export function flattenTranslations(resource: unknown, prefix = "", result: Record<string, string> = {}): Record<string, string> {
  if (!resource || typeof resource !== "object") throw new Error(`Invalid translation object: ${prefix || "root"}`);
  for (const [key, value] of Object.entries(resource)) {
    const path = prefix ? `${prefix}.${key}` : key;
    if (["__proto__", "constructor", "prototype"].includes(key) || key.includes(".")) throw new Error(`Invalid translation key: ${path}`);
    if (typeof value === "string") {
      if (!value.trim()) throw new Error(`Empty translation: ${path}`);
      result[path] = value;
    } else flattenTranslations(value, path, result);
  }
  return result;
}

const plural = /_(zero|one|two|few|many|other)$/;
const parameters = (text: string) => [...text.matchAll(/{{\s*-?\s*([^},]+)[^}]*}}/g)].map(match => match[1]!.trim()).sort().join("|");

export function validateTranslations(code: string, resource: TranslationResource, english: TranslationResource, complete: boolean) {
  const reference = flattenTranslations(english), messages = flattenTranslations(resource);
  const referenceFor = (key: string) => reference[key] ?? reference[key.replace(plural, "_other")];
  for (const [key, text] of Object.entries(messages)) {
    const source = referenceFor(key);
    if (source === undefined) throw new Error(`${code}: unknown key ${key}`);
    if (parameters(text) !== parameters(source)) throw new Error(`${code}: interpolation differs at ${key}`);
    if (/<\/?[a-z][^>]*>/i.test(text) && !/<\/?[a-z][^>]*>/i.test(source)) throw new Error(`${code}: unexpected markup at ${key}`);
  }
  const ordinary = Object.keys(reference).filter(key => !plural.test(key));
  const groups = [...new Set(Object.keys(reference).filter(key => plural.test(key)).map(key => key.replace(plural, "")))];
  const required = [...ordinary, ...groups.flatMap(group => new Intl.PluralRules(code).resolvedOptions().pluralCategories.map(category => `${group}_${category}`))];
  const missing = required.filter(key => messages[key] === undefined);
  if (complete && missing.length) throw new Error(`${code}: ${missing.length} missing translations (${missing.slice(0, 4).join(", ")})`);
  return { translated: required.length - missing.length, total: required.length, missing };
}

export function validateRegistry(entries: readonly LocaleDefinition[]) {
  const codes = new Set<string>();
  for (const locale of entries) {
    if (!/^[A-Za-z]{2,8}(?:-[A-Za-z0-9]{1,8})*$/.test(locale.code) || canonicalLocale(locale.code) !== locale.code || codes.has(locale.code)) throw new Error(`Invalid or duplicate locale: ${locale.code}`);
    codes.add(locale.code);
    if (!locale.nativeName?.trim() || !locale.englishName?.trim() || !["ltr", "rtl"].includes(locale.direction) || !["ts", "json"].includes(locale.format) || !Array.isArray(locale.fallbacks) || !/^[A-Za-z]{2,8}(?:-[A-Za-z0-9]{1,8})*$/.test(locale.helpLocale)) throw new Error(`Invalid locale metadata: ${locale.code}`);
  }
  if (!codes.has("en")) throw new Error("English fallback is required");
  const aliases = new Set(codes);
  for (const locale of entries) {
    for (const alias of locale.aliases ?? []) {
      if (canonicalLocale(alias) !== alias || aliases.has(alias)) throw new Error(`Invalid or ambiguous locale alias: ${alias}`);
      aliases.add(alias);
    }
  }
  const visit = (code: string, path: string[]) => {
    if (!codes.has(code)) throw new Error(`Unknown fallback: ${code}`);
    if (path.includes(code)) throw new Error(`Cyclic fallback: ${[...path, code].join(" -> ")}`);
    entries.find(locale => locale.code === code)!.fallbacks.forEach(next => visit(next, [...path, code]));
  };
  entries.forEach(locale => visit(locale.code, []));
}
