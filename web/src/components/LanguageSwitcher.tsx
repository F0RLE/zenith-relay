import { useEffect, useState } from "react";
import { locales, resolveLocale } from "../i18n/catalog";
import { useLocale } from "../i18n/LocaleProvider";
import { OptionMenu } from "./OptionMenu";
import { LanguageFlag } from "./LanguageFlag";
import "./language-switcher.css";

const desktopMenuQuery = "(min-width: 701px) and (hover: hover) and (pointer: fine)";
const languageOptions = locales.map(entry => ({ value: entry.code, label: entry.label, shortLabel: entry.shortLabel, lang: entry.code, icon: <LanguageFlag locale={entry.code} /> }));

export function LanguageSwitcher() {
  const { locale, setLocale, t } = useLocale();
  const [desktopMenu, setDesktopMenu] = useState(() => matchMedia(desktopMenuQuery).matches);
  const [pointerFocus, setPointerFocus] = useState(false);

  useEffect(() => {
    const media = matchMedia(desktopMenuQuery);
    const sync = () => { setDesktopMenu(media.matches); setPointerFocus(false); };
    sync();
    media.addEventListener("change", sync);
    return () => media.removeEventListener("change", sync);
  }, []);

  if (desktopMenu) return <div className="language-switcher">
    <OptionMenu label={t("language.label")} value={locale} options={languageOptions}
      onChange={value => setLocale(resolveLocale(value))}
      triggerClassName="key-button key-secondary language-trigger" listClassName="language-menu" iconOnly excludeSelected orientation="horizontal" />
  </div>;

  return <div className="language-switcher">
    <select className="key-button key-secondary language-select" aria-label={t("language.label")} value={locale}
      data-pointer-focus={pointerFocus} onPointerDown={() => setPointerFocus(true)}
      onKeyDown={() => setPointerFocus(false)} onBlur={() => setPointerFocus(false)}
      onChange={event => setLocale(resolveLocale(event.currentTarget.value))}>
      {locales.map(entry => <option key={entry.code} value={entry.code} lang={entry.code}>{entry.label}</option>)}
    </select>
    <span className="language-native-flag"><LanguageFlag locale={locale} /></span>
  </div>;
}
