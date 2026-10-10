import { useId } from "react";
import { useTranslation } from "react-i18next";
import { ToggleSwitch } from "../../components/Ui";
import { useRelayState } from "../../state/RelayStateProvider";
import { usePendingFlag } from "../../state/usePendingFlag";

export function DegradedRoutesControl() {
  const { t } = useTranslation();
  const { mode, runtime, blockDegradedRoutesEnabled, setBlockDegradedRoutesEnabled } = useRelayState();
  const controlId = useId();
  const { checked, saving, select } = usePendingFlag(blockDegradedRoutesEnabled);
  if (mode === "zenith" || !runtime || (mode === "remote" && !runtime.capabilities.features.includes("block_degraded_routes"))) return null;
  return <div className="integration-account-policy">
    <div className="relay-toggle-setting">
      <label htmlFor={controlId} data-relay-tooltip={t("gateway.degradedRoutesHint")}>
        <strong>{t("gateway.degradedRoutes")}</strong>
        <span id={`${controlId}-description`}>{t("gateway.degradedRoutesDescription")}</span>
      </label>
      <ToggleSwitch
        id={controlId}
        label={t("gateway.degradedRoutes")}
        aria-describedby={`${controlId}-description`}
        checked={checked}
        disabled={saving}
        onChange={(enabled) => select(enabled, () => setBlockDegradedRoutesEnabled(enabled))}
      />
    </div>
  </div>;
}
