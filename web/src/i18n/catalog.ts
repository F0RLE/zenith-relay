import en from "./messages/en.json" with { type: "json" };
import { localeDefinition } from "./registry";
import type { Locale } from "./registry";
import type { MessageKey, MessageParams, Messages } from "./types";

export { defaultLocale, locales, localeStorageKey, resolveLocale, localeDefinition } from "./registry";
export type { Locale } from "./registry";

const loaded = new Map<Locale, Partial<Messages>>([["en", en]]);
const pending = new Map<Locale, Promise<Partial<Messages>>>();

export function isMessageKey(value: string): value is MessageKey { return Object.hasOwn(en, value); }
export function isLocaleLoaded(locale: Locale): boolean { return loaded.has(locale); }

/** Deduplicate downloads and allow a failed language load to be retried. */
export function loadLocale(locale: Locale): Promise<Partial<Messages>> {
  const ready = loaded.get(locale);
  if (ready) return Promise.resolve(ready);
  const existing = pending.get(locale);
  if (existing) return existing;
  const request = localeDefinition(locale).load().then(messages => {
    loaded.set(locale, messages);
    return messages;
  }).finally(() => pending.delete(locale));
  pending.set(locale, request);
  return request;
}

/** English covers absent/empty messages. Substitution never interprets HTML. */
export function formatMessage(messages: Partial<Messages>, key: MessageKey, params: MessageParams = {}): string {
  const translated = messages[key];
  const message = typeof translated === "string" && translated.trim() ? translated : en[key];
  return message.replace(/\{(\w+)\}/g, (placeholder, name: string) => Object.hasOwn(params, name) ? String(params[name]) : placeholder);
}

/** Non-React callers load a non-English resource before using it. */
export function translate(locale: Locale, key: MessageKey, params: MessageParams = {}): string {
  return formatMessage(loaded.get(locale) ?? en, key, params);
}
