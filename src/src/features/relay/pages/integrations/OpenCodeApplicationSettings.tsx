import { useTranslation } from "react-i18next";

export function OpenCodeApplicationSettings() {
  const { t } = useTranslation();
  return <section className="gateway-tab-panel" role="tabpanel" aria-label={t("gateway.tabs.opencode")}>
    <div className="gateway-opencode-panel">
      <div className="gateway-opencode-development" role="status">
        <strong>{t("gateway.openCodeInDevelopment")}</strong>
      </div>
    </div>
  </section>;
}
