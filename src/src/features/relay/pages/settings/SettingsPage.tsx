import { useEffect, useState, type ReactNode } from "react";
import { Database, FolderOpen, Palette, RefreshCw, Trash2 } from "lucide-react";
import { useTranslation } from "react-i18next";
import { APP_VERSION, restartApplication } from "../../../../platform/desktop";
import { relayCommands } from "../../api/commands";
import type { RelayStorageInfo } from "../../api/types";
import { Button, OptionMenu, PageHeader, SettingToggle, StatusBadge, useConfirm } from "../../components/Ui";
import { useRelayState } from "../../state/RelayStateProvider";

type SettingsUpdateState = "idle" | "checking" | "current" | "available" | "error" | "skipped";

export function SettingsPage({ updateCheckState, updateVersion, onCheckUpdates }: { updateCheckState: SettingsUpdateState; updateVersion: string | null; onCheckUpdates: () => Promise<SettingsUpdateState> }) {
  const { t, i18n } = useTranslation();
  const { mode, theme, setTheme, profileSwitchBackupPrompt, setProfileSwitchBackupPrompt, profileSnapshotBackupBeforeRestore, setProfileSnapshotBackupBeforeRestore, resetOnboarding, perform, busy } = useRelayState();
  const confirm = useConfirm();
  const [storageInfo, setStorageInfo] = useState<RelayStorageInfo | null>(null);
  const [storageUnavailable, setStorageUnavailable] = useState(false);
  useEffect(() => {
    let active = true;
    void relayCommands.storageInfo()
      .then((info) => { if (active) setStorageInfo(info); })
      .catch(() => { if (active) setStorageUnavailable(true); });
    return () => { active = false; };
  }, []);
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
        <div className="settings-control-row"><div><strong>{t("settings.language")}</strong></div><OptionMenu className="field-option-menu" label={t("settings.language")} value={i18n.language.startsWith("ru") ? "ru" : "en"} onChange={(value) => void i18n.changeLanguage(value)} options={[{ value: "ru", label: "Русский" }, { value: "en", label: "English" }]} /></div>
        <div className="settings-control-row"><div><strong>{t("settings.theme")}</strong></div><div className="segmented settings-theme-control" role="group" aria-label={t("settings.theme")}>{(["system", "light", "dark"] as const).map((value) => <button key={value} type="button" className={theme === value ? "active" : ""} aria-pressed={theme === value} onClick={() => setTheme(value)}>{t(`settings.themes.${value}`)}</button>)}</div></div>
      </SettingsGroup>

      <SettingsGroup icon={<RefreshCw aria-hidden />} title={t("settings.application")}>
        <div className="settings-control-row"><div><strong>{t("settings.currentVersion")}</strong><div className="settings-version-meta" role="status" aria-live="polite"><span>v{APP_VERSION}</span><StatusBadge status={updateStatus.status} label={updateStatus.label} /></div></div><Button variant="secondary" icon={<RefreshCw aria-hidden />} busy={updateCheckState === "checking"} onClick={() => void onCheckUpdates()}>{t("common.check")}</Button></div>
        <div className="settings-control-row settings-path-row"><div><strong>{t("settings.dataPath")}</strong><small><code title={storageInfo?.dataPath}>{storageInfo?.dataPath ?? t(storageUnavailable ? "settings.pathUnavailable" : "settings.pathLoading")}</code></small></div><Button variant="secondary" icon={<FolderOpen aria-hidden />} busy={busy === "open-data"} onClick={() => perform("open-data", () => relayCommands.openFolder("data"), "feedback.opened")}>{t("settings.openData")}</Button></div>
      </SettingsGroup>

      {mode === "local" ? <SettingsGroup icon={<Database aria-hidden />} title={t("settings.localData")}>
        <SettingToggle className="settings-profile-backup-toggle" label={t("settings.profileSwitchBackupPrompt")} description={t("settings.profileSwitchBackupPromptHint")} checked={profileSwitchBackupPrompt} onChange={setProfileSwitchBackupPrompt} />
        <SettingToggle className="settings-profile-backup-toggle" label={t("settings.profileSnapshotBackupBeforeRestore")} description={t("settings.profileSnapshotBackupBeforeRestoreHint")} checked={profileSnapshotBackupBeforeRestore} onChange={setProfileSnapshotBackupBeforeRestore} />
        <div className="settings-control-row settings-danger-row"><div><strong>{t("settings.resetData")}</strong><small>{t("settings.resetDataHint")}</small></div><Button variant="danger" icon={<Trash2 aria-hidden />} busy={busy === "recovery-reset"} onClick={reset}>{t("common.reset")}</Button></div>
      </SettingsGroup> : null}
    </div>
  </section>;
}

function SettingsGroup({ icon, title, children }: { icon: ReactNode; title: string; children: ReactNode }) {
  return <section className="settings-group"><header>{icon}<h2>{title}</h2></header><div className="settings-group-body">{children}</div></section>;
}
