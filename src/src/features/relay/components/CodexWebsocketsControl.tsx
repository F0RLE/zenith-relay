import { Cable } from "lucide-react";
import { useTranslation } from "react-i18next";
import { CodexFeatureToggleControl } from "./CodexFeatureToggleControl";
import { useRelayState } from "../state/RelayStateProvider";

/** Keeps the Codex transport preference and Relay WebSocket fallback in sync. */
export function CodexWebsocketsControl({ className = "" }: { className?: string }) {
  const { t } = useTranslation();
  const { mode, runtime, busy, codexWebsocketsEnabled, setCodexWebsocketsEnabled } = useRelayState();
  const supported = mode !== "remote" || Boolean(runtime?.capabilities.features.includes("codex_websockets"));
  if (!supported) return null;
  const disabled = !runtime || busy === "codex-websockets";
  return <CodexFeatureToggleControl
    className={className}
    styleClassPrefix="codex-websockets"
    icon={Cable}
    title={t("codex.websocketsTitle")}
    hint={t("codex.websocketsHint")}
    label={t("codex.websockets")}
    description={codexWebsocketsEnabled ? t("codex.websocketsEnabled") : t("codex.websocketsDisabled")}
    checked={codexWebsocketsEnabled}
    disabled={disabled}
    onChange={(enabled) => void setCodexWebsocketsEnabled(enabled)}
  />;
}
