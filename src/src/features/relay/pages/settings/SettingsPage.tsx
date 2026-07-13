import { useState } from "react";
import { Database, FolderOpen, History, Palette, RefreshCw, RotateCcw, Settings2, ShieldCheck, Trash2 } from "lucide-react";
import { useTranslation } from "react-i18next";
import { restartApplication, updateAndRelaunch } from "../../../../tauri";
import { relayCommands } from "../../api/commands";
import { Button, PageHeader, StatusBadge } from "../../components/Ui";
import { useRelayState } from "../../state/RelayStateProvider";

const sections = [
  { id: "general", icon: Settings2 },
  { id: "appearance", icon: Palette },
  { id: "storage", icon: Database },
  { id: "updates", icon: RefreshCw },
  { id: "security", icon: ShieldCheck },
  { id: "recovery", icon: History },
] as const;

export function SettingsPage() {
  const { t, i18n } = useTranslation(); const { theme, setTheme, compact, setCompact, mode, resetOnboarding, perform, busy } = useRelayState(); const [section, setSection] = useState<(typeof sections)[number]["id"]>("general");
  const restore = () => { if (window.confirm(t("profiles.restoreConfirm"))) void perform("recovery-restore", relayCommands.restoreCodex, "feedback.restored"); };
  const reset = () => { if (window.confirm(t("settings.resetDataConfirm"))) void perform("recovery-reset", async () => { await relayCommands.resetLocalData(); resetOnboarding(); await restartApplication(); }, "feedback.reset"); };
  return <section className="relay-page"><PageHeader title={t("nav.settings")} subtitle={t("settings.subtitle")} /><div className="settings-layout"><nav aria-label={t("settings.sections")}>{sections.map(({ id, icon: Icon }) => <button key={id} className={section === id ? "active" : ""} aria-current={section === id ? "page" : undefined} type="button" onClick={() => setSection(id)}><Icon aria-hidden /><span>{t(`settings.${id}`)}</span></button>)}</nav><div className="settings-content">
    {section === "general" ? <><h2>{t("settings.general")}</h2><label className="relay-field"><span>{t("settings.language")}</span><select value={i18n.language.startsWith("ru")?"ru":"en"} onChange={(event) => i18n.changeLanguage(event.target.value)}><option value="ru">Русский</option><option value="en">English</option></select></label><label className="relay-field"><span>{t("settings.defaultMode")}</span><select value={mode} disabled><option>{t(`modes.${mode}`)}</option></select></label><Button variant="secondary" icon={<RotateCcw aria-hidden />} onClick={resetOnboarding}>{t("settings.restartSetup")}</Button></> : null}
    {section === "appearance" ? <><h2>{t("settings.appearance")}</h2><fieldset><legend>{t("settings.theme")}</legend><div className="segmented">{(["system","light","dark"] as const).map((value) => <button key={value} type="button" className={theme === value ? "active" : ""} onClick={() => setTheme(value)}>{t(`settings.themes.${value}`)}</button>)}</div></fieldset><label className="toggle-row"><input type="checkbox" checked={compact} onChange={(event) => setCompact(event.target.checked)} /><span>{t("settings.compact")}</span></label></> : null}
    {section === "storage" ? <><h2>{t("settings.storage")}</h2><dl className="detail-list"><div><dt>{t("settings.dataPath")}</dt><dd><code>{t("settings.platformDataPath")}</code></dd></div><div><dt>{t("settings.retention")}</dt><dd>30 {t("settings.days")}</dd></div></dl><Button variant="secondary" icon={<FolderOpen aria-hidden />} busy={busy === "open-data"} onClick={() => perform("open-data", () => relayCommands.openFolder("data"), "feedback.opened")}>{t("settings.openData")}</Button></> : null}
    {section === "updates" ? <><h2>{t("settings.updates")}</h2><div className="settings-status"><StatusBadge status="ready" label={t("settings.currentVersion")} /><code>1.0.5</code></div><Button variant="secondary" icon={<RefreshCw aria-hidden />} busy={busy === "update-check"} onClick={() => perform("update-check", updateAndRelaunch, "feedback.upToDate")}>{t("settings.checkUpdates")}</Button></> : null}
    {section === "security" ? <><h2>{t("settings.security")}</h2><div className="settings-status"><ShieldCheck aria-hidden /><div><strong>{t("settings.secretStore")}</strong><span>{t("settings.secretStoreHint")}</span></div><StatusBadge status="ready" label={t("common.available")} /></div><p className="warning-box">{t("settings.insecureWarning")}</p></> : null}
    {section === "recovery" ? <><h2>{t("settings.recovery")}</h2><p>{t("settings.recoveryHint")}</p><Button variant="secondary" icon={<RotateCcw aria-hidden />} busy={busy === "recovery-restore"} onClick={restore}>{t("settings.restoreBackup")}</Button><div className="danger-zone"><h3>{t("settings.resetData")}</h3><p>{t("settings.resetDataHint")}</p><Button variant="danger" icon={<Trash2 aria-hidden />} busy={busy === "recovery-reset"} onClick={reset}>{t("settings.resetData")}</Button></div></> : null}
  </div></div></section>;
}
