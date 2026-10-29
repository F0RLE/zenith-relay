import { useEffect, useState } from "react";
import { Camera, CircleAlert, FolderOpen, RotateCcw } from "lucide-react";
import { useTranslation } from "react-i18next";
import { relayCommands } from "../../../api/commands";
import type { ClaudeDesktopStatus } from "../../../api/types";
import { Button, StatusIcon } from "../../../components/Ui";
import { useRelayState } from "../../../state/RelayStateProvider";
import { RecoveryConfirmationDialog, RecoveryEmptyState, RecoverySnapshotTable, RecoverySurface, type RecoverySnapshotRow } from "./RecoverySurface";

export function ClaudeDesktopRecoveryHeaderAction() {
  const { t } = useTranslation();
  const { busy, perform } = useRelayState();
  return <Button variant="secondary" icon={<FolderOpen aria-hidden />} busy={busy === "claude-desktop-open-folder"} onClick={() => perform("claude-desktop-open-folder", () => relayCommands.openFolder("claude_desktop_backups"), "feedback.opened", { skipRefresh: true })}>{t("profiles.openFolder")}</Button>;
}

export function ClaudeDesktopRecoveryTab() {
  const { i18n, t } = useTranslation();
  const { busy, perform, runtimeRevision } = useRelayState();
  const [status, setStatus] = useState<ClaudeDesktopStatus | null>(null);
  const [loadFailed, setLoadFailed] = useState(false);
  const [reload, setReload] = useState(0);
  const [restoreRequested, setRestoreRequested] = useState(false);
  const [snapshotName, setSnapshotName] = useState("");
  const hasSnapshot = Boolean(status?.hasRecovery);
  useEffect(() => {
    let active = true;
    void relayCommands.getClaudeDesktopStatus().then((value) => {
      if (active) { setStatus(value); setLoadFailed(false); }
    }).catch(() => { if (active) { setStatus(null); setLoadFailed(true); } });
    return () => { active = false; };
  }, [reload, runtimeRevision]);
  const createSnapshot = async () => {
    const name = snapshotName.trim();
    if (name && status && !hasSnapshot && await perform("claude-desktop-snapshot-create", () => relayCommands.createClaudeDesktopSnapshot(name), "feedback.snapshotCreated", { backgroundRefresh: true })) {
      setSnapshotName("");
      setReload((value) => value + 1);
    }
  };
  const restore = async () => {
    if (await perform("claude-desktop-restore", relayCommands.restoreClaudeDesktop, "feedback.restored", { backgroundRefresh: true })) {
      setRestoreRequested(false);
      setReload((value) => value + 1);
    }
  };
  const rows: RecoverySnapshotRow[] = hasSnapshot ? [{
    id: "claude-desktop",
    name: status?.snapshotName || t("profiles.snapshotDefaultName"),
    createdAt: status?.snapshotCreatedAtMs ? new Intl.DateTimeFormat(i18n.language, { dateStyle: "medium", timeStyle: "short" }).format(new Date(status.snapshotCreatedAtMs)) : "-",
    contents: <StatusIcon status="ready" label={t("profiles.claudeDesktopSnapshotContents")} />,
    actions: <Button variant="secondary" icon={<RotateCcw aria-hidden />} disabled={Boolean(busy)} busy={busy === "claude-desktop-restore"} onClick={() => setRestoreRequested(true)}>{t("profiles.restoreAction")}</Button>,
  }] : [];
  return <>
    <RecoverySurface className="profile-recovery-claude-desktop" isEmpty={!hasSnapshot} notice={loadFailed ? <div className="profile-recovery-warning" role="alert">
      <CircleAlert aria-hidden />
      <span>{t("profiles.loadFailed")}</span>
      <Button variant="secondary" onClick={() => setReload((value) => value + 1)}>{t("common.retry")}</Button>
    </div> : null} composer={<form className="profile-snapshot-create" onSubmit={(event) => { event.preventDefault(); void createSnapshot(); }}>
      <label className="relay-field">
        <span>{t("profiles.snapshotName")}</span>
        <input value={snapshotName} maxLength={80} onChange={(event) => setSnapshotName(event.target.value)} placeholder={t("profiles.claudeDesktopSnapshotPlaceholder")} />
      </label>
      <Button type="submit" variant="primary" icon={<Camera aria-hidden />} disabled={Boolean(busy) || !status || !snapshotName.trim() || hasSnapshot} busy={busy === "claude-desktop-snapshot-create"}>
        {t("profiles.createSnapshot")}
      </Button>
    </form>}>
      {rows.length ? <RecoverySnapshotTable rows={rows} /> : status ? <RecoveryEmptyState title={t("profiles.noSnapshots")} /> : null}
    </RecoverySurface>
    {restoreRequested ? <RecoveryConfirmationDialog
      title={t("profiles.claudeDesktopRestoreTitle")}
      confirmation={t("gateway.claudeDesktopRestoreConfirm")}
      busy={busy === "claude-desktop-restore"}
      onCancel={() => setRestoreRequested(false)}
      onConfirm={() => void restore()}
    /> : null}
  </>;
}
