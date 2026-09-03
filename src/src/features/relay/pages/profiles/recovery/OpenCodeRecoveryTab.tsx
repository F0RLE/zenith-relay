import { useCallback, useEffect, useState } from "react";
import { Camera, RotateCcw } from "lucide-react";
import { useTranslation } from "react-i18next";
import { relayCommands } from "../../../api/commands";
import type { OpenCodeConfigStatus } from "../../../api/types";
import { Button, StatusIcon } from "../../../components/Ui";
import { useRelayState } from "../../../state/RelayStateProvider";
import { RecoveryConfirmationDialog, RecoveryEmptyState, RecoverySnapshotTable, RecoverySurface, type RecoverySnapshotRow } from "./RecoverySurface";

export function OpenCodeRecoveryTab() {
  const { i18n, t } = useTranslation();
  const { busy, perform } = useRelayState();
  const [status, setStatus] = useState<OpenCodeConfigStatus | null>(null);
  const [snapshotName, setSnapshotName] = useState("");
  const [restoreRequested, setRestoreRequested] = useState(false);
  const refreshStatus = useCallback(() => {
    void relayCommands.getOpenCodeConfigStatus().then(setStatus).catch(() => setStatus(null));
  }, []);
  useEffect(refreshStatus, [refreshStatus]);

  const hasSnapshot = Boolean(status?.hasBackup);
  const snapshotDate = (value: number | null | undefined) => value ? new Intl.DateTimeFormat(i18n.language, { dateStyle: "medium", timeStyle: "short" }).format(new Date(value)) : "-";
  const createSnapshot = async () => {
    const name = snapshotName.trim();
    if (name && !hasSnapshot && await perform("opencode-snapshot-create", () => relayCommands.createOpenCodeSnapshot(name), "feedback.openCodeSnapshotCreated")) {
      setSnapshotName("");
      refreshStatus();
    }
  };
  const restore = async () => {
    if (await perform("opencode-restore", relayCommands.restoreOpenCodeConfig, "feedback.restored")) {
      setRestoreRequested(false);
      refreshStatus();
    }
  };
  const rows: RecoverySnapshotRow[] = hasSnapshot ? [{
    id: "opencode",
    name: status?.backupName || t("profiles.openCodeSnapshotName"),
    createdAt: snapshotDate(status?.backupCreatedAtMs),
    contents: <StatusIcon status="ready" label={t("profiles.openCodeSnapshotContents")} />,
    actions: <Button variant="secondary" icon={<RotateCcw aria-hidden />} aria-label={t("profiles.openCodeRestore")} disabled={Boolean(busy)} onClick={() => setRestoreRequested(true)}>{t("profiles.restoreAction")}</Button>,
  }] : [];

  return <><RecoverySurface className="profile-recovery-opencode" isEmpty={!hasSnapshot} title={t("profiles.openCodeSectionTitle")} hint={t("profiles.openCodeSectionHint")}>
    <form className="profile-snapshot-create opencode-snapshot-create" onSubmit={(event) => { event.preventDefault(); void createSnapshot(); }}><label className="relay-field"><span>{t("profiles.snapshotName")}</span><input value={snapshotName} maxLength={80} onChange={(event) => setSnapshotName(event.target.value)} placeholder={t("profiles.openCodeSnapshotPlaceholder")} /></label><Button type="submit" variant="primary" icon={<Camera aria-hidden />} busy={busy === "opencode-snapshot-create"} disabled={!snapshotName.trim() || hasSnapshot || Boolean(busy)}>{t("profiles.openCodeCreateSnapshot")}</Button></form>
    {rows.length ? <RecoverySnapshotTable rows={rows} /> : <RecoveryEmptyState title={t("profiles.openCodeNoSnapshot")} description={t("profiles.openCodeNoSnapshotHint")} />}
  </RecoverySurface>
  {restoreRequested ? <RecoveryConfirmationDialog title={t("profiles.openCodeRestoreTitle")} confirmation={t("profiles.openCodeRestoreConfirm")} hint={t("profiles.openCodeRestoreHint")} busy={busy === "opencode-restore"} onCancel={() => setRestoreRequested(false)} onConfirm={() => void restore()} /> : null}</>;
}
