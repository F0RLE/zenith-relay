import type { ReactNode } from "react";
import { History } from "lucide-react";
import { useTranslation } from "react-i18next";
import { Button, Dialog, EmptyState } from "../../../components/Ui";

type RecoverySurfaceProps = {
  className?: string;
  isEmpty: boolean;
  title: string;
  hint: string;
  notice?: ReactNode;
  children: ReactNode;
};

export function RecoverySurface({ className = "", isEmpty, title, hint, notice, children }: RecoverySurfaceProps) {
  return <section className={`profile-recovery${isEmpty ? " is-empty" : ""}${className ? ` ${className}` : ""}`}>
    {notice}
    <section className="profile-recovery-section profile-named-section">
      <header className="profile-recovery-section-heading"><span><History aria-hidden /></span><div><h2>{title}</h2><small>{hint}</small></div></header>
      {children}
    </section>
  </section>;
}

export function RecoveryEmptyState({ title, description }: { title: string; description: string }) {
  return <div className="profile-recovery-empty-state"><EmptyState title={title} description={description} /></div>;
}

export type RecoverySnapshotRow = {
  id: string;
  name: string;
  detail?: string;
  createdAt: string;
  contents: ReactNode;
  actions: ReactNode;
};

export function RecoverySnapshotTable({ rows }: { rows: RecoverySnapshotRow[] }) {
  const { t } = useTranslation();
  return <div className="relay-table-wrap profile-snapshot-table-wrap"><table className="relay-table profile-snapshot-table"><thead><tr><th>{t("common.name")}</th><th>{t("profiles.created")}</th><th>{t("profiles.contents")}</th><th><span className="sr-only">{t("common.actions")}</span></th></tr></thead><tbody>{rows.map((row) => <tr key={row.id}><td><strong>{row.name}</strong>{row.detail ? <small>{row.detail}</small> : null}</td><td>{row.createdAt}</td><td>{row.contents}</td><td><div className="inline-actions">{row.actions}</div></td></tr>)}</tbody></table></div>;
}

type RecoveryConfirmationDialogProps = {
  title: string;
  confirmation: ReactNode;
  hint?: ReactNode;
  busy: boolean;
  onCancel: () => void;
  onConfirm: () => void;
};

export function RecoveryConfirmationDialog({ title, confirmation, hint, busy, onCancel, onConfirm }: RecoveryConfirmationDialogProps) {
  const { t } = useTranslation();
  return <Dialog title={title} onClose={() => { if (!busy) onCancel(); }} footer={<><Button variant="secondary" disabled={busy} onClick={onCancel}>{t("common.no")}</Button><Button variant="danger" busy={busy} disabled={busy} onClick={onConfirm}>{t("common.yes")}</Button></>}><p className="confirm-dialog-message">{confirmation}</p>{hint ? <p className="confirm-dialog-message">{hint}</p> : null}</Dialog>;
}
