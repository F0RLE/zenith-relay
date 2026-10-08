import { useId } from "react";
import { useTranslation } from "react-i18next";
import { ToggleSwitch } from "../../components/Ui";
import { useRelayState } from "../../state/RelayStateProvider";
import { usePendingFlag } from "../../state/usePendingFlag";

export function RouteRecoveryControl() {
  const { t } = useTranslation();
  const { mode, runtime, routeRecoveryEnabled, setRouteRecoveryEnabled } = useRelayState();
  const controlId = useId();
  const { checked, select } = usePendingFlag(routeRecoveryEnabled);
  // Older servers only apply the legacy setting to ChatGPT requests.
  if (mode === "zenith" || !runtime || (mode === "remote" && !runtime.capabilities.features.includes("route_recovery_v1"))) return null;
  return <div className="gateway-api-toggle-setting">
    <div className="relay-toggle-setting">
      <label htmlFor={controlId} data-relay-tooltip={t("gateway.routeRecoveryHint")}>
        <strong>{t("gateway.routeRecovery")}</strong>
        <span id={`${controlId}-description`}>{t("gateway.routeRecoveryDescription")}</span>
      </label>
      <ToggleSwitch
        id={controlId}
        label={t("gateway.routeRecovery")}
        aria-describedby={`${controlId}-description`}
        checked={checked}
        onChange={(enabled) => select(enabled, () => setRouteRecoveryEnabled(enabled))}
      />
    </div>
  </div>;
}
