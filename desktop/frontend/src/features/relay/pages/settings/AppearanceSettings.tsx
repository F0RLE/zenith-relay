import type { ReactNode } from "react";
import { Palette } from "lucide-react";
import { useTranslation } from "react-i18next";
import { LanguagePicker } from "../../../../i18n/LanguagePicker";
import { useRelayState } from "../../state/RelayStateProvider";

export function SettingsGroup({ icon, title, children }: { icon: ReactNode; title: string; children: ReactNode }) {
  return <section className="settings-group"><header>{icon}<h2>{title}</h2></header><div className="settings-group-body">{children}</div></section>;
}

export function AppearanceSettings() {
  const { t } = useTranslation();
  const { theme, setTheme } = useRelayState();
  return <SettingsGroup icon={<Palette aria-hidden />} title={t("settings.appearance")}>
    <div className="settings-control-row">
      <div><strong>{t("settings.language")}</strong></div>
      <LanguagePicker />
    </div>
    <div className="settings-control-row">
      <div><strong>{t("settings.theme")}</strong></div>
      <div className="segmented settings-theme-control" role="group" aria-label={t("settings.theme")}>
        {(["system", "light", "dark"] as const).map((themeName) => (
          <button key={themeName} type="button" className={theme === themeName ? "active" : ""} aria-pressed={theme === themeName} onClick={() => setTheme(themeName)}>
            {t(`settings.themes.${themeName}`)}
          </button>
        ))}
      </div>
    </div>
  </SettingsGroup>;
}
