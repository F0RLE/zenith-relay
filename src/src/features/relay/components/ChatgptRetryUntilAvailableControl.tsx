import { RefreshCw } from "lucide-react";
import { useTranslation } from "react-i18next";
import { CodexFeatureToggleControl } from "./CodexFeatureToggleControl";
import { useRelayState } from "../state/RelayStateProvider";

/** Opt-in ChatGPT-only persistence while the provider pool recovers. */
export function ChatgptRetryUntilAvailableControl({ className = "" }: { className?: string }) {
  const { t } = useTranslation();
  const { mode, runtime, busy, chatgptRetryUntilAvailable, setChatgptRetryUntilAvailable } = useRelayState();
  const supported = mode !== "remote" || Boolean(runtime?.capabilities.features.includes("chatgpt_retry_until_available"));
  if (!supported) return null;
  const disabled = !runtime || busy === "chatgpt-retry-until-available";
  return <CodexFeatureToggleControl
    className={className}
    styleClassPrefix="chatgpt-retry-until-available"
    icon={RefreshCw}
    title={t("codex.retryUntilAvailableTitle")}
    hint={t("codex.retryUntilAvailableHint")}
    label={t("codex.retryUntilAvailable")}
    description={chatgptRetryUntilAvailable ? t("codex.retryUntilAvailableEnabled") : t("codex.retryUntilAvailableDisabled")}
    checked={chatgptRetryUntilAvailable}
    disabled={disabled}
    onChange={(enabled) => void setChatgptRetryUntilAvailable(enabled)}
  />;
}
