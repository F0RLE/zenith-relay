import { useState } from "react";
import { LandingLanguageSwitcher } from "../../components/LandingLanguageSwitcher";
import { RelayMark } from "../../components/RelayMark";
import { ThemeIcon } from "../../components/ThemeIcon";
import type { useTheme } from "../../components/useTheme";
import { useLocale } from "../../i18n/LocaleProvider";

export function PublicHeader({ brand, theme, toggleTheme, cabinet = false }: ReturnType<typeof useTheme> & { brand: string; cabinet?: boolean }) {
  const { t } = useLocale();
  const [open, setOpen] = useState(false);
  return <header className="landing-header"><div className="landing-container landing-header-inner">
    <a className="landing-brand" href="/" aria-label={brand === "Relay" ? t("landing.home") : brand}><RelayMark className="landing-mark" /><strong>{brand}</strong></a>
    <div className="landing-header-actions">
      <LandingLanguageSwitcher open={open} onOpenChange={setOpen} />
      <button type="button" className="key-button key-secondary key-icon" data-theme-toggle aria-label={t(theme === "light" ? "theme.dark" : "theme.light")} onClick={toggleTheme}><ThemeIcon theme={theme} /></button>
      {cabinet && <a className="key-button key-secondary" href="/app/overview">{t("landing.cabinet")}</a>}
    </div>
  </div></header>;
}
