import { RotateCcw } from "lucide-react";
import { useTranslation } from "react-i18next";

type SettingsPanelProps = {
  canReset: boolean;
  onReset: () => void;
};

export function SettingsPanel({ canReset, onReset }: SettingsPanelProps) {
  const { t } = useTranslation();

  return (
    <section className="settings-panel" aria-label={t("settings.label")}>
      <button className="settings-zone" type="button" disabled={!canReset} onClick={onReset}>
        <span className="settings-action-main">
          <RotateCcw aria-hidden />
          <span>{t("actions.reset")}</span>
        </span>
        <span className="settings-hint">{t("settings.resetHint")}</span>
      </button>
    </section>
  );
}
