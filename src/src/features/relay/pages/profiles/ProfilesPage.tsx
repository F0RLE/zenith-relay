import { useCallback, useEffect, useState } from "react";
import { FolderOpen, Play, Plus, RotateCcw, Wrench } from "lucide-react";
import { useTranslation } from "react-i18next";
import { relayCommands } from "../../api/commands";
import type { HistoryRepairPreview, HistoryRepairResult, OpenCodeProfileState, ProfileBinding } from "../../api/types";
import { Button, EmptyState, PageHeader, StatusBadge, Tabs } from "../../components/Ui";
import { useRelayState } from "../../state/RelayStateProvider";

export function ProfilesPage() {
  const { t } = useTranslation();
  const { mode, runtime, busy, perform } = useRelayState();
  const [view, setView] = useState("profiles");
  const [bindings, setBindings] = useState<ProfileBinding[]>([]);
  const [openCode, setOpenCode] = useState<OpenCodeProfileState | null>(null);
  const [selectedProfiles, setSelectedProfiles] = useState<string[]>([]);
  const [repairTarget, setRepairTarget] = useState<"openai" | "zenith_relay_local">("zenith_relay_local");
  const [repairPreview, setRepairPreview] = useState<HistoryRepairPreview | null>(null);
  const [repairResult, setRepairResult] = useState<HistoryRepairResult | null>(null);
  const account = runtime?.accounts[0];
  const key = runtime?.keys[0];

  const loadProfiles = useCallback(() => {
    if (mode !== "local") {
      setBindings([]); setOpenCode(null); return;
    }
    void Promise.all([relayCommands.profileBindings(), relayCommands.openCodeProfileState()])
      .then(([nextBindings, nextOpenCode]) => { setBindings(nextBindings); setOpenCode(nextOpenCode); })
      .catch(() => { setBindings([]); setOpenCode(null); });
  }, [mode]);
  useEffect(loadProfiles, [loadProfiles, runtime]);
  useEffect(() => setSelectedProfiles((current) => current.length ? current.filter((path) => bindings.some((binding) => binding.profileDir === path)) : bindings.map((binding) => binding.profileDir)), [bindings]);

  const attachCodex = async () => {
    const work = account ? () => relayCommands.attachCodexAccount(account.id) : key ? () => relayCommands.attachCodexGateway(key.id) : null;
    if (work && await perform("profile-attach", work, "feedback.profileAttached")) loadProfiles();
  };
  const attachOpenCode = async () => { if (key && await perform("opencode-attach", () => relayCommands.attachOpenCodeGateway(key.id), "feedback.profileAttached")) loadProfiles(); };
  const restore = async (work: () => Promise<unknown>, id: string) => {
    if (window.confirm(t("profiles.restoreConfirm")) && await perform(id, work, "feedback.restored")) loadProfiles();
  };
  const hasProfiles = bindings.length > 0 || Boolean(openCode?.backupAvailable);
  const primary = bindings.length ? () => perform("profile-launch", relayCommands.launchCodex, "feedback.launched") : attachCodex;
  const previewRepair = async () => {
    const result: { current: HistoryRepairPreview | null } = { current: null };
    const ok = await perform("history-repair-preview", async () => { result.current = await relayCommands.previewHistoryRepair(selectedProfiles, repairTarget); });
    if (ok) { setRepairPreview(result.current); setRepairResult(null); }
  };
  const applyRepair = async () => {
    if (!repairPreview || !window.confirm(t("profiles.repairConfirm"))) return;
    const result: { current: HistoryRepairResult | null } = { current: null };
    const ok = await perform("history-repair-apply", async () => { result.current = await relayCommands.applyHistoryRepair(repairPreview.sessionId); }, "feedback.saved");
    if (ok) { setRepairResult(result.current); setRepairPreview(null); }
  };
  const rollbackRepair = async () => {
    if (!repairResult || !window.confirm(t("profiles.rollbackConfirm"))) return;
    const ok = await perform("history-repair-rollback", () => relayCommands.rollbackHistoryRepair(repairResult.backupId), "feedback.restored");
    if (ok) setRepairResult(null);
  };
  const resetRepairPreview = () => { setRepairPreview(null); setRepairResult(null); };

  return <section className="relay-page">
    <PageHeader title={t("nav.profiles")} subtitle={t("profiles.subtitle")} actions={<Button variant="primary" icon={bindings.length ? <Play aria-hidden /> : <Plus aria-hidden />} disabled={mode !== "local" || (!bindings.length && !account && !key)} busy={busy === (bindings.length ? "profile-launch" : "profile-attach")} onClick={primary}>{bindings.length ? t("profiles.launchSelected") : t("profiles.attachCodex")}</Button>} />
    <Tabs value={view} onChange={setView} label={t("profiles.views")} items={[{id:"profiles",label:t("nav.profiles")},{id:"backups",label:t("profiles.backups")},{id:"repair",label:t("profiles.repair")}]} />
    {view === "profiles" ? mode !== "local" ? <EmptyState title={t("profiles.localOnlyTitle")} description={t("profiles.localOnlyDescription")} /> : hasProfiles ? <div className="relay-table-wrap"><table className="relay-table"><thead><tr><th>{t("common.status")}</th><th>{t("common.name")}</th><th>{t("profiles.client")}</th><th>{t("gateway.endpoint")}</th><th>{t("profiles.backup")}</th><th><span className="sr-only">{t("common.actions")}</span></th></tr></thead><tbody>
      {bindings.map((binding) => <tr key={binding.profileDir}><td><StatusBadge status="ready" label={t("profiles.attached")} /></td><td><strong>Codex</strong><small>{binding.profileDir}</small></td><td>Codex</td><td><code>{runtime?.gateway.baseUrl}</code></td><td>{t("profiles.available")}</td><td><Button variant="ghost" aria-label={t("profiles.restore")} icon={<RotateCcw aria-hidden />} onClick={() => restore(() => relayCommands.restoreAccountProfile(binding.profileDir), `restore-${binding.profileDir}`)}>{t("profiles.restore")}</Button></td></tr>)}
      {openCode?.backupAvailable ? <tr><td><StatusBadge status={openCode.changed ? "warning" : "ready"} label={openCode.changed ? t("profiles.changed") : t("profiles.attached")} /></td><td><strong>OpenCode</strong><small>{openCode.configPath}</small></td><td>OpenCode</td><td><code>{runtime?.gateway.baseUrl}</code></td><td>{t("profiles.available")}</td><td><Button variant="ghost" aria-label={t("profiles.restore")} icon={<RotateCcw aria-hidden />} onClick={() => restore(relayCommands.restoreOpenCode, "opencode-restore")}>{t("profiles.restore")}</Button></td></tr> : null}
    </tbody></table></div> : <EmptyState title={t("profiles.emptyTitle")} description={t("profiles.emptyDescription")} action={<div className="inline-actions"><Button variant="primary" onClick={attachCodex}>{t("profiles.attachCodex")}</Button><Button variant="secondary" disabled={!key} onClick={attachOpenCode}>{t("profiles.attachOpenCode")}</Button></div>} /> : null}
    {view === "backups" ? <section className="flat-section"><h2>{t("profiles.backups")}</h2><p>{t("profiles.backupHint")}</p><div className="inline-actions"><Button variant="secondary" icon={<FolderOpen aria-hidden />} busy={busy === "profile-open-folder"} onClick={() => perform("profile-open-folder", () => relayCommands.openFolder("profile_backups"), "feedback.opened")}>{t("profiles.openFolder")}</Button><Button variant="primary" icon={<RotateCcw aria-hidden />} disabled={mode !== "local"} onClick={() => restore(relayCommands.restoreCodex, "profile-restore")}>Codex</Button><Button variant="secondary" icon={<RotateCcw aria-hidden />} disabled={mode !== "local" || !openCode?.backupAvailable} onClick={() => restore(relayCommands.restoreOpenCode, "opencode-restore")}>OpenCode</Button></div></section> : null}
    {view === "repair" ? <section className="flat-section history-repair"><h2><Wrench aria-hidden />{t("profiles.historyRepair")}</h2><p>{t("profiles.historyRepairHint")}</p>{bindings.length ? <fieldset><legend>{t("profiles.instances")}</legend><div className="scope-grid">{bindings.map((binding) => <label key={binding.profileDir}><input type="checkbox" checked={selectedProfiles.includes(binding.profileDir)} onChange={() => { setSelectedProfiles((current) => current.includes(binding.profileDir) ? current.filter((path) => path !== binding.profileDir) : [...current, binding.profileDir]); resetRepairPreview(); }} />{binding.profileDir}</label>)}</div></fieldset> : <p className="form-note">{t("profiles.defaultInstance")}</p>}<label className="relay-field"><span>{t("profiles.targetProvider")}</span><select value={repairTarget} onChange={(event) => { setRepairTarget(event.target.value as typeof repairTarget); resetRepairPreview(); }}><option value="zenith_relay_local">Zenith Relay Local</option><option value="openai">OpenAI</option></select></label><div className="inline-actions"><Button variant="secondary" busy={busy === "history-repair-preview"} disabled={mode !== "local" || (bindings.length > 0 && selectedProfiles.length === 0)} onClick={previewRepair}>{t("profiles.previewRepair")}</Button>{repairPreview ? <Button variant="primary" busy={busy === "history-repair-apply"} disabled={repairPreview.codexRunning || repairPreview.rolloutRecordCount + repairPreview.sqliteRowCount === 0} title={repairPreview.codexRunning ? t("profiles.runningWarning") : undefined} onClick={applyRepair}>{t("profiles.applyRepair")}</Button> : null}</div>{repairPreview ? <div className="settings-status" role="status"><StatusBadge status={repairPreview.rolloutRecordCount + repairPreview.sqliteRowCount ? "warning" : "ready"} label={t("profiles.previewReady")} /><dl className="detail-list"><div><dt>{t("profiles.rolloutFiles")}</dt><dd>{repairPreview.rolloutFileCount}</dd></div><div><dt>{t("profiles.rolloutRecords")}</dt><dd>{repairPreview.rolloutRecordCount}</dd></div><div><dt>{t("profiles.databaseRows")}</dt><dd>{repairPreview.sqliteRowCount}</dd></div></dl>{repairPreview.codexRunning ? <p className="warning-box">{t("profiles.runningWarning")}</p> : null}</div> : null}{repairResult ? <div className="settings-status" role="status"><StatusBadge status="ready" label={t("profiles.repairComplete")} /><code>{repairResult.backupPath}</code><Button variant="secondary" busy={busy === "history-repair-rollback"} onClick={rollbackRepair}>{t("profiles.rollbackRepair")}</Button></div> : null}</section> : null}
  </section>;
}
