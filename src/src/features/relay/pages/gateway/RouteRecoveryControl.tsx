import { useId } from "react";
import { useTranslation } from "react-i18next";
import { ToggleSwitch } from "../../components/Ui";
import { useRelayState } from "../../state/RelayStateProvider";

export function RouteRecoveryControl() {
  const { t } = useTranslation();
  const { mode, runtime, busy, routeRecoveryEnabled, setRouteRecoveryEnabled } = useRelayState();
  const id = useId();
  // Older servers only apply the legacy setting to ChatGPT requests.
  if (mode === "zenith" || !runtime || (mode === "remote" && !runtime.capabilities.features.includes("route_recovery_v1"))) return null;
  return <div className="gateway-api-toggle-setting" aria-busy={busy === "route-recovery"}>
    <div className="relay-toggle-setting">
      <label htmlFor={id} data-relay-tooltip={t("gateway.routeRecoveryHint")}>
        <strong>{t("gateway.routeRecovery")}</strong>
        <span id={`${id}-description`}>{t("gateway.routeRecoveryDescription")}</span>
      </label>
      <ToggleSwitch
        id={id}
        label={t("gateway.routeRecovery")}
        aria-describedby={`${id}-description`}
        checked={routeRecoveryEnabled}
        disabled={busy === "route-recovery"}
        onChange={(enabled) => void setRouteRecoveryEnabled(enabled)}
      />
    </div>
  </div>;
}
