import { useState } from "react";
import { ExternalLink, Server } from "lucide-react";
import { useTranslation } from "react-i18next";
import { openDemo } from "../../api/hostedDevice";
import type { HostedDeviceReview } from "../../api/types/hostedDevice";
import { Button, CopyButton, PageHeader } from "../../components/Ui";
import { AppearanceSettings, SettingsGroup } from "../settings/AppearanceSettings";
import { useHostedFormatting } from "./HostedObservations";
import { hostedErrorKey } from "./hostedErrors";

export function HostedSettings({ device, expires, error, action, onPrepare, onCheck, onForget, onRetry, onConnections }: {
  device: HostedDeviceReview | null | undefined; expires: number | undefined; error: unknown;
  action: "prepare" | "check" | "forget" | undefined; onPrepare: (invitation: string) => void;
  onCheck: () => void; onForget: () => void; onRetry: () => void; onConnections: () => void;
}) {
  const { t } = useTranslation();
  const { date } = useHostedFormatting();
  const [invitation, setInvitation] = useState("");
  const [confirm, setConfirm] = useState(false);
  const [opening, setOpening] = useState(false);
  const [previewFailed, setPreviewFailed] = useState(false);

  async function openPreview() {
    setOpening(true); setPreviewFailed(false);
    try { await openDemo(); }
    catch { setPreviewFailed(true); }
    finally { setOpening(false); }
  }

  return <section className="relay-page relay-workspace-page settings-page">
    <PageHeader workspace title={t("nav.settings")} />
    <div className="settings-groups">
      <SettingsGroup icon={<Server aria-hidden />} title={t("hosted.server")}>
        <div className="hosted-device-pane hosted-settings-pane">
          {device === undefined && !error && <p role="status">{t("common.loading")}</p>}
          {device === null && <form onSubmit={e => { e.preventDefault(); if (!action && invitation.trim()) onPrepare(invitation.trim()); }}>
            <label className="relay-field"><span>{t("hosted.invitation")}</span>
              <input value={invitation} onChange={e => setInvitation(e.target.value)} maxLength={512} autoComplete="off" spellCheck={false} /></label>
            <Button type="submit" variant="primary" busy={action === "prepare"} disabled={!invitation.trim()}>{t("hosted.prepare")}</Button>
          </form>}
          {device && <>
            <div className="hosted-device-target"><strong>{device.destination.origin}</strong><code>{device.destination.installation}</code>
              {expires && <small role="status">{t("hosted.expires", { date: date(expires) })}</small>}
            </div>
            {!expires && <label className="relay-field"><span>{t("hosted.review")}</span>
              <input readOnly value={device.approval_code} onFocus={e => e.currentTarget.select()} />
            </label>}
            <div className="hosted-device-actions">
              {!expires && <CopyButton value={device.approval_code} label={t("common.copy")} />}
              <Button variant={expires ? "secondary" : "primary"} busy={action === "check"} disabled={!!action}
                onClick={() => { setConfirm(false); onCheck(); }}>{t("hosted.read")}</Button>
              <Button variant={confirm ? "danger" : "ghost"} busy={action === "forget"} disabled={!!action} onClick={() => {
                if (confirm) { setConfirm(false); setInvitation(""); onForget(); }
                else setConfirm(true);
              }}>{t(confirm ? "hosted.confirmForget" : "hosted.forget")}</Button>
              {confirm && <Button disabled={!!action} onClick={() => setConfirm(false)}>{t("common.cancel")}</Button>}
            </div>
          </>}
          {!!error && <p role="alert" className="error-text">{t(hostedErrorKey(error))}</p>}
          {device === undefined && !!error && <Button onClick={onRetry}>{t("common.retry")}</Button>}
          {error === "copy_review_pending" && <Button onClick={onConnections}>{t("nav.connections")}</Button>}
          {import.meta.env.DEV && <>
            <Button variant="ghost" icon={<ExternalLink aria-hidden />} busy={opening} onClick={() => void openPreview()}>{t("hosted.openPreview")}</Button>
            {previewFailed && <p role="alert">{t("hosted.previewUnavailable")} <code>cd hosted/web; bun run dev</code></p>}
          </>}
        </div>
      </SettingsGroup>
      <AppearanceSettings />
    </div>
  </section>;
}
