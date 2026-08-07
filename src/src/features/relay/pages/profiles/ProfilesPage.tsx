import { useCallback, useEffect, useState } from "react";
import { AlertTriangle, ArrowLeft, Camera, CircleAlert, FolderOpen, History, RotateCcw, ShieldCheck, Trash2 } from "lucide-react";
import { useTranslation } from "react-i18next";
import { relayCommands } from "../../api/commands";
import type { ProfileBinding, ProfileSnapshot } from "../../api/types";
import { Button, Dialog, EmptyState, IconButton, PageHeader, StatusIcon, useConfirm } from "../../components/Ui";
import { useRelayState } from "../../state/RelayStateProvider";

type SnapshotRestoreMode = "managed" | "full";

export function ProfilesPage() {
  const { i18n, t } = useTranslation();
  const { mode, busy, perform, profileSnapshotBackupBeforeRestore } = useRelayState();
  const confirm = useConfirm();
  const [bindings, setBindings] = useState<ProfileBinding[]>([]);
  const [snapshots, setSnapshots] = useState<ProfileSnapshot[]>([]);
  const [snapshotName, setSnapshotName] = useState("");
  const [snapshotRestoreTarget, setSnapshotRestoreTarget] = useState<ProfileSnapshot | null>(null);
  const [snapshotRestoreMode, setSnapshotRestoreMode] = useState<SnapshotRestoreMode>("managed");
  const [saveCurrentBeforeRestore, setSaveCurrentBeforeRestore] = useState(profileSnapshotBackupBeforeRestore);
  const [loadFailed, setLoadFailed] = useState(false);
  const snapshotRestoreBusy = busy === "profile-snapshot-restore" || busy === "profile-snapshot-full-restore";

  const loadRecovery = useCallback(() => {
    if (mode !== "local") {
      setBindings([]);
      setSnapshots([]);
      setLoadFailed(false);
      return;
    }
    setLoadFailed(false);
    void Promise.allSettled([relayCommands.profileBindings(), relayCommands.profileSnapshots()]).then(([loadedBindings, loadedSnapshots]) => {
      setBindings(loadedBindings.status === "fulfilled" ? loadedBindings.value : []);
      setSnapshots(loadedSnapshots.status === "fulfilled" ? loadedSnapshots.value.snapshots : []);
      setLoadFailed(loadedBindings.status === "rejected" || loadedSnapshots.status === "rejected" || (loadedSnapshots.status === "fulfilled" && loadedSnapshots.value.invalidCount > 0));
    });
  }, [mode]);

  useEffect(loadRecovery, [loadRecovery]);

  const createSnapshot = async () => {
    const name = snapshotName.trim();
    if (name && await perform("profile-snapshot-create", () => relayCommands.createProfileSnapshot(name), "feedback.snapshotCreated")) {
      setSnapshotName("");
      loadRecovery();
    }
  };

  const requestSnapshotRestore = (snapshot: ProfileSnapshot) => {
    setSaveCurrentBeforeRestore(profileSnapshotBackupBeforeRestore);
    setSnapshotRestoreMode("managed");
    setSnapshotRestoreTarget(snapshot);
  };

  const restoreSnapshot = async () => {
    const snapshot = snapshotRestoreTarget;
    if (!snapshot) return;
    const fullRestore = snapshotRestoreMode === "full";
    const safetyName = saveCurrentBeforeRestore
      ? Array.from(t("profiles.safetySnapshotName", { name: snapshot.name })).slice(0, 80).join("").trim()
      : null;
    if (await perform(fullRestore ? "profile-snapshot-full-restore" : "profile-snapshot-restore", () => fullRestore
      ? relayCommands.restoreFullProfileSnapshot(snapshot.id, safetyName)
      : relayCommands.restoreProfileSnapshot(snapshot.id, safetyName), "feedback.snapshotRestored")) {
      setSnapshotRestoreTarget(null);
      setSnapshotRestoreMode("managed");
      loadRecovery();
    }
  };

  const closeSnapshotRestore = () => {
    if (snapshotRestoreBusy) return;
    setSnapshotRestoreTarget(null);
    setSnapshotRestoreMode("managed");
  };

  const deleteSnapshot = async (snapshot: ProfileSnapshot) => {
    if (await confirm(t("profiles.snapshotDeleteConfirm", { name: snapshot.name }), { danger: true })
      && await perform("profile-snapshot-delete", () => relayCommands.deleteProfileSnapshot(snapshot.id), "feedback.deleted")) {
      loadRecovery();
    }
  };

  const restoreAutomatic = async (binding: ProfileBinding) => {
    if (await confirm(t("profiles.restoreConfirm", { profile: displayProfilePath(binding.profileDir) }))
      && await perform("profile-restore", () => binding.credentialKind === "oauth_account"
        ? relayCommands.restoreAccountProfile(binding.profileDir)
        : relayCommands.restoreCodex(), "feedback.restored")) {
      loadRecovery();
    }
  };

  const snapshotDate = (value: number) => new Intl.DateTimeFormat(i18n.language, {
    dateStyle: "medium",
    timeStyle: "short",
  }).format(new Date(value));
  const displayProfilePath = (value: string) => value.startsWith("\\\\?\\") ? value.slice(4) : value;

  const snapshotForm = <form className="profile-snapshot-create" onSubmit={(event) => { event.preventDefault(); void createSnapshot(); }}>
    <label className="relay-field">
      <span>{t("profiles.snapshotName")}</span>
      <input value={snapshotName} maxLength={80} onChange={(event) => setSnapshotName(event.target.value)} placeholder={t("profiles.snapshotNamePlaceholder")} />
    </label>
    <Button type="submit" variant="primary" icon={<Camera aria-hidden />} busy={busy === "profile-snapshot-create"} disabled={!snapshotName.trim()}>{t("profiles.createSnapshot")}</Button>
  </form>;

  return <section className="relay-page profile-recovery-page">
    <PageHeader title={t("nav.profiles")} subtitle={t("profiles.subtitle")} actions={mode === "local" ? <Button variant="secondary" icon={<FolderOpen aria-hidden />} busy={busy === "profile-open-folder"} onClick={() => perform("profile-open-folder", () => relayCommands.openFolder("profile_backups"), "feedback.opened")}>{t("profiles.openFolder")}</Button> : null} />
    {mode !== "local" ? <EmptyState title={t("profiles.localOnlyTitle")} description={t("profiles.localOnlyDescription")} /> : <section className={`profile-recovery${snapshots.length ? "" : " is-empty"}`}>
      {loadFailed ? <div className="profile-recovery-warning" role="alert"><CircleAlert aria-hidden /><span>{t("profiles.loadFailed")}</span><Button variant="secondary" onClick={loadRecovery}>{t("common.retry")}</Button></div> : null}

      {bindings.length ? <section className="profile-recovery-section profile-automatic-section"><header className="profile-recovery-section-heading"><span><ShieldCheck aria-hidden /></span><div><h2>{t("profiles.automaticSectionTitle")}</h2><small>{t("profiles.automaticSectionHint")}</small></div></header><div className="profile-automatic-backups">{bindings.map((binding) => <div className="profile-automatic-backup" key={`${binding.credentialKind}:${binding.profileDir}`}>
        <div><strong>{t("profiles.automaticBackup")}</strong><small><code title={displayProfilePath(binding.profileDir)}>{displayProfilePath(binding.profileDir)}</code></small></div>
        <StatusIcon status={binding.active ? "ready" : "warning"} label={t(binding.active ? "profiles.automaticBackupReady" : "profiles.automaticBackupChanged")} />
        <Button variant="secondary" icon={<RotateCcw aria-hidden />} busy={busy === "profile-restore"} disabled={!binding.active || busy !== null} title={!binding.active ? t("profiles.automaticBackupChanged") : undefined} onClick={() => void restoreAutomatic(binding)}>{t("profiles.restoreAutomatic")}</Button>
      </div>)}</div></section> : null}

      <section className="profile-recovery-section profile-named-section"><header className="profile-recovery-section-heading"><span><History aria-hidden /></span><div><h2>{t("profiles.namedSectionTitle")}</h2><small>{t("profiles.namedSectionHint")}</small></div></header>
      {snapshots.length ? <>{snapshotForm}<div className="relay-table-wrap profile-snapshot-table-wrap"><table className="relay-table profile-snapshot-table"><thead><tr>
        <th>{t("common.name")}</th>
        <th>{t("profiles.created")}</th>
        <th>{t("profiles.contents")}</th>
        <th><span className="sr-only">{t("common.actions")}</span></th>
      </tr></thead><tbody>{snapshots.map((snapshot) => <tr key={snapshot.id}>
        <td><strong title={snapshot.name}>{snapshot.name}</strong><small title={displayProfilePath(snapshot.profileDir)}>{displayProfilePath(snapshot.profileDir)}</small></td>
        <td>{snapshotDate(snapshot.createdAtMs)}</td>
        <td><StatusIcon status={snapshot.configAvailable && snapshot.authAvailable ? "ready" : "info"} label={snapshot.configAvailable && snapshot.authAvailable ? t("profiles.snapshotComplete") : t("profiles.snapshotPartial")} /></td>
        <td><div className="inline-actions"><Button variant="secondary" icon={<RotateCcw aria-hidden />} aria-label={t("profiles.restoreSnapshot", { name: snapshot.name })} disabled={busy !== null} onClick={() => requestSnapshotRestore(snapshot)}>{t("profiles.restoreAutomatic")}</Button><IconButton className="danger" label={t("profiles.deleteSnapshot", { name: snapshot.name })} icon={<Trash2 aria-hidden />} disabled={busy !== null} onClick={() => deleteSnapshot(snapshot)} /></div></td>
      </tr>)}</tbody></table></div></> : loadFailed ? null : <div className="profile-recovery-empty-state"><EmptyState title={t("profiles.noSnapshots")} description={t("profiles.noSnapshotsHint")} />{snapshotForm}</div>}
      </section>
    </section>}
    {snapshotRestoreTarget ? <Dialog
      title={snapshotRestoreMode === "full" ? t("profiles.snapshotFullRestoreTitle") : t("profiles.snapshotRestoreTitle")}
      onClose={closeSnapshotRestore}
      footer={<><Button variant="secondary" disabled={snapshotRestoreBusy} onClick={closeSnapshotRestore}>{t("common.cancel")}</Button><Button variant={snapshotRestoreMode === "full" ? "danger" : "primary"} busy={snapshotRestoreBusy} disabled={snapshotRestoreBusy} onClick={() => void restoreSnapshot()}>{t(snapshotRestoreMode === "full" ? "profiles.snapshotFullRestoreAction" : "profiles.snapshotRestoreAction")}</Button></>}
    >
      {snapshotRestoreMode === "full" ? <div className="snapshot-restore-full-warning">
        <p className="confirm-dialog-message">{t("profiles.snapshotFullRestoreConfirm", { name: snapshotRestoreTarget.name })}</p>
        <p className="confirm-dialog-message">{t("profiles.snapshotFullRestoreHint")}</p>
        <Button variant="ghost" icon={<ArrowLeft aria-hidden />} disabled={snapshotRestoreBusy} onClick={() => setSnapshotRestoreMode("managed")}>{t("profiles.snapshotFullRestoreBack")}</Button>
      </div> : <>
        <p className="confirm-dialog-message">{t("profiles.snapshotRestoreConfirm", { name: snapshotRestoreTarget.name })}</p>
        <div className="snapshot-restore-scope">
          <strong>{t("profiles.snapshotRestoreScopeTitle")}</strong>
          <span>{t("profiles.snapshotRestoreScopeHint")}</span>
          <Button variant="danger" icon={<AlertTriangle aria-hidden />} disabled={snapshotRestoreBusy} onClick={() => setSnapshotRestoreMode("full")}>{t("profiles.snapshotFullRestoreAction")}</Button>
        </div>
      </>}
      <label className="snapshot-restore-backup-choice">
        <input type="checkbox" checked={saveCurrentBeforeRestore} disabled={snapshotRestoreBusy} onChange={(event) => setSaveCurrentBeforeRestore(event.target.checked)} />
        <span>{t("profiles.snapshotBackupBeforeRestore")}</span>
      </label>
    </Dialog> : null}
  </section>;
}
