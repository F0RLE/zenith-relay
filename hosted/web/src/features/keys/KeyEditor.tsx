import { useEffect, useRef, useState } from "react";
import { DatePicker } from "../../components/DatePicker";
import type { KeyRecord } from "./KeysPage";
import { useLocale } from "../../i18n/LocaleProvider";

export type KeyDraft = Pick<KeyRecord, "name" | "dailyLimit" | "expires">;

export function KeyEditor({ mode, initial, onSave, onCancel }: {
  mode: "create" | "edit"; initial?: KeyRecord;
  onSave: (draft: KeyDraft) => void; onCancel: () => void;
}) {
  const { t } = useLocale();
  const [name, setName] = useState(initial?.name ?? "");
  const [limit, setLimit] = useState(initial?.dailyLimit == null ? "" : String(initial.dailyLimit));
  const [expires, setExpires] = useState(initial?.expires ?? "");
  const nameInput = useRef<HTMLInputElement>(null);
  const label = t(mode === "create" ? "keys.createKey" : "keys.keySettings");
  useEffect(() => { nameInput.current?.focus(); }, []);

  return <form className="key-editor" aria-label={label} onKeyDown={event => {
    if (event.key === "Escape" && !event.defaultPrevented) { event.preventDefault(); onCancel(); }
  }} onSubmit={event => {
    event.preventDefault();
    if (!name.trim()) return;
    onSave({ name: name.trim(), dailyLimit: limit ? Number(limit) : null, expires: expires || null });
  }}>
    {mode === "create" && <h2>{t("keys.newKey")}</h2>}
    <div className="key-editor-fields">
      <label>{t("keys.name")}<input ref={nameInput} required maxLength={80} autoComplete="off" value={name} onChange={event => setName(event.target.value)} /></label>
      <label>{t("keys.dailyLimitUsd")}<input type="number" inputMode="numeric" min="1" max="10000" step="1" placeholder={t("keys.workspacePolicy")} value={limit} onChange={event => setLimit(event.target.value)} /></label>
      <DatePicker label={t("keys.validUntil")} value={expires} onChange={setExpires} />
    </div>
    <div className="key-editor-actions"><button className="button" type="button" onClick={onCancel}>{t("common.cancel")}</button><button className="button primary-button" type="submit" disabled={!name.trim()}>{t(mode === "edit" ? "common.save" : "keys.createKey")}</button></div>
  </form>;
}
