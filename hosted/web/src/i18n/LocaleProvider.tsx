import { createContext, useCallback, useContext, useEffect, useMemo, useRef, useState } from "react";
import type { ReactNode } from "react";
import { defaultLocale, isMessageKey, isLocaleLoaded, loadLocale, localeDefinition, localeStorageKey, resolveLocale, translate } from "./catalog";
import type { Locale } from "./catalog";
import type { Translator } from "./types";
import "./locale-status.css";

type LocaleContextValue = {
  locale: Locale;
  setLocale: (locale: Locale) => void;
  t: Translator;
  /** Known presentation labels only. Never pass account/key names or raw provider content. */
  localize: (message: string) => string;
};

const LocaleContext = createContext<LocaleContextValue | null>(null);

export function LocaleProvider({ children }: { children: ReactNode }) {
  const [locale, updateLocale] = useState<Locale>(defaultLocale);
  const [failed, setFailed] = useState<Locale | null>(null);
  const requestId = useRef(0);

  const selectLocale = useCallback((next: Locale, persist: boolean) => {
    const selected = resolveLocale(next);
    const id = ++requestId.current;
    setFailed(null);
    const commit = () => {
      if (id !== requestId.current) return;
      updateLocale(selected);
      if (persist) {
        try { localStorage.setItem(localeStorageKey, selected); }
        catch { /* Language switching also works without browser storage. */ }
      }
    };
    if (isLocaleLoaded(selected)) commit();
    else void loadLocale(selected).then(commit, () => { if (id === requestId.current) setFailed(selected); });
  }, []);
  const setLocale = useCallback((next: Locale) => selectLocale(next, true), [selectLocale]);

  useEffect(() => {
    try { selectLocale(resolveLocale(localStorage.getItem(localeStorageKey)), false); }
    catch { /* English is ready even if storage is inaccessible. */ }
    return () => { requestId.current++; };
  }, [selectLocale]);

  useEffect(() => {
    document.documentElement.lang = locale;
    document.documentElement.dir = localeDefinition(locale).direction;
  }, [locale]);

  useEffect(() => {
    function sync(event: StorageEvent) {
      if (event.key === localeStorageKey || event.key === null) selectLocale(resolveLocale(event.newValue), false);
    }
    window.addEventListener("storage", sync);
    return () => window.removeEventListener("storage", sync);
  }, [selectLocale]);

  const value = useMemo<LocaleContextValue>(() => ({ locale, setLocale, t: (key, params) => translate(locale, key, params), localize: message => isMessageKey(message) ? translate(locale, message) : message }), [locale, setLocale]);
  return <LocaleContext.Provider value={value}>{children}{failed && <div className="locale-load-error" role="alert"><span>{value.t("language.loadError")}</span><button type="button" onClick={() => setLocale(failed)}>{value.t("common.retry")}</button><button type="button" aria-label={value.t("common.close")} onClick={() => setFailed(null)}>×</button></div>}</LocaleContext.Provider>;
}

export function useLocale() {
  const context = useContext(LocaleContext);
  if (!context) throw new Error("useLocale must be used within LocaleProvider");
  return context;
}
