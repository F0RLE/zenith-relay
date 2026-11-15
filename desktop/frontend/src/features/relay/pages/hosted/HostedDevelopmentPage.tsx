import { useState } from "react";
import { ExternalLink } from "lucide-react";
import { useTranslation } from "react-i18next";
import { relayCommands } from "../../api/commands";
import { Button, PageHeader } from "../../components/Ui";

export function HostedDevelopmentPage() {
  const { t } = useTranslation();
  const [opening, setOpening] = useState(false);
  const [failed, setFailed] = useState(false);
  async function openPreview() {
    if (opening) return;
    setOpening(true); setFailed(false);
    try { await relayCommands.openHostedPreview(); }
    catch { setFailed(true); }
    finally { setOpening(false); }
  }
  return <section className="relay-page">
    <PageHeader title={t("modes.hosted")} />
    <div className="relay-empty">
      <strong>{t("hosted.inDevelopment")}</strong>
      {import.meta.env.DEV && <>
        <Button icon={<ExternalLink aria-hidden />} disabled={opening} onClick={() => { void openPreview(); }}>{t("hosted.openPreview")}</Button>
        {failed && <p role="alert">{t("hosted.previewUnavailable")} <code>cd web; bun run dev</code></p>}
      </>}
    </div>
  </section>;
}
