import { Bot } from "lucide-react";
import { useTranslation } from "react-i18next";
import { SettingToggle } from "./Ui";
import { useRelayState } from "../state/RelayStateProvider";

/** Shared policy control for Codex-owned activity summaries and task titles. */
export function CodexBackgroundTasksControl({ className = "" }: { className?: string }) {
  const { t } = useTranslation();
  const { mode, runtime, busy, codexBackgroundTasksEnabled, setCodexBackgroundTasksEnabled } = useRelayState();
  const supported = mode !== "remote" || Boolean(runtime?.capabilities.features.includes("codex_background_tasks"));
  if (!supported) return null;
  const disabled = !runtime || busy === "codex-background-tasks";
  return <section className={`codex-background-tasks-control${className ? ` ${className}` : ""}`}>
    <div className="codex-background-tasks-heading">
      <span className="codex-background-tasks-icon"><Bot aria-hidden /></span>
      <div>
        <h2>{t("codex.backgroundTasksTitle")}</h2>
        <p>{t("codex.backgroundTasksHint")}</p>
      </div>
    </div>
    <SettingToggle
      className="codex-background-tasks-toggle"
      label={t("codex.backgroundTasks")}
      description={codexBackgroundTasksEnabled ? t("codex.backgroundTasksEnabled") : t("codex.backgroundTasksDisabled")}
      checked={codexBackgroundTasksEnabled}
      disabled={disabled}
      onChange={(enabled) => void setCodexBackgroundTasksEnabled(enabled)}
    />
  </section>;
}
