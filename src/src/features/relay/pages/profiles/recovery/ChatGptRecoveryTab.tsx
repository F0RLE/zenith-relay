import { useCallback, useEffect, useState } from "react";
import { Camera, CircleAlert, FolderOpen, RotateCcw, Trash2 } from "lucide-react";
import { useTranslation } from "react-i18next";
import { relayCommands } from "../../../api/commands";
import type { ProfileSnapshot } from "../../../api/types";
import { Button, IconButton, StatusIcon } from "../../../components/Ui";
import { useRelayState } from "../../../state/RelayStateProvider";
import { RecoveryConfirmationDialog, RecoveryEmptyState, RecoverySnapshotTable, RecoverySurface, type RecoverySnapshotRow } from "./RecoverySurface";

export function ChatGptRecoveryHeaderAction() {
  const { t } = useTranslation();
  const { busy, perform } = useRelayState();
  return <Button variant="secondary" icon={<FolderOpen aria-hidden />} busy={busy === "profile-open-folder"} onClick={() => perform("profile-open-folder", () => relayCommands.openFolder("profile_backups"), "feedback.opened")}>{t("profiles.openFolder")}</Button>;
}

export function ChatGptRecoveryTab() {
  const { i18n, t } = useTranslation();
  const { busy, perform } = useRelayState();
  const [snapshots, setSnapshots] = useState<ProfileSnapshot[]>([]);
  const [snapshotName, setSnapshotName] = useState("");
  const [restoreTarget, setRestoreTarget] = useState<ProfileSnapshot | null>(null);
  const [deleteTarget, setDeleteTarget] = useState<ProfileSnapshot | null>(null);
  const [loadFailed, setLoadFailed] = useState(false);

  const loadSnapshots = useCallback(() => {
    setLoadFailed(false);
    void relayCommands.profileSnapshots().then((result) => {
      setSnapshots(result.snapshots);
      setLoadFailed(result.invalidCount > 0);
    }).catch(() => {
      setSnapshots([]);
      setLoadFailed(true);
    });
  }, []);

  useEffect(loadSnapshots, [loadSnapshots]);

  const createSnapshot = async () => {
    const name = snapshotName.trim();
    if (name && await perform("profile-snapshot-create", () => relayCommands.createProfileSnapshot(name), "feedback.snapshotCreated")) {
      setSnapshotName("");
      loadSnapshots();
    }
  };

  const restoreSnapshot = async () => {
    const snapshot = restoreTarget;
    if (snapshot && await perform("profile-snapshot-restore", () => relayCommands.restoreProfileSnapshot(snapshot.id), "feedback.snapshotRestored")) {
      setRestoreTarget(null);
      loadSnapshots();
    }
  };

  const deleteSnapshot = async () => {
    const snapshot = deleteTarget;
    if (snapshot && await perform("profile-snapshot-delete", () => relayCommands.deleteProfileSnapshot(snapshot.id), "feedback.deleted")) {
      setDeleteTarget(null);
      loadSnapshots();
    }
  };

  const snapshotDate = (value: number) => new Intl.DateTimeFormat(i18n.language, { dateStyle: "medium", timeStyle: "short" }).format(new Date(value));
  const displayPath = (value: string) => value.startsWith("\\\\?\\") ? value.slice(4) : value;
  const rows: RecoverySnapshotRow[] = snapshots.map((snapshot) => ({
    id: snapshot.id,
    name: snapshot.name,
    detail: displayPath(snapshot.profileDir),
    createdAt: snapshotDate(snapshot.createdAtMs),
    contents: <StatusIcon status={snapshot.configAvailable && snapshot.authAvailable ? "ready" : "info"} label={snapshot.configAvailable && snapshot.authAvailable ? t("profiles.snapshotComplete") : t("profiles.snapshotPartial")} />,
    actions: <><Button variant="secondary" icon={<RotateCcw aria-hidden />} aria-label={t("profiles.restoreSnapshot", { name: snapshot.name })} disabled={Boolean(busy)} onClick={() => setRestoreTarget(snapshot)}>{t("profiles.restoreAction")}</Button><IconButton className="danger" label={t("profiles.deleteSnapshot", { name: snapshot.name })} icon={<Trash2 aria-hidden />} disabled={Boolean(busy)} onClick={() => setDeleteTarget(snapshot)} /></>,
  }));

  const notice = loadFailed ? <div className="profile-recovery-warning" role="alert"><CircleAlert aria-hidden /><span>{t("profiles.loadFailed")}</span><Button variant="secondary" onClick={loadSnapshots}>{t("common.retry")}</Button></div> : null;
  return <><RecoverySurface isEmpty={!snapshots.length} title={t("profiles.chatGptSectionTitle")} hint={t("profiles.chatGptSectionHint")} notice={notice}>
    <form className="profile-snapshot-create" onSubmit={(event) => { event.preventDefault(); void createSnapshot(); }}>
      <label className="relay-field"><span>{t("profiles.snapshotName")}</span><input value={snapshotName} maxLength={80} onChange={(event) => setSnapshotName(event.target.value)} placeholder={t("profiles.snapshotNamePlaceholder")} /></label>
      <Button type="submit" variant="primary" icon={<Camera aria-hidden />} busy={busy === "profile-snapshot-create"} disabled={!snapshotName.trim() || Boolean(busy)}>{t("profiles.createSnapshot")}</Button>
    </form>
    {rows.length ? <RecoverySnapshotTable rows={rows} /> : loadFailed ? null : <RecoveryEmptyState title={t("profiles.noSnapshots")} description={t("profiles.noSnapshotsHint")} />}
  </RecoverySurface>
  {restoreTarget ? <RecoveryConfirmationDialog title={t("profiles.snapshotRestoreTitle")} confirmation={t("profiles.snapshotRestoreConfirm", { name: restoreTarget.name })} hint={t("profiles.snapshotFullRestoreHint")} busy={busy === "profile-snapshot-restore"} onCancel={() => setRestoreTarget(null)} onConfirm={() => void restoreSnapshot()} /> : null}
  {deleteTarget ? <RecoveryConfirmationDialog title={t("profiles.snapshotDeleteTitle")} confirmation={t("profiles.snapshotDeleteConfirm", { name: deleteTarget.name })} busy={busy === "profile-snapshot-delete"} onCancel={() => setDeleteTarget(null)} onConfirm={() => void deleteSnapshot()} /> : null}</>;
}
