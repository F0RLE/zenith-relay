import { Download } from "lucide-react";
import { useTranslation } from "react-i18next";
import type { AppUpdate } from "../../../platform/desktop";
import { MarkdownPreview } from "../components/MarkdownPreview";
import { Button, Dialog } from "../components/Ui";
import type { UpdateInstallError, UpdateProgress } from "../hooks/useAppUpdates";
import { prepareReleaseNotes } from "./updateReleaseNotes";

type UpdateDialogProps = {
  update: AppUpdate;
  installing: boolean;
  progress: UpdateProgress | null;
  installError: UpdateInstallError;
  onInstall: () => void;
  onSkip: () => void;
  onClose: () => void;
};

export function UpdateDialog({ update, installing, progress, installError, onInstall, onSkip, onClose }: UpdateDialogProps) {
  const { i18n, t } = useTranslation();
  const percent = progress?.total ? Math.min(100, Math.round(progress.downloaded / progress.total * 100)) : null;
  const date = update.date ? new Intl.DateTimeFormat(i18n.language, { dateStyle: "long" }).format(new Date(update.date)) : null;
  const notes = prepareReleaseNotes(update.body, i18n.language, update.version);
  return <Dialog className="update-dialog" title={t("updates.title", { version: update.version })} onClose={onClose} footer={<div className="update-actions"><Button variant="secondary" disabled={installing} onClick={onSkip}>{t("updates.skipVersion", { version: update.version })}</Button><Button variant="primary" icon={<Download aria-hidden />} busy={installing} onClick={onInstall}>{t("updates.install")}</Button></div>}>
    <div className="update-release"><span className="update-release-icon"><Download aria-hidden /></span><div><strong>{t("updates.versionChange", { current: update.currentVersion, next: update.version })}</strong>{date ? <small>{date}</small> : null}</div></div>
    <section className="update-notes"><h3>{t("updates.changelog")}</h3><MarkdownPreview content={notes || t("updates.noChangelog")} /></section>
    {installing ? <div className="update-progress" role="status"><div><strong>{t("updates.downloading")}</strong><span>{percent === null ? t("updates.preparing") : `${percent}%`}</span></div><progress max={100} value={percent ?? undefined} /></div> : null}
    {installError ? <p className="warning-box" role="alert">{t(installError === "write" ? "updates.portableWriteFailed" : "updates.installFailed")}</p> : null}
  </Dialog>;
}
