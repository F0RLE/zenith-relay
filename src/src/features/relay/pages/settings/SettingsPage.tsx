import { useEffect, useState, type ReactNode } from "react";
import { Database, FolderOpen, History, Palette, RefreshCw, RotateCcw, Settings2, ShieldCheck, Trash2 } from "lucide-react";
import { useTranslation } from "react-i18next";
import { APP_VERSION, restartApplication } from "../../../../tauri";
import { relayCommands } from "../../api/commands";
import type { RelayStorageInfo } from "../../api/types";
import { Button, OptionMenu, PageHeader, StatusBadge } from "../../components/Ui";
import { useRelayState } from "../../state/RelayStateProvider";

type SettingsUpdateState = "idle" | "checking" | "current" | "available" | "error" | "skipped";

export function SettingsPage({ updateCheckState, updateVersion, onCheckUpdates }: { updateCheckState: SettingsUpdateState; updateVersion: string | null; onCheckUpdates: () => Promise<SettingsUpdateState> }) {
  const { t, i18n } = useTranslation();
  const { theme, setTheme, compact, setCompact, snapshotBeforeSwitch, setSnapshotBeforeSwitch, resetOnboarding, perform, busy } = useRelayState();
  const [storageInfo, setStorageInfo] = useState<RelayStorageInfo | null>(null);
  const [storageUnavailable, setStorageUnavailable] = useState(false);
  useEffect(() => {
    let active = true;
    void relayCommands.storageInfo()
      .then((info) => { if (active) setStorageInfo(info); })
      .catch(() => { if (active) setStorageUnavailable(true); });
    return () => { active = false; };
  }, []);
  const restore = () => { if (window.confirm(t("profiles.restoreConfirm"))) void perform("recovery-restore", relayCommands.restoreCodex, "feedback.restored"); };
  const reset = () => { if (window.confirm(t("settings.resetDataConfirm"))) void perform("recovery-reset", async () => { await relayCommands.resetLocalData(); resetOnboarding(); await restartApplication(); }, "feedback.reset"); };
  const updateStatus = updateCheckState === "available" ? { status: "info" as const, label: t("updates.availableVersion", { version: updateVersion }) }
    : updateCheckState === "error" ? { status: "error" as const, label: t("updates.checkFailed") }
      : updateCheckState === "skipped" ? { status: "warning" as const, label: t("updates.skipped") }
        : { status: "ready" as const, label: t("updates.current") };

  return <section className="relay-page settings-page">
    <PageHeader title={t("nav.settings")} subtitle={t("settings.subtitle")} />
    <div className="settings-groups">
      <SettingsGroup icon={<Settings2 aria-hidden />} title={t("settings.general")}>
        <div className="settings-control-row"><div><strong>{t("settings.language")}</strong></div><OptionMenu className="field-option-menu" label={t("settings.language")} value={i18n.language.startsWith("ru") ? "ru" : "en"} onChange={(value) => void i18n.changeLanguage(value)} options={[{ value: "ru", label: "Русский" }, { value: "en", label: "English" }]} /></div>
        <label className="settings-control-row settings-switch-row"><span><strong>{t("settings.snapshotBeforeSwitch")}</strong><small>{t("settings.snapshotBeforeSwitchHint")}</small></span><input type="checkbox" checked={snapshotBeforeSwitch} onChange={(event) => setSnapshotBeforeSwitch(event.target.checked)} /></label>
        <div className="settings-control-row"><div><strong>{t("settings.restartSetup")}</strong></div><Button variant="secondary" icon={<RotateCcw aria-hidden />} onClick={resetOnboarding}>{t("common.restart")}</Button></div>
      </SettingsGroup>

      <SettingsGroup icon={<Palette aria-hidden />} title={t("settings.appearance")}>
        <div className="settings-control-row"><div><strong>{t("settings.theme")}</strong></div><div className="segmented settings-theme-control">{(["system", "light", "dark"] as const).map((value) => <button key={value} type="button" className={theme === value ? "active" : ""} onClick={() => setTheme(value)}>{t(`settings.themes.${value}`)}</button>)}</div></div>
        <label className="settings-control-row settings-switch-row"><span><strong>{t("settings.compact")}</strong></span><input type="checkbox" checked={compact} onChange={(event) => setCompact(event.target.checked)} /></label>
      </SettingsGroup>

      <SettingsGroup icon={<Database aria-hidden />} title={t("settings.storage")}>
        <div className="settings-control-row"><div><strong>{t("settings.dataPath")}</strong><small><code title={storageInfo?.dataPath}>{storageInfo?.dataPath ?? t(storageUnavailable ? "settings.pathUnavailable" : "settings.pathLoading")}</code></small></div><Button variant="secondary" icon={<FolderOpen aria-hidden />} busy={busy === "open-data"} onClick={() => perform("open-data", () => relayCommands.openFolder("data"), "feedback.opened")}>{t("settings.openData")}</Button></div>
        <div className="settings-control-row settings-storage-locations"><div><strong>{t("settings.storageLocations")}</strong><dl className="settings-path-grid">
          <StoragePath label={t("settings.backupsPath")} path={storageInfo?.backupsPath} />
          <StoragePath label={t("settings.exportsPath")} path={storageInfo?.exportsPath} />
          <StoragePath label={t("settings.cachePath")} path={storageInfo?.cachePath} />
          <StoragePath label={t("settings.chatgptProfilePath")} path={storageInfo?.chatgptProfilePath} />
        </dl></div></div>
        <div className="settings-control-row"><div><strong>{t("settings.retention")}</strong></div><span className="settings-value">30 {t("settings.days")}</span></div>
        {storageInfo?.legacyDataPath && <p className="settings-note settings-warning-note"><strong>{t("settings.legacyDataFound")}</strong><code>{storageInfo.legacyDataPath}</code><span>{t("settings.legacyDataHint")}</span></p>}
      </SettingsGroup>

      <SettingsGroup icon={<ShieldCheck aria-hidden />} title={t("settings.security")}>
        <div className="settings-control-row"><div><strong>{t("settings.secretStore")}</strong><small>{t("settings.secretStoreHint")}</small></div><StatusBadge status="ready" label={t("common.available")} /></div>
        <p className="settings-note">{t("settings.insecureWarning")}</p>
      </SettingsGroup>

      <SettingsGroup icon={<RefreshCw aria-hidden />} title={t("settings.updates")}>
        <div className="settings-control-row"><div><strong>{t("settings.currentVersion")}</strong><small>v{APP_VERSION}</small></div><StatusBadge status={updateStatus.status} label={updateStatus.label} /></div>
        <div className="settings-control-row"><div><strong>{t("settings.checkUpdates")}</strong></div><Button variant="secondary" icon={<RefreshCw aria-hidden />} busy={updateCheckState === "checking"} onClick={() => void onCheckUpdates()}>{t("common.check")}</Button></div>
      </SettingsGroup>

      <SettingsGroup icon={<History aria-hidden />} title={t("settings.recovery")}>
        <div className="settings-control-row"><div><strong>{t("settings.restoreBackup")}</strong><small>{t("settings.recoveryHint")}</small></div><Button variant="secondary" icon={<RotateCcw aria-hidden />} busy={busy === "recovery-restore"} onClick={restore}>{t("common.restore")}</Button></div>
        <div className="settings-control-row settings-danger-row"><div><strong>{t("settings.resetData")}</strong><small>{t("settings.resetDataHint")}</small></div><Button variant="danger" icon={<Trash2 aria-hidden />} busy={busy === "recovery-reset"} onClick={reset}>{t("common.reset")}</Button></div>
      </SettingsGroup>
    </div>
  </section>;
}

function StoragePath({ label, path }: { label: string; path: string | null | undefined }) {
  const { t } = useTranslation();
  return <div><dt>{label}</dt><dd><code title={path ?? undefined}>{path ?? t("settings.pathLoading")}</code></dd></div>;
}

function SettingsGroup({ icon, title, children }: { icon: ReactNode; title: string; children: ReactNode }) {
  return <section className="settings-group"><header>{icon}<h2>{title}</h2></header><div className="settings-group-body">{children}</div></section>;
}
