import { useEffect, useId, useState } from "react";
import { useTranslation } from "react-i18next";
import { relayCommands } from "../../api/commands";
import type { ToolPolicy } from "../../api/types";
import { ToggleSwitch } from "../../components/Ui";
import { useRelayState } from "../../state/RelayStateProvider";

const defaultPolicy: ToolPolicy = {
  mode: "pass_through",
};

export function ToolPolicyControl() {
  const { mode, runtime } = useRelayState();
  if (!runtime || mode === "zenith" || (mode === "remote" && !runtime.capabilities.features.includes("tool_policy_v1"))) return null;
  return <ToolPolicyEditor key={`${mode}:${runtime.runtimeTarget.serverId ?? "local"}`} saved={runtime.gateway.toolPolicy ?? defaultPolicy} />;
}

function ToolPolicyEditor({ saved }: { saved: ToolPolicy }) {
  const { t } = useTranslation();
  const { mode, busy, perform } = useRelayState();
  const id = useId();
  const [policyMode, setPolicyMode] = useState(saved.mode);
  const saving = busy === "tool-policy";
  useEffect(() => {
    setPolicyMode(saved.mode);
  }, [saved.mode]);

  const changeMode = async (enabled: boolean) => {
    if (saving) return;
    const policy: ToolPolicy = { mode: enabled ? "automatic" : "pass_through" };
    if (policy.mode === saved.mode) return;
    setPolicyMode(policy.mode);
    const input = { policy, expectedPolicy: { mode: saved.mode } };
    const success = await perform("tool-policy", () => mode === "local"
      ? relayCommands.setToolPolicy(input)
      : relayCommands.setRemoteToolPolicy(input), "feedback.saved");
    if (!success) setPolicyMode(saved.mode);
  };
  return <div className="gateway-tool-policy gateway-api-toggle-setting" aria-busy={saving}>
    <div className="relay-toggle-setting">
      <label htmlFor={id} data-relay-tooltip={t("toolPolicy.hint")}>
        <strong>{t("toolPolicy.title")}</strong>
        <span id={`${id}-description`}>{t("toolPolicy.description")}</span>
      </label>
      <ToggleSwitch id={id} label={t("toolPolicy.optimize")} aria-describedby={`${id}-description`} checked={policyMode === "automatic"} disabled={saving}
        onChange={(checked) => void changeMode(checked)} />
    </div>
  </div>;
}
