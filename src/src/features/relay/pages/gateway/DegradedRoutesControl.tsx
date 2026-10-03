import { useId } from "react";
import { useTranslation } from "react-i18next";
import { ToggleSwitch } from "../../components/Ui";
import { useRelayState } from "../../state/RelayStateProvider";
import { usePendingFlag } from "../../state/usePendingFlag";

export function DegradedRoutesControl() {
  const { t } = useTranslation();
  const { mode, runtime, blockDegradedRoutesEnabled, setBlockDegradedRoutesEnabled } = useRelayState();
  const id = useId();
  const { checked, saving, select } = usePendingFlag(blockDegradedRoutesEnabled);
  if (mode === "zenith" || !runtime || (mode === "remote" && !runtime.capabilities.features.includes("block_degraded_routes"))) return null;
  return <div className="gateway-api-toggle-setting">
    <div className="relay-toggle-setting">
      <label htmlFor={id} data-relay-tooltip={t("gateway.degradedRoutesHint")}>
        <strong>{t("gateway.degradedRoutes")}</strong>
        <span id={`${id}-description`}>{t("gateway.degradedRoutesDescription")}</span>
      </label>
      <ToggleSwitch
        id={id}
        label={t("gateway.degradedRoutes")}
        aria-describedby={`${id}-description`}
        checked={checked}
        disabled={saving}
        onChange={(enabled) => select(enabled, () => setBlockDegradedRoutesEnabled(enabled))}
      />
    </div>
  </div>;
}
