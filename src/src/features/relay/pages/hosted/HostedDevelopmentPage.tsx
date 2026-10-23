import { useState } from "react";
import { ExternalLink, Laptop } from "lucide-react";
import { useTranslation } from "react-i18next";
import { relayCommands } from "../../api/commands";
import { Button, EmptyState, PageHeader } from "../../components/Ui";

export function HostedDevelopmentPage({ onComputer }: { onComputer: () => void }) {
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
    <EmptyState title={t("hosted.inDevelopment")} description={t("hosted.description")} action={<>
      <Button variant="primary" icon={<Laptop aria-hidden />} onClick={onComputer}>{t("hosted.useComputer")}</Button>
      {import.meta.env.DEV && <>
        <Button icon={<ExternalLink aria-hidden />} disabled={opening} onClick={() => { void openPreview(); }}>{t("hosted.openPreview")}</Button>
        <p className="form-note">{t("hosted.previewHint")}</p>
        {failed && <p role="alert">{t("hosted.previewUnavailable")} <code>cd web; bun run dev</code></p>}
      </>}
    </>} />
  </section>;
}
