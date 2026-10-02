import { Bot } from "lucide-react";
import { useTranslation } from "react-i18next";
import { CodexFeatureToggleControl } from "./CodexFeatureToggleControl";
import { useRelayState } from "../state/RelayStateProvider";
import { usePendingFlag } from "../state/usePendingFlag";

/** Shared policy control for Codex-owned activity summaries and task titles. */
export function CodexBackgroundTasksControl({ className = "" }: { className?: string }) {
  const { t } = useTranslation();
  const { mode, runtime, codexBackgroundTasksEnabled, setCodexBackgroundTasksEnabled } = useRelayState();
  const { checked, select } = usePendingFlag(codexBackgroundTasksEnabled);
  const supported = mode !== "remote" || Boolean(runtime?.capabilities.features.includes("codex_background_tasks"));
  if (!supported) return null;
  return <CodexFeatureToggleControl
    className={className}
    styleClassPrefix="codex-background-tasks"
    icon={Bot}
    title={t("codex.backgroundTasksTitle")}
    hint={t("codex.backgroundTasksHint")}
    label={t("codex.backgroundTasks")}
    description={checked ? t("codex.backgroundTasksEnabled") : t("codex.backgroundTasksDisabled")}
    checked={checked}
    disabled={!runtime}
    onChange={(enabled) => select(enabled, () => setCodexBackgroundTasksEnabled(enabled))}
  />;
}
