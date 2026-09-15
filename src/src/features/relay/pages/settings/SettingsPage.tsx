import { useEffect, useState, type ReactNode } from "react";
import { Bug, Database, FileText, FileWarning, FolderOpen, Palette, RefreshCw, Trash2 } from "lucide-react";
import { useTranslation } from "react-i18next";
import { setI18nLanguage } from "../../../../i18n";
import { APP_VERSION, restartApplication } from "../../../../platform/desktop";
import { relayCommands } from "../../api/commands";
import type { DiagnosticSettings, RelayStorageInfo } from "../../api/types";
import { Button, OptionMenu, PageHeader, SettingToggle, StatusBadge, useConfirm } from "../../components/Ui";
import { useRelayState } from "../../state/RelayStateProvider";

type SettingsUpdateState = "idle" | "checking" | "current" | "available" | "error" | "skipped";

export function SettingsPage({ updateCheckState, updateVersion, onCheckUpdates }: { updateCheckState: SettingsUpdateState; updateVersion: string | null; onCheckUpdates: () => Promise<SettingsUpdateState> }) {
  const { t, i18n } = useTranslation();
  const { mode, theme, setTheme, profileSwitchBackupPrompt, setProfileSwitchBackupPrompt, resetOnboarding, perform, busy } = useRelayState();
  const confirm = useConfirm();
  const [storageInfo, setStorageInfo] = useState<RelayStorageInfo | null>(null);
  const [storageUnavailable, setStorageUnavailable] = useState(false);
  const [diagnosticSettings, setDiagnosticSettings] = useState<DiagnosticSettings | null>(null);
  const [diagnosticsUnavailable, setDiagnosticsUnavailable] = useState(false);
  useEffect(() => {
    let active = true;
    void relayCommands.storageInfo()
      .then((info) => { if (active) setStorageInfo(info); })
      .catch(() => { if (active) setStorageUnavailable(true); });
    return () => { active = false; };
  }, []);
  useEffect(() => {
    let active = true;
    void relayCommands.diagnosticSettings()
      .then((settings) => { if (active) setDiagnosticSettings(settings); })
      .catch(() => { if (active) setDiagnosticsUnavailable(true); });
    return () => { active = false; };
  }, []);
  const updateDiagnosticDebug = async (enabled: boolean) => {
    const settings = await relayCommands.setDiagnosticDebugMode(enabled);
    setDiagnosticSettings(settings);
    return settings;
  };
  const reset = async () => { if (await confirm(t("settings.resetDataConfirm"), { danger: true })) await perform("recovery-reset", async () => { await relayCommands.resetLocalData(); resetOnboarding(); await restartApplication(); }, "feedback.reset"); };
  const updateStatus = updateCheckState === "available" ? { status: "info" as const, label: t("updates.availableVersion", { version: updateVersion }) }
    : updateCheckState === "error" ? { status: "error" as const, label: t("update.failed") }
      : updateCheckState === "skipped" ? { status: "warning" as const, label: t("updates.skipped") }
        : updateCheckState === "checking" ? { status: "info" as const, label: t("update.checking") }
          : updateCheckState === "idle" ? { status: "disabled" as const, label: t("updates.notChecked") }
            : { status: "ready" as const, label: t("update.upToDate") };

  return <section className="relay-page settings-page">
    <PageHeader title={t("nav.settings")} subtitle={t("settings.subtitle")} />
    <div className="settings-groups">
      <SettingsGroup icon={<Palette aria-hidden />} title={t("settings.appearance")}>
        <div className="settings-control-row"><div><strong>{t("settings.language")}</strong></div><OptionMenu className="field-option-menu" label={t("settings.language")} value={i18n.language.startsWith("ru") ? "ru" : "en"} onChange={(value) => void setI18nLanguage(value)} options={[{ value: "ru", label: "Русский" }, { value: "en", label: "English" }]} /></div>
        <div className="settings-control-row"><div><strong>{t("settings.theme")}</strong></div><div className="segmented settings-theme-control" role="group" aria-label={t("settings.theme")}>{(["system", "light", "dark"] as const).map((value) => <button key={value} type="button" className={theme === value ? "active" : ""} aria-pressed={theme === value} onClick={() => setTheme(value)}>{t(`settings.themes.${value}`)}</button>)}</div></div>
      </SettingsGroup>

      <SettingsGroup icon={<RefreshCw aria-hidden />} title={t("settings.application")}>
        <div className="settings-control-row"><div><strong>{t("settings.currentVersion")}</strong><div className="settings-version-meta" role="status" aria-live="polite"><span>v{APP_VERSION}</span><StatusBadge status={updateStatus.status} label={updateStatus.label} /></div></div><Button variant="secondary" icon={<RefreshCw aria-hidden />} busy={updateCheckState === "checking"} onClick={() => void onCheckUpdates()}>{t("common.check")}</Button></div>
        <div className="settings-control-row settings-path-row"><div><strong>{t("settings.dataPath")}</strong><small><code data-relay-tooltip={storageInfo?.dataPath}>{storageInfo?.dataPath ?? t(storageUnavailable ? "settings.pathUnavailable" : "settings.pathLoading")}</code></small></div><Button variant="secondary" icon={<FolderOpen aria-hidden />} busy={busy === "open-data"} onClick={() => perform("open-data", () => relayCommands.openFolder("data"), "feedback.opened")}>{t("settings.openData")}</Button></div>
      </SettingsGroup>

      {mode === "local" ? <SettingsGroup icon={<Database aria-hidden />} title={t("settings.localData")}>
        <SettingToggle className="settings-profile-backup-toggle" label={t("settings.profileSwitchBackupPrompt")} description={t("settings.profileSwitchBackupPromptHint")} checked={profileSwitchBackupPrompt} onChange={setProfileSwitchBackupPrompt} />
        <div className="settings-debug-section">
          <SettingToggle
            className="settings-debug-toggle"
            label={t("settings.debugMode")}
            description={diagnosticSettings
              ? t("settings.debugModeHint", { state: t(diagnosticSettings.debugEnabled ? "settings.debugModeOn" : "settings.debugModeOff") })
              : t(diagnosticsUnavailable ? "settings.debugModeUnavailable" : "settings.debugModeLoading")}
            checked={diagnosticSettings?.debugEnabled ?? false}
            disabled={diagnosticSettings === null || busy === "diagnostics-debug"}
            onChange={(enabled) => void perform("diagnostics-debug", () => updateDiagnosticDebug(enabled), "feedback.saved")}
          />
          {diagnosticSettings?.debugEnabled ? <div className="settings-debug-details" role="region" aria-label={t("settings.debugDetails")}>
            <div className="settings-debug-details-copy"><strong>{t("settings.debugDetailsTitle")}</strong><small>{t("settings.debugDetailsHint")}</small></div>
            <div className="settings-debug-details-action"><code data-relay-tooltip={storageInfo?.operationLogsPath}>{storageInfo?.operationLogsPath ?? t(storageUnavailable ? "settings.pathUnavailable" : "settings.pathLoading")}</code><Button variant="secondary" icon={<FileText aria-hidden />} busy={busy === "open-operation-logs"} onClick={() => perform("open-operation-logs", () => relayCommands.openFolder("operation_logs"), "feedback.opened")}>{t("settings.openOperations")}</Button></div>
          </div> : null}
        </div>
        <div className="settings-control-row settings-danger-row"><div><strong>{t("settings.resetData")}</strong><small>{t("settings.resetDataHint")}</small></div><Button variant="danger" icon={<Trash2 aria-hidden />} busy={busy === "recovery-reset"} onClick={reset}>{t("common.reset")}</Button></div>
      </SettingsGroup> : null}

      {mode === "local" && diagnosticSettings?.debugEnabled ? <SettingsGroup icon={<Bug aria-hidden />} title={t("settings.diagnostics")}>
        <div className="settings-control-row settings-path-row"><div><strong>{t("settings.logsPath")}</strong><small>{t("settings.logsHint")}<code data-relay-tooltip={storageInfo?.logsPath}>{storageInfo?.logsPath ?? t(storageUnavailable ? "settings.pathUnavailable" : "settings.pathLoading")}</code></small></div><Button variant="secondary" icon={<FolderOpen aria-hidden />} busy={busy === "open-logs"} onClick={() => perform("open-logs", () => relayCommands.openFolder("logs"), "feedback.opened")}>{t("settings.openLogs")}</Button></div>
        <div className="settings-control-row settings-path-row"><div><strong>{t("settings.errorLogs")}</strong><small><code data-relay-tooltip={storageInfo?.errorLogsPath}>{storageInfo?.errorLogsPath ?? t(storageUnavailable ? "settings.pathUnavailable" : "settings.pathLoading")}</code></small></div><Button variant="secondary" icon={<FileWarning aria-hidden />} busy={busy === "open-error-logs"} onClick={() => perform("open-error-logs", () => relayCommands.openFolder("error_logs"), "feedback.opened")}>{t("settings.openErrors")}</Button></div>
        <div className="settings-control-row settings-path-row"><div><strong>{t("settings.crashLogs")}</strong><small><code data-relay-tooltip={storageInfo?.crashLogsPath}>{storageInfo?.crashLogsPath ?? t(storageUnavailable ? "settings.pathUnavailable" : "settings.pathLoading")}</code></small></div><Button variant="secondary" icon={<Bug aria-hidden />} busy={busy === "open-crash-logs"} onClick={() => perform("open-crash-logs", () => relayCommands.openFolder("crash_logs"), "feedback.opened")}>{t("settings.openCrashes")}</Button></div>
        <div className="settings-control-row settings-path-row"><div><strong>{t("settings.operationLogs")}</strong><small><code data-relay-tooltip={storageInfo?.operationLogsPath}>{storageInfo?.operationLogsPath ?? t(storageUnavailable ? "settings.pathUnavailable" : "settings.pathLoading")}</code></small></div><Button variant="secondary" icon={<FileText aria-hidden />} busy={busy === "open-operation-logs"} onClick={() => perform("open-operation-logs", () => relayCommands.openFolder("operation_logs"), "feedback.opened")}>{t("settings.openOperations")}</Button></div>
      </SettingsGroup> : null}
    </div>
  </section>;
}

function SettingsGroup({ icon, title, children }: { icon: ReactNode; title: string; children: ReactNode }) {
  return <section className="settings-group"><header>{icon}<h2>{title}</h2></header><div className="settings-group-body">{children}</div></section>;
}
