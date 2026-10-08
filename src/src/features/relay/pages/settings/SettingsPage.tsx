import { useEffect, useState, type ReactNode } from "react";
import { Bug, Database, FileText, FileWarning, FolderOpen, Palette, RefreshCw, Trash2 } from "lucide-react";
import { useTranslation } from "react-i18next";
import { setI18nLanguage } from "../../../../i18n";
import { APP_VERSION, restartApplication } from "../../../../platform/desktop";
import { relayCommands } from "../../api/commands";
import type { DiagnosticSettings, RelayStorageInfo } from "../../api/types";
import { Button, OptionMenu, PageHeader, SettingToggle, StatusBadge, useConfirm } from "../../components/Ui";
import { useRelayState } from "../../state/RelayStateProvider";
import { usePendingFlag } from "../../state/usePendingFlag";

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
      .then((storageInfo) => { if (active) setStorageInfo(storageInfo); })
      .catch(() => { if (active) setStorageUnavailable(true); });
    return () => { active = false; };
  }, []);
  useEffect(() => {
    let active = true;
    void relayCommands.diagnosticSettings()
      .then((diagnosticSettings) => { if (active) setDiagnosticSettings(diagnosticSettings); })
      .catch(() => { if (active) setDiagnosticsUnavailable(true); });
    return () => { active = false; };
  }, []);
  const debugMode = usePendingFlag(diagnosticSettings?.debugEnabled ?? false);
  const updateDiagnosticDebug = async (enabled: boolean) => {
    const updatedDiagnosticSettings = await relayCommands.setDiagnosticDebugMode(enabled);
    setDiagnosticSettings(updatedDiagnosticSettings);
    return updatedDiagnosticSettings;
  };
  const reset = async () => { if (await confirm(t("settings.resetDataConfirm"), { danger: true })) await perform("recovery-reset", async () => { await relayCommands.resetLocalData(); resetOnboarding(); await restartApplication(); }, "feedback.reset"); };
  const updateStatus = updateCheckState === "available" ? { status: "info" as const, label: t("updates.availableVersion", { version: updateVersion }) }
    : updateCheckState === "error" ? { status: "error" as const, label: t("update.failed") }
      : updateCheckState === "skipped" ? { status: "warning" as const, label: t("updates.skipped") }
        : updateCheckState === "checking" ? { status: "info" as const, label: t("update.checking") }
          : updateCheckState === "idle" ? { status: "disabled" as const, label: t("updates.notChecked") }
            : { status: "ready" as const, label: t("update.upToDate") };

  return <section className="relay-page relay-workspace-page settings-page">
    <PageHeader workspace title={t("nav.settings")} />
    <div className="settings-groups">
      <SettingsGroup icon={<Palette aria-hidden />} title={t("settings.appearance")}>
        <div className="settings-control-row">
          <div><strong>{t("settings.language")}</strong></div>
          <OptionMenu
            className="field-option-menu"
            label={t("settings.language")}
            value={i18n.language.startsWith("ru") ? "ru" : "en"}
            onChange={(languageCode) => void setI18nLanguage(languageCode)}
            options={[{ value: "ru", label: "Русский" }, { value: "en", label: "English" }]}
          />
        </div>
        <div className="settings-control-row">
          <div><strong>{t("settings.theme")}</strong></div>
          <div className="segmented settings-theme-control" role="group" aria-label={t("settings.theme")}>
            {(["system", "light", "dark"] as const).map((themeName) => (
              <button key={themeName} type="button" className={theme === themeName ? "active" : ""} aria-pressed={theme === themeName} onClick={() => setTheme(themeName)}>
                {t(`settings.themes.${themeName}`)}
              </button>
            ))}
          </div>
        </div>
      </SettingsGroup>

      <SettingsGroup icon={<RefreshCw aria-hidden />} title={t("settings.application")}>
        <div className="settings-control-row">
          <div>
            <strong>{t("settings.currentVersion")}</strong>
            <div className="settings-version-meta" role="status" aria-live="polite">
              <span>v{APP_VERSION}</span>
              <StatusBadge status={updateStatus.status} label={updateStatus.label} />
            </div>
          </div>
          <Button variant="secondary" icon={<RefreshCw aria-hidden />} busy={updateCheckState === "checking"} onClick={() => void onCheckUpdates()}>{t("common.check")}</Button>
        </div>
        <SettingsPathRow
          title={t("settings.dataPath")}
          path={storageInfo?.dataPath}
          missing={t(storageUnavailable ? "settings.pathUnavailable" : "settings.pathLoading")}
          icon={<FolderOpen aria-hidden />}
          action={t("settings.openData")}
          busy={busy === "open-data"}
          onOpen={() => perform("open-data", () => relayCommands.openFolder("data"), "feedback.opened", { backgroundRefresh: true })}
        />
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
            checked={debugMode.checked}
            disabled={diagnosticSettings === null}
            onChange={(enabled) => debugMode.select(enabled, () => perform("diagnostics-debug", () => updateDiagnosticDebug(enabled), "feedback.saved", { backgroundRefresh: true, uiLock: false }))}
          />
        </div>
        <div className="settings-control-row settings-danger-row">
          <div><strong>{t("settings.resetData")}</strong><small>{t("settings.resetDataHint")}</small></div>
          <Button variant="danger" icon={<Trash2 aria-hidden />} busy={busy === "recovery-reset"} onClick={reset}>{t("common.reset")}</Button>
        </div>
      </SettingsGroup> : null}

      {mode === "local" && diagnosticSettings?.debugEnabled ? <SettingsGroup icon={<Bug aria-hidden />} title={t("settings.diagnostics")}>
        <SettingsPathRow
          title={t("settings.logsPath")}
          hint={t("settings.logsHint")}
          path={storageInfo?.logsPath}
          missing={t(storageUnavailable ? "settings.pathUnavailable" : "settings.pathLoading")}
          icon={<FolderOpen aria-hidden />}
          action={t("settings.openLogs")}
          busy={busy === "open-logs"}
          onOpen={() => perform("open-logs", () => relayCommands.openFolder("logs"), "feedback.opened", { backgroundRefresh: true })}
        />
        <SettingsPathRow
          title={t("settings.errorLogs")}
          path={storageInfo?.errorLogsPath}
          missing={t(storageUnavailable ? "settings.pathUnavailable" : "settings.pathLoading")}
          icon={<FileWarning aria-hidden />}
          action={t("settings.openErrors")}
          busy={busy === "open-error-logs"}
          onOpen={() => perform("open-error-logs", () => relayCommands.openFolder("error_logs"), "feedback.opened", { backgroundRefresh: true })}
        />
        <SettingsPathRow
          title={t("settings.crashLogs")}
          path={storageInfo?.crashLogsPath}
          missing={t(storageUnavailable ? "settings.pathUnavailable" : "settings.pathLoading")}
          icon={<Bug aria-hidden />}
          action={t("settings.openCrashes")}
          busy={busy === "open-crash-logs"}
          onOpen={() => perform("open-crash-logs", () => relayCommands.openFolder("crash_logs"), "feedback.opened", { backgroundRefresh: true })}
        />
        <SettingsPathRow
          title={t("settings.operationLogs")}
          path={storageInfo?.operationLogsPath}
          missing={t(storageUnavailable ? "settings.pathUnavailable" : "settings.pathLoading")}
          icon={<FileText aria-hidden />}
          action={t("settings.openOperations")}
          busy={busy === "open-operation-logs"}
          onOpen={() => perform("open-operation-logs", () => relayCommands.openFolder("operation_logs"), "feedback.opened", { backgroundRefresh: true })}
        />
      </SettingsGroup> : null}
    </div>
  </section>;
}

function SettingsGroup({ icon, title, children }: { icon: ReactNode; title: string; children: ReactNode }) {
  return <section className="settings-group"><header>{icon}<h2>{title}</h2></header><div className="settings-group-body">{children}</div></section>;
}

function SettingsPathRow({ title, hint, path, missing, icon, action, busy, onOpen }: {
  title: string;
  hint?: string;
  path: string | undefined;
  missing: string;
  icon: ReactNode;
  action: string;
  busy: boolean;
  onOpen: () => void | Promise<unknown>;
}) {
  return (
    <div className="settings-control-row settings-path-row">
      <div>
        <strong>{title}</strong>
        <small>{hint}<code data-relay-tooltip={path}>{path ?? missing}</code></small>
      </div>
      <Button variant="secondary" icon={icon} busy={busy} onClick={onOpen}>{action}</Button>
    </div>
  );
}
