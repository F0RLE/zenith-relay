import { Cable } from "lucide-react";
import { useTranslation } from "react-i18next";
import { SettingToggle } from "./Ui";
import { useRelayState } from "../state/RelayStateProvider";

/** Keeps the Codex transport preference and Relay WebSocket fallback in sync. */
export function CodexWebsocketsControl({ className = "" }: { className?: string }) {
  const { t } = useTranslation();
  const { mode, runtime, busy, codexWebsocketsEnabled, setCodexWebsocketsEnabled } = useRelayState();
  const supported = mode !== "remote" || Boolean(runtime?.capabilities.features.includes("codex_websockets"));
  if (!supported) return null;
  const disabled = !runtime || busy === "codex-websockets";
  return (
    <section className={`codex-websockets-control${className ? ` ${className}` : ""}`}>
      <div className="codex-websockets-heading">
        <span className="codex-websockets-icon"><Cable aria-hidden /></span>
        <div>
          <h2>{t("codex.websocketsTitle")}</h2>
          <p>{t("codex.websocketsHint")}</p>
        </div>
      </div>
      <SettingToggle
        className="codex-websockets-toggle"
        label={t("codex.websockets")}
        description={codexWebsocketsEnabled ? t("codex.websocketsEnabled") : t("codex.websocketsDisabled")}
        checked={codexWebsocketsEnabled}
        disabled={disabled}
        onChange={(enabled) => void setCodexWebsocketsEnabled(enabled)}
      />
    </section>
  );
}
