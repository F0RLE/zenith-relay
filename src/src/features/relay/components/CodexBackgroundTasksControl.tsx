import { Bot } from "lucide-react";
import { useTranslation } from "react-i18next";
import { CodexFeatureToggleControl } from "./CodexFeatureToggleControl";
import { useRelayState } from "../state/RelayStateProvider";

/** Shared policy control for Codex-owned activity summaries and task titles. */
export function CodexBackgroundTasksControl({ className = "" }: { className?: string }) {
  const { t } = useTranslation();
  const { mode, runtime, busy, codexBackgroundTasksEnabled, setCodexBackgroundTasksEnabled } = useRelayState();
  const supported = mode !== "remote" || Boolean(runtime?.capabilities.features.includes("codex_background_tasks"));
  if (!supported) return null;
  const disabled = !runtime || busy === "codex-background-tasks";
  return <CodexFeatureToggleControl
    className={className}
    styleClassPrefix="codex-background-tasks"
    icon={Bot}
    title={t("codex.backgroundTasksTitle")}
    hint={t("codex.backgroundTasksHint")}
    label={t("codex.backgroundTasks")}
    description={codexBackgroundTasksEnabled ? t("codex.backgroundTasksEnabled") : t("codex.backgroundTasksDisabled")}
    checked={codexBackgroundTasksEnabled}
    disabled={disabled}
    onChange={(enabled) => void setCodexBackgroundTasksEnabled(enabled)}
  />;
}
