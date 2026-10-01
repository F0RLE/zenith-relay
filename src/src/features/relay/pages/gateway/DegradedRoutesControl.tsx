import { useId } from "react";
import { useTranslation } from "react-i18next";
import { ToggleSwitch } from "../../components/Ui";
import { useRelayState } from "../../state/RelayStateProvider";

export function DegradedRoutesControl() {
  const { t } = useTranslation();
  const { mode, runtime, busy, blockDegradedRoutesEnabled, setBlockDegradedRoutesEnabled } = useRelayState();
  const id = useId();
  if (mode === "zenith" || !runtime || (mode === "remote" && !runtime.capabilities.features.includes("block_degraded_routes"))) return null;
  return <div className="gateway-api-toggle-setting" aria-busy={busy === "block-degraded-routes"}>
    <div className="relay-toggle-setting">
      <label htmlFor={id} data-relay-tooltip={t("gateway.degradedRoutesHint")}>
        <strong>{t("gateway.degradedRoutes")}</strong>
        <span id={`${id}-description`}>{t("gateway.degradedRoutesDescription")}</span>
      </label>
      <ToggleSwitch
        id={id}
        label={t("gateway.degradedRoutes")}
        aria-describedby={`${id}-description`}
        checked={blockDegradedRoutesEnabled}
        disabled={busy === "block-degraded-routes"}
        onChange={(enabled) => void setBlockDegradedRoutesEnabled(enabled)}
      />
    </div>
  </div>;
}
