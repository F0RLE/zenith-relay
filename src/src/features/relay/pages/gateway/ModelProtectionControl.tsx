import { useId } from "react";
import { useTranslation } from "react-i18next";
import { ToggleSwitch } from "../../components/Ui";
import { persistRoutingPolicy } from "../../routingPolicy";
import { useRelayState } from "../../state/RelayStateProvider";
import { usePendingFlag } from "../../state/usePendingFlag";

export function ModelProtectionControl() {
  const { mode, runtime } = useRelayState();
  if (!runtime || mode === "zenith") return null;
  if (mode === "remote" && typeof runtime.gateway.basisPointsEnabled !== "boolean") return null;
  if (!runtime.gateway.basisPointsEnabled && !runtime.accounts.some((account) => account.basisPointsAvailable)) return null;
  return <ModelProtectionToggle key={`${mode}:${runtime.runtimeTarget.serverId ?? "local"}`} />;
}

function ModelProtectionToggle() {
  const { t } = useTranslation();
  const { mode, runtime, perform } = useRelayState();
  const id = useId();
  const saved = Boolean(runtime?.gateway.basisPointsEnabled);
  const { checked, saving, select } = usePendingFlag(saved);
  if (!runtime) return null;
  const { gateway } = runtime;
  const change = (enabled: boolean) => {
    if (enabled === checked || saving) return;
    select(enabled, () => perform("gateway-basis-points", () => persistRoutingPolicy(mode, {
      maxRetryCandidates: gateway.maxRetryCandidates,
      defaultServiceTier: gateway.defaultServiceTier,
      basisPointsEnabled: enabled,
    }), "feedback.saved", { backgroundRefresh: true, uiLock: false }));
  };

  return <div className="model-protection-control gateway-api-toggle-setting">
    <div className="relay-toggle-setting">
      <label htmlFor={id} data-relay-tooltip={t("gateway.modelProtectionHint")}>
        <strong>{t("gateway.modelProtection")}</strong>
        <span id={`${id}-description`}>{t("gateway.modelProtectionDescription")}</span>
      </label>
      <ToggleSwitch id={id} label={t("gateway.modelProtection")}
        aria-describedby={`${id}-description`}
        checked={checked}
        disabled={saving}
        onChange={(enabled) => void change(enabled)} />
    </div>
  </div>;
}
