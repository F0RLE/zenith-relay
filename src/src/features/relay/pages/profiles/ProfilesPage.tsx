import { useCallback, useEffect, useState } from "react";
import { Camera, CircleAlert, FolderOpen, History, RotateCcw, Trash2 } from "lucide-react";
import { useTranslation } from "react-i18next";
import { relayCommands } from "../../api/commands";
import type { ProfileSnapshot } from "../../api/types";
import { Button, Dialog, EmptyState, IconButton, PageHeader, StatusIcon, useConfirm } from "../../components/Ui";
import { useRelayState } from "../../state/RelayStateProvider";

const DELETE_COOLDOWN_SECONDS = 10;

export function ProfilesPage() {
  const { i18n, t } = useTranslation();
  const { mode, busy, perform, readyState } = useRelayState();
  const confirm = useConfirm();
  const [snapshots, setSnapshots] = useState<ProfileSnapshot[]>([]);
  const [snapshotName, setSnapshotName] = useState("");
  const [snapshotRestoreTarget, setSnapshotRestoreTarget] = useState<ProfileSnapshot | null>(null);
  const [snapshotDeleteTarget, setSnapshotDeleteTarget] = useState<ProfileSnapshot | null>(null);
  const [deleteCountdown, setDeleteCountdown] = useState(0);
  const [loadFailed, setLoadFailed] = useState(false);
  const snapshotRestoreBusy = busy === "profile-snapshot-restore";
  const snapshotDeleteBusy = busy === "profile-snapshot-delete";

  const loadRecovery = useCallback(() => {
    if (mode !== "local") {
      setSnapshots([]);
      setLoadFailed(false);
      return;
    }
    setLoadFailed(false);
    void relayCommands.profileSnapshots().then((result) => {
      setSnapshots(result.snapshots);
      setLoadFailed(result.invalidCount > 0);
    }).catch(() => {
      setSnapshots([]);
      setLoadFailed(true);
    });
  }, [mode]);

  useEffect(loadRecovery, [loadRecovery]);

  useEffect(() => {
    if (!snapshotDeleteTarget || deleteCountdown <= 0) return;
    const timer = window.setInterval(() => {
      setDeleteCountdown((value) => Math.max(0, value - 1));
    }, 1_000);
    return () => window.clearInterval(timer);
  }, [snapshotDeleteTarget, deleteCountdown]);

  const createSnapshot = async () => {
    const name = snapshotName.trim();
    if (name && readyState?.codexRunning && !await confirm(t("profiles.snapshotRestartMessage"), {
      title: t("profiles.snapshotRestartTitle"),
      confirmLabel: t("profiles.snapshotRestartAction"),
    })) return;
    if (name && await perform("profile-snapshot-create", () => relayCommands.createProfileSnapshot(name), "feedback.snapshotCreated")) {
      setSnapshotName("");
      loadRecovery();
    }
  };

  const restoreSnapshot = async () => {
    const snapshot = snapshotRestoreTarget;
    if (!snapshot) return;
    if (await perform("profile-snapshot-restore", () => relayCommands.restoreProfileSnapshot(snapshot.id), "feedback.snapshotRestored")) {
      setSnapshotRestoreTarget(null);
      loadRecovery();
    }
  };

  const requestSnapshotDelete = (snapshot: ProfileSnapshot) => {
    setSnapshotDeleteTarget(snapshot);
    setDeleteCountdown(DELETE_COOLDOWN_SECONDS);
  };

  const deleteSnapshot = async () => {
    const snapshot = snapshotDeleteTarget;
    if (!snapshot || deleteCountdown > 0) return;
    if (await perform("profile-snapshot-delete", () => relayCommands.deleteProfileSnapshot(snapshot.id), "feedback.deleted")) {
      setSnapshotDeleteTarget(null);
      loadRecovery();
    }
  };

  const snapshotDate = (value: number) => new Intl.DateTimeFormat(i18n.language, {
    dateStyle: "medium",
    timeStyle: "short",
  }).format(new Date(value));
  const displayProfilePath = (value: string) => value.startsWith("\\\\?\\") ? value.slice(4) : value;
  const snapshotDisplayName = (snapshot: ProfileSnapshot) => snapshot.isOriginal ? t("profiles.originalSnapshotName") : snapshot.name;

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

      <section className="profile-recovery-section profile-named-section"><header className="profile-recovery-section-heading"><span><History aria-hidden /></span><div><h2>{t("profiles.namedSectionTitle")}</h2><small>{t("profiles.namedSectionHint")}</small></div></header>
        {snapshotForm}
        {snapshots.length ? <div className="relay-table-wrap profile-snapshot-table-wrap"><table className="relay-table profile-snapshot-table"><thead><tr>
          <th>{t("common.name")}</th>
          <th>{t("profiles.created")}</th>
          <th>{t("profiles.contents")}</th>
          <th><span className="sr-only">{t("common.actions")}</span></th>
        </tr></thead><tbody>{snapshots.map((snapshot) => {
          const name = snapshotDisplayName(snapshot);
          return <tr key={snapshot.id} data-original={snapshot.isOriginal ? "true" : undefined}>
            <td><strong title={name}>{name}</strong><small title={displayProfilePath(snapshot.profileDir)}>{displayProfilePath(snapshot.profileDir)}</small></td>
            <td>{snapshotDate(snapshot.createdAtMs)}</td>
            <td><StatusIcon status={snapshot.configAvailable && snapshot.authAvailable ? "ready" : "info"} label={snapshot.configAvailable && snapshot.authAvailable ? t("profiles.snapshotComplete") : t("profiles.snapshotPartial")} /></td>
            <td><div className="inline-actions"><Button variant="secondary" icon={<RotateCcw aria-hidden />} aria-label={t("profiles.restoreSnapshot", { name })} disabled={busy !== null} onClick={() => setSnapshotRestoreTarget(snapshot)}>{t("profiles.restoreAction")}</Button><IconButton className="danger" label={t("profiles.deleteSnapshot", { name })} icon={<Trash2 aria-hidden />} disabled={busy !== null} onClick={() => requestSnapshotDelete(snapshot)} /></div></td>
          </tr>;
        })}</tbody></table></div> : loadFailed ? null : <div className="profile-recovery-empty-state"><EmptyState title={t("profiles.noSnapshots")} description={t("profiles.noSnapshotsHint")} /></div>}
      </section>
    </section>}

    {snapshotRestoreTarget ? <Dialog
      title={t("profiles.snapshotRestoreTitle")}
      onClose={() => { if (!snapshotRestoreBusy) setSnapshotRestoreTarget(null); }}
      footer={<><Button variant="secondary" disabled={snapshotRestoreBusy} onClick={() => setSnapshotRestoreTarget(null)}>{t("common.no")}</Button><Button variant="danger" busy={snapshotRestoreBusy} disabled={snapshotRestoreBusy} onClick={() => void restoreSnapshot()}>{t("common.yes")}</Button></>}
    >
      <p className="confirm-dialog-message">{t("profiles.snapshotRestoreConfirm", { name: snapshotDisplayName(snapshotRestoreTarget) })}</p>
      <p className="confirm-dialog-message">{t("profiles.snapshotFullRestoreHint")}</p>
    </Dialog> : null}

    {snapshotDeleteTarget ? <Dialog
      title={t("profiles.snapshotDeleteTitle")}
      onClose={() => { if (!snapshotDeleteBusy) setSnapshotDeleteTarget(null); }}
      footer={<><Button variant="secondary" disabled={snapshotDeleteBusy} onClick={() => setSnapshotDeleteTarget(null)}>{t("common.no")}</Button><Button variant="danger" busy={snapshotDeleteBusy} disabled={snapshotDeleteBusy || deleteCountdown > 0} onClick={() => void deleteSnapshot()}>{deleteCountdown > 0 ? t("profiles.snapshotDeleteCountdown", { seconds: deleteCountdown }) : t("common.yes")}</Button></>}
    >
      <p className="confirm-dialog-message">{t("profiles.snapshotDeleteConfirm", { name: snapshotDisplayName(snapshotDeleteTarget) })}</p>
      {deleteCountdown > 0 ? <p className="snapshot-delete-countdown" role="status" aria-live="polite">{t("profiles.snapshotDeleteWait", { seconds: deleteCountdown })}</p> : null}
    </Dialog> : null}
  </section>;
}
