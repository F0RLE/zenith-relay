import { Cable } from "lucide-react";
import { useTranslation } from "react-i18next";
import { CodexFeatureToggleControl } from "./CodexFeatureToggleControl";
import { useConfirm } from "./Ui";
import { useRelayState } from "../state/RelayStateProvider";
import { usePendingFlag } from "../state/usePendingFlag";

/** Keeps the Codex transport preference and Relay WebSocket fallback in sync. */
export function CodexWebsocketsControl({ className = "" }: { className?: string }) {
  const { t } = useTranslation();
  const { mode, runtime, codexWebsocketsEnabled, setCodexWebsocketsEnabled } = useRelayState();
  const confirm = useConfirm();
  const { checked, select } = usePendingFlag(codexWebsocketsEnabled);
  const supported = mode !== "remote" || Boolean(runtime?.capabilities.features.includes("codex_websockets"));
  if (!supported) return null;
  return <CodexFeatureToggleControl
    className={className}
    styleClassPrefix="codex-websockets"
    icon={Cable}
    title={t("codex.websocketsTitle")}
    hint={t("codex.websocketsHint")}
    label={t("codex.websockets")}
    description={checked ? t("codex.websocketsEnabled") : t("codex.websocketsDisabled")}
    checked={checked}
    disabled={!runtime}
    onChange={async (enabled) => {
      if (enabled && !await confirm(t("codex.websocketsRestartMessage"), {
        title: t("codex.websocketsRestartTitle"),
        confirmLabel: t("codex.websocketsRestartAction"),
      })) return;
      select(enabled, () => setCodexWebsocketsEnabled(enabled));
    }}
  />;
}
