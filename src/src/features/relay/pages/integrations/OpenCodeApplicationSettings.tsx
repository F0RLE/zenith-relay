import { useTranslation } from "react-i18next";
import { ApplicationLaunchPath } from "./ApplicationLaunchPath";

export function OpenCodeApplicationSettings() {
  const { t } = useTranslation();
  return <section className="gateway-tab-panel" role="tabpanel" aria-label={t("gateway.tabs.opencode")}>
    <ApplicationLaunchPath application="opencode" />
    <div className="gateway-opencode-panel">
      <div className="gateway-opencode-development" role="status">
        <strong>{t("gateway.openCodeInDevelopment")}</strong>
      </div>
    </div>
  </section>;
}
