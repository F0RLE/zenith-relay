import { useCallback, useEffect, useState } from "react";
import { Camera, FolderOpen, RotateCcw, Trash2 } from "lucide-react";
import { useTranslation } from "react-i18next";
import { relayCommands } from "../../api/commands";
import type { ProfileBinding, ProfileSnapshot } from "../../api/types";
import { Button, EmptyState, IconButton, PageHeader, StatusIcon, useConfirm } from "../../components/Ui";
import { useRelayState } from "../../state/RelayStateProvider";

export function ProfilesPage() {
  const { i18n, t } = useTranslation();
  const { mode, busy, perform } = useRelayState();
  const confirm = useConfirm();
  const [bindings, setBindings] = useState<ProfileBinding[]>([]);
  const [snapshots, setSnapshots] = useState<ProfileSnapshot[]>([]);
  const [snapshotName, setSnapshotName] = useState("");

  const loadRecovery = useCallback(() => {
    if (mode !== "local") {
      setBindings([]);
      setSnapshots([]);
      return;
    }
    void relayCommands.profileBindings().then(setBindings).catch(() => setBindings([]));
    void relayCommands.profileSnapshots().then(setSnapshots).catch(() => setSnapshots([]));
  }, [mode]);

  useEffect(loadRecovery, [loadRecovery]);

  const createSnapshot = async () => {
    const name = snapshotName.trim();
    if (name && await perform("profile-snapshot-create", () => relayCommands.createProfileSnapshot(name), "feedback.snapshotCreated")) {
      setSnapshotName("");
      loadRecovery();
    }
  };

  const restoreSnapshot = async (snapshot: ProfileSnapshot) => {
    if (await confirm(t("profiles.snapshotRestoreConfirm", { name: snapshot.name }))
      && await perform("profile-snapshot-restore", () => relayCommands.restoreProfileSnapshot(snapshot.id, t("profiles.safetySnapshotName", { name: snapshot.name })), "feedback.snapshotRestored")) {
      loadRecovery();
    }
  };

  const deleteSnapshot = async (snapshot: ProfileSnapshot) => {
    if (await confirm(t("profiles.snapshotDeleteConfirm", { name: snapshot.name }), { danger: true })
      && await perform("profile-snapshot-delete", () => relayCommands.deleteProfileSnapshot(snapshot.id), "feedback.deleted")) {
      loadRecovery();
    }
  };

  const restoreAutomatic = async (binding: ProfileBinding) => {
    if (await confirm(t("profiles.restoreConfirm", { profile: binding.profileDir }))
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

  const snapshotForm = <form className="profile-snapshot-create" onSubmit={(event) => { event.preventDefault(); void createSnapshot(); }}>
    <label className="relay-field">
      <span>{t("profiles.snapshotName")}</span>
      <input value={snapshotName} maxLength={80} onChange={(event) => setSnapshotName(event.target.value)} placeholder={t("profiles.snapshotNamePlaceholder")} />
    </label>
    <Button type="submit" variant="primary" icon={<Camera aria-hidden />} busy={busy === "profile-snapshot-create"} disabled={!snapshotName.trim()}>{t("profiles.createSnapshot")}</Button>
  </form>;

  return <section className="relay-page profile-recovery-page">
    <PageHeader title={t("nav.profiles")} subtitle={t("profiles.subtitle")} />
    {mode !== "local" ? <EmptyState title={t("profiles.localOnlyTitle")} description={t("profiles.localOnlyDescription")} /> : <section className={`profile-recovery${snapshots.length ? "" : " is-empty"}`}>
      {snapshots.length ? <>{snapshotForm}<div className="relay-table-wrap profile-snapshot-table-wrap"><table className="relay-table profile-snapshot-table"><thead><tr>
        <th>{t("common.name")}</th>
        <th>{t("profiles.created")}</th>
        <th>{t("profiles.contents")}</th>
        <th><span className="sr-only">{t("common.actions")}</span></th>
      </tr></thead><tbody>{snapshots.map((snapshot) => <tr key={snapshot.id}>
        <td><strong title={snapshot.name}>{snapshot.name}</strong><small title={snapshot.profileDir}>{snapshot.profileDir}</small></td>
        <td>{snapshotDate(snapshot.createdAtMs)}</td>
        <td><StatusIcon status={snapshot.configAvailable && snapshot.authAvailable ? "ready" : "info"} label={snapshot.configAvailable && snapshot.authAvailable ? t("profiles.snapshotComplete") : t("profiles.snapshotPartial")} /></td>
        <td><div className="inline-actions"><IconButton label={t("profiles.restoreSnapshot", { name: snapshot.name })} icon={<RotateCcw aria-hidden />} disabled={busy !== null} onClick={() => restoreSnapshot(snapshot)} /><IconButton label={t("profiles.deleteSnapshot", { name: snapshot.name })} icon={<Trash2 aria-hidden />} disabled={busy !== null} onClick={() => deleteSnapshot(snapshot)} /></div></td>
      </tr>)}</tbody></table></div></> : <><EmptyState title={t("profiles.noSnapshots")} description={t("profiles.noSnapshotsHint")} />{snapshotForm}</>}

      {bindings.length ? <div className="profile-automatic-backups">{bindings.map((binding) => <div className="profile-automatic-backup" key={`${binding.credentialKind}:${binding.profileDir}`}>
        <div><strong>{t("profiles.automaticBackup")}</strong><small><code title={binding.profileDir}>{binding.profileDir}</code></small></div>
        <StatusIcon status={binding.active ? "ready" : "warning"} label={t(binding.active ? "profiles.automaticBackupReady" : "profiles.automaticBackupChanged")} />
        <Button variant="secondary" icon={<RotateCcw aria-hidden />} busy={busy === "profile-restore"} disabled={!binding.active || busy !== null} title={!binding.active ? t("profiles.automaticBackupChanged") : undefined} onClick={() => void restoreAutomatic(binding)}>{t("profiles.restoreAutomatic")}</Button>
      </div>)}</div> : null}

      <div className="inline-actions profile-backup-tools">
        <Button variant="secondary" icon={<FolderOpen aria-hidden />} busy={busy === "profile-open-folder"} onClick={() => perform("profile-open-folder", () => relayCommands.openFolder("profile_backups"), "feedback.opened")}>{t("profiles.openFolder")}</Button>
      </div>
    </section>}
  </section>;
}
