import type { i18n } from "i18next";
import { localeDefinition } from "./registry";

export const STARTUP_COPY_KEY = "relay.startupCopy";
export const nativeKeys = ["open", "quit", "startPool", "stopPool", "poolRunning", "poolStopped", "signIn", "connected", "closeWindow"] as const;
export type NativeMessages = Record<(typeof nativeKeys)[number], string>;
let nativeRevision = 0;
let nativeWrite: Promise<void> = Promise.resolve();

/** Serialize optional native preference writes; discard queued obsolete choices. */
export function synchronizeNativeLanguage(write: () => Promise<void>): Promise<void> {
  const revision = ++nativeRevision;
  nativeWrite = nativeWrite.catch(() => undefined).then(async () => {
    if (revision === nativeRevision) await write();
  });
  return nativeWrite;
}

export function applyLanguagePresentation(instance: i18n) {
  const locale = localeDefinition(instance.language);
  document.documentElement.lang = locale.code;
  document.documentElement.dir = locale.direction;
  const copy = Object.fromEntries(["interface", "runtime", "slow", "error", "reload", "renderError", "renderRecovery"].map(key => [key, instance.t(`startup.${key}`)]));
  // Small public UI copy only; never user names, provider data or credentials.
  try { localStorage.setItem(STARTUP_COPY_KEY, JSON.stringify({ locale: locale.code, aliases: locale.aliases ?? [], direction: locale.direction, copy })); } catch { /* Optional display cache. */ }
  const messages = Object.fromEntries(nativeKeys.map(key => [key, instance.t(`native.${key}`)])) as NativeMessages;
  return { language: locale.code, messages };
}
