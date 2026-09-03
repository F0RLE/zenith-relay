import { Download } from "lucide-react";
import { useTranslation } from "react-i18next";
import type { AppUpdate } from "../../../platform/desktop";
import { Button, Dialog } from "../components/Ui";
import type { UpdateInstallError, UpdateProgress } from "../hooks/useAppUpdates";

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
  const notes = localizeReleaseNotes(update.body, i18n.language);
  return <Dialog title={t("updates.title", { version: update.version })} onClose={onClose} footer={<div className="update-actions"><Button variant="secondary" disabled={installing} onClick={onSkip}>{t("updates.skipVersion", { version: update.version })}</Button><Button variant="primary" icon={<Download aria-hidden />} busy={installing} onClick={onInstall}>{t("updates.install")}</Button></div>}>
    <div className="update-release"><div><span>{t("updates.versionChange", { current: update.currentVersion, next: update.version })}</span>{date ? <small>{date}</small> : null}</div></div>
    <section className="update-notes"><h3>{t("updates.changelog")}</h3><p>{notes || t("updates.noChangelog")}</p></section>
    {installing ? <div className="update-progress" role="status"><div><strong>{t("updates.downloading")}</strong><span>{percent === null ? t("updates.preparing") : `${percent}%`}</span></div><progress max={100} value={percent ?? undefined} /></div> : null}
    {installError ? <p className="warning-box" role="alert">{t(installError === "write" ? "updates.portableWriteFailed" : "updates.installFailed")}</p> : null}
  </Dialog>;
}

function localizeReleaseNotes(body: string | undefined, language: string) {
  if (!body?.trim()) return "";
  const markers = [...body.matchAll(/<!--\s*relay-notes:([a-z0-9-]+)\s*-->/gi)];
  if (!markers.length) return body.trim();
  const sections = new Map<string, string>();
  markers.forEach((marker, index) => {
    const markerLocale = marker[1];
    if (!markerLocale) return;
    sections.set(
      markerLocale.toLowerCase(),
      body.slice((marker.index ?? 0) + marker[0].length, markers[index + 1]?.index).trim(),
    );
  });
  const locale = language.toLowerCase();
  const baseLocale = locale.split("-")[0] ?? locale;
  return sections.get(locale) ?? sections.get(baseLocale) ?? sections.get("en") ?? sections.values().next().value ?? "";
}
