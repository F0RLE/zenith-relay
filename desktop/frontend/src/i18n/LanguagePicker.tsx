import { useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import { OptionMenu } from "../features/relay/components/ui/menus";
import { setI18nLanguage } from "./index";
import { locales, resolveLanguage } from "./registry";

const options = locales.map(locale => ({
  value: locale.code, label: locale.nativeName, shortLabel: locale.code.toUpperCase(),
  searchText: `${locale.englishName} ${locale.code} ${locale.aliases?.join(" ") ?? ""}`,
}));

export function LanguagePicker({ compact = false }: { compact?: boolean }) {
  const { t, i18n } = useTranslation();
  const [failed, setFailed] = useState(false);
  const [pending, setPending] = useState(false);
  const revision = useRef(0);
  const select = async (code: string) => {
    const current = ++revision.current;
    setFailed(false); setPending(true);
    try { await setI18nLanguage(code); }
    catch { if (revision.current === current) setFailed(true); }
    finally { if (revision.current === current) setPending(false); }
  };
  return <div className="relay-language-picker" aria-busy={pending}>
    <OptionMenu className={compact ? "setup-language-menu" : "field-option-menu"}
      listClassName="relay-language-options" label={t("settings.language")}
      value={resolveLanguage(i18n.language)} align={compact ? "center" : "end"} fitContent
      options={compact ? options : options.map(({ shortLabel: _shortLabel, ...option }) => option)}
      search={{ label: t("localization.search"), empty: t("localization.noMatches") }}
      onChange={code => void select(code)} />
    {failed ? <small role="alert">{t("localization.loadFailed")}</small> : null}
  </div>;
}
