import { useId, useState } from "react";
import { useTranslation } from "react-i18next";
import { ToggleSwitch } from "../../components/Ui";
import { persistRoutingPolicy } from "../../routingPolicy";
import { useRelayState } from "../../state/RelayStateProvider";

export function ModelProtectionControl() {
  const { mode, runtime } = useRelayState();
  if (!runtime || mode === "zenith") return null;
  if (mode === "remote" && typeof runtime.gateway.basisPointsEnabled !== "boolean") return null;
  if (!runtime.gateway.basisPointsEnabled && !runtime.accounts.some((account) => account.basisPointsAvailable)) return null;
  return <ModelProtectionToggle key={`${mode}:${runtime.runtimeTarget.serverId ?? "local"}`} />;
}

function ModelProtectionToggle() {
  const { t } = useTranslation();
  const { mode, runtime, busy, perform } = useRelayState();
  const id = useId();
  const [pending, setPending] = useState<boolean | null>(null);
  if (!runtime) return null;
  const { gateway } = runtime;

  const saving = pending !== null || busy === "gateway-basis-points";
  const change = async (enabled: boolean) => {
    if (saving || enabled === Boolean(gateway.basisPointsEnabled)) return;
    setPending(enabled);
    try {
      await perform("gateway-basis-points", () => persistRoutingPolicy(mode, {
        maxRetryCandidates: gateway.maxRetryCandidates,
        defaultServiceTier: gateway.defaultServiceTier,
        basisPointsEnabled: enabled,
      }), "feedback.saved");
    } finally {
      setPending(null);
    }
  };

  return <div className="model-protection-control gateway-api-toggle-setting" aria-busy={saving}>
    <div className="relay-toggle-setting">
      <label htmlFor={id} data-relay-tooltip={t("gateway.modelProtectionHint")}>
        <strong>{t("gateway.modelProtection")}</strong>
        <span id={`${id}-description`}>{t("gateway.modelProtectionDescription")}</span>
      </label>
      <ToggleSwitch id={id} label={t("gateway.modelProtection")}
        aria-describedby={`${id}-description`}
        checked={pending ?? gateway.basisPointsEnabled ?? false} disabled={saving}
        onChange={(enabled) => void change(enabled)} />
    </div>
  </div>;
}
