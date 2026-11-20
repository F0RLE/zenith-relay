import { useTranslation } from "react-i18next";
import { ApplicationLaunchSettings } from "./ApplicationLaunchPath";
import { ApplicationProtocolControl } from "./ApplicationProtocolControl";

export function OpenCodeApplicationSettings() {
  const { t } = useTranslation();
  return <section className="gateway-tab-panel" role="tabpanel" aria-label={t("gateway.tabs.opencode")}>
    <ApplicationLaunchSettings application="opencode" />
    <div className="gateway-settings-panel">
      <ApplicationProtocolControl application="opencode" />
    </div>
    <div className="gateway-opencode-panel">
      <div className="gateway-opencode-development" role="status">
        <strong>{t("gateway.openCodeInDevelopment")}</strong>
      </div>
    </div>
  </section>;
}
