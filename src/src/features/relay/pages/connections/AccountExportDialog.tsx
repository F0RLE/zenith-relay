import { type ChangeEvent, useRef, useState } from "react";
import { Check, Copy, Download, Eye, Pencil, Upload } from "lucide-react";
import { useTranslation } from "react-i18next";
import { relayCommands } from "../../api/commands";
import type { AccountExportFormat } from "../../api/types";
import { Button, Dialog, IconButton, copyText } from "../../components/Ui";
import { useRelayState } from "../../state/RelayStateProvider";
import { MarkdownPreview } from "./MarkdownPreview";

const MAX_ZENITH_EXPORT_DESCRIPTION_LENGTH = 2_000;
const accountExportFormats: Array<{ value: AccountExportFormat; label: string; multiple: boolean }> = [
  { value: "zenith", label: "Zenith", multiple: true },
  { value: "sub2api", label: "sub2api", multiple: true },
  { value: "cpa", label: "CPA", multiple: false },
  { value: "cockpit", label: "Cockpit Tools", multiple: true },
  { value: "9router", label: "9router", multiple: true },
  { value: "codex", label: "ChatGPT", multiple: false },
  { value: "axon_hub", label: "AxonHub", multiple: false },
  { value: "codex_manager", label: "Codex-Manager", multiple: true },
];
export function AccountExportDialog({ accountIds, onClose }: { accountIds: string[]; onClose: () => void }) {
  const { t } = useTranslation();
  const { mode, perform, busy } = useRelayState();
  const [format, setFormat] = useState<AccountExportFormat>("zenith");
  const [description, setDescription] = useState("");
  const [descriptionMode, setDescriptionMode] = useState<"edit" | "preview">("edit");
  const [descriptionError, setDescriptionError] = useState<string | null>(null);
  const markdownFileInput = useRef<HTMLInputElement>(null);
  const formats = accountExportFormats.filter((option) => accountIds.length === 1 || option.multiple);
  const selectedFormat = formats.find((option) => option.value === format) ?? formats[0];
  const loadMarkdown = async (event: ChangeEvent<HTMLInputElement>) => {
    const input = event.currentTarget;
    const file = input.files?.[0];
    input.value = "";
    if (!file) return;
    try {
      const content = (await file.text()).replace(/\r\n?/g, "\n");
      if (content.length > MAX_ZENITH_EXPORT_DESCRIPTION_LENGTH) {
        setDescriptionError(t("accounts.exportDescriptionTooLong", { max: MAX_ZENITH_EXPORT_DESCRIPTION_LENGTH }));
        return;
      }
      setDescription(content);
      setDescriptionError(null);
      setDescriptionMode("preview");
    } catch {
      setDescriptionError(t("accounts.exportDescriptionReadFailed"));
    }
  };
  const run = async (destination: "copy" | "download") => {
    const ok = await perform(`account-export-${destination}`, async () => {
      const input = {
        accountIds,
        format,
        destination,
        ...(format === "zenith" && description.trim() ? { description } : {}),
      } as const;
      const result = mode === "local"
        ? await relayCommands.exportLocalAccounts(input)
        : await relayCommands.exportRemoteAccounts(input);
      if (destination === "copy") {
        if (!result.content) throw new Error("account export content is missing");
        await copyText(result.content);
      }
    }, destination === "copy" ? "feedback.accountExportCopied" : "feedback.accountExportDownloaded");
    if (ok) onClose();
  };
  return <Dialog title={t("accounts.exportTitle")} onClose={onClose} footer={<><Button variant="secondary" onClick={onClose}>{t("common.cancel")}</Button><Button variant="secondary" icon={<Copy aria-hidden />} busy={busy === "account-export-copy"} onClick={() => run("copy")}>{t("accounts.copyExport")}</Button><Button variant="primary" icon={<Download aria-hidden />} busy={busy === "account-export-download"} onClick={() => run("download")}>{t("accounts.downloadExport")}</Button></>}>
    <div className="relay-form account-export-form">
      <div className="account-export-heading"><span>{t("accounts.exportFormat")}</span><strong>{t("accounts.exportCount", { count: accountIds.length })}</strong></div>
      <div className="account-export-formats" data-count={formats.length} role="radiogroup" aria-label={t("accounts.exportFormat")}>{formats.map((option) => <button type="button" role="radio" data-value={option.value} aria-checked={format === option.value} key={option.value} onClick={() => setFormat(option.value)}><span>{option.label}</span>{format === option.value ? <Check aria-hidden /> : null}</button>)}</div>
      <p className="account-export-description">{t(`accounts.exportFormats.${selectedFormat.value}`)}</p>
      {format === "zenith" ? <div className="relay-field account-export-description-field">
        <div className="account-export-description-toolbar">
          <label htmlFor="zenith-export-description">{t("accounts.exportDescription")}</label>
          <div className="account-export-description-controls">
            <input ref={markdownFileInput} type="file" accept=".md,text/markdown" onChange={(event) => void loadMarkdown(event)} />
            <IconButton label={t("accounts.loadMarkdown")} icon={<Upload aria-hidden />} onClick={() => markdownFileInput.current?.click()} />
            <div className="markdown-mode-switch" role="group" aria-label={t("accounts.descriptionMode")}>
              <button type="button" aria-pressed={descriptionMode === "edit"} onClick={() => setDescriptionMode("edit")}><Pencil aria-hidden />{t("accounts.descriptionEdit")}</button>
              <button type="button" aria-pressed={descriptionMode === "preview"} onClick={() => setDescriptionMode("preview")}><Eye aria-hidden />{t("accounts.descriptionPreview")}</button>
            </div>
          </div>
        </div>
        {descriptionMode === "edit"
          ? <textarea id="zenith-export-description" value={description} maxLength={MAX_ZENITH_EXPORT_DESCRIPTION_LENGTH} placeholder={t("accounts.exportDescriptionPlaceholder")} onChange={(event) => { setDescription(event.target.value); setDescriptionError(null); }} />
          : <div className="account-export-markdown-preview" role="region" aria-label={t("accounts.descriptionPreview")}>{description.trim() ? <MarkdownPreview content={description} /> : <p>{t("accounts.descriptionPreviewEmpty")}</p>}</div>}
        {descriptionError ? <p className="form-note error-text" role="alert">{descriptionError}</p> : null}
        <small>{t("accounts.exportDescriptionHint", { count: description.length, max: MAX_ZENITH_EXPORT_DESCRIPTION_LENGTH })}</small>
      </div> : null}
    </div>
  </Dialog>;
}
