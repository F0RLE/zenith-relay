import { useEffect, useRef, useState } from "react";
import type { MouseEvent } from "react";
import { ChevronDown, ChevronUp, Plus } from "../../components/Icons";
import { CopyButton } from "../../components/CopyButton";
import { OptionMenu } from "../../components/OptionMenu";
import type { CustomerModel } from "../models/ModelsPage";
import { KeyEditor } from "./KeyEditor";
import type { KeyDraft } from "./KeyEditor";
import { useLocale } from "../../i18n/LocaleProvider";
import type { MessageKey } from "../../i18n/types";
import { displayCalendarDate } from "../../components/calendar-date";
import "./keys.css";

export interface KeyRecord {
  id: string; name: string; mask: string; models: string[];
  dailyLimit: number | null; expires: string | null; revoked: boolean;
}
type KeyAction = { kind: "edit"; key: KeyRecord } | { kind: "create" };
type KeyConfirmation = { kind: "reissue" | "revoke"; keyId: string };
type SortField = "name" | "revoked" | "models" | "dailyLimit" | "expires";
type KeySort = { field: SortField; direction: "ascending" | "descending" };
const columns: { label: MessageKey; field?: SortField; className: string }[] = [
  { label: "common.name", field: "name", className: "key-name-column" },
  { label: "common.apiKey", className: "key-token-column" },
  { label: "common.status", field: "revoked", className: "key-status-column" },
  { label: "nav.models", field: "models", className: "key-models-column" },
  { label: "keys.dailyLimit", field: "dailyLimit", className: "key-limit-column" },
  { label: "keys.expiration", field: "expires", className: "key-expiry-column" },
  { label: "common.actions", className: "key-actions-column" },
];

export function KeysPage({ keys, models, onChange }: {
  keys: KeyRecord[]; models: CustomerModel[];
  onChange?: (keys: KeyRecord[]) => void;
}) {
  const { t, locale } = useLocale();
  const [action, setAction] = useState<KeyAction | null>(null);
  const [confirming, setConfirming] = useState<KeyConfirmation | null>(null);
  const [result, setResult] = useState<{ secret: string; keyId: string } | null>(null);
  const [sort, setSort] = useState<KeySort | null>(null);
  const opener = useRef<HTMLButtonElement | null>(null);
  const confirmation = useRef<HTMLDivElement | null>(null);
  const reveal = useRef<HTMLElement | null>(null);
  const rows = useRef(new Map<string, HTMLElement>());

  const modelNames = (key: KeyRecord) => key.models.map(id => models.find(model => model.id === id)?.name ?? id).join(", ");
  const orderedKeys = sort ? [...keys].sort((a, b) => {
    const value = (key: KeyRecord) => sort.field === "models" ? modelNames(key) : key[sort.field];
    const left = value(a), right = value(b);
    // Inherited policy has no comparable numeric/date value; keep it last in either direction.
    if (left === null || right === null) return left === right ? 0 : left === null ? 1 : -1;
    const difference = typeof left === "string" && typeof right === "string"
      ? left.localeCompare(right, locale, { numeric: true }) : Number(left) - Number(right);
    return sort.direction === "ascending" ? difference : -difference;
  }) : keys;

  function sortBy(field: SortField) {
    setSort(current => ({ field, direction: current?.field === field && current.direction === "ascending" ? "descending" : "ascending" }));
    setConfirming(null);
  }

  function focusRow(id: string, selector = "h2") {
    requestAnimationFrame(() => rows.current.get(id)?.querySelector<HTMLElement>(selector)?.focus());
  }
  function cancelEditor() {
    setAction(null);
    requestAnimationFrame(() => opener.current?.isConnected && opener.current.focus());
  }
  function open(next: KeyAction, trigger: HTMLButtonElement) {
    opener.current = trigger;
    setConfirming(null);
    setAction(next);
  }
  useEffect(() => {
    if (!confirming) return;
    confirmation.current?.querySelector<HTMLButtonElement>(".key-confirm-action")?.focus();
    const dismissOutside = (event: Event) => {
      if (!confirmation.current?.contains(event.target as Node)) {
        setConfirming(current => current === confirming ? null : current);
      }
    };
    const dismissEscape = (event: KeyboardEvent) => {
      if (event.key !== "Escape") return;
      event.preventDefault();
      setConfirming(null);
      focusRow(confirming.keyId, `[data-key-action="${confirming.kind}"]`);
    };
    // Dismiss on click capture, after pointerup: collapsing a row during
    // pointerdown can move the next row away before its button receives click.
    document.addEventListener("click", dismissOutside, true);
    document.addEventListener("keydown", dismissEscape);
    return () => {
      document.removeEventListener("click", dismissOutside, true);
      document.removeEventListener("keydown", dismissEscape);
    };
  }, [confirming]);
  useEffect(() => {
    if (result) reveal.current?.querySelector<HTMLButtonElement>("button")?.focus();
  }, [result]);

  function issue(draft: KeyDraft, scope: string[]) {
    if (!onChange || result) return;
    const id = crypto.randomUUID();
    const secret = `demo_not_a_real_key_${id}`;
    const key: KeyRecord = { ...draft, id, mask: `demo_…${id.slice(-4)}`, models: [...scope], revoked: false };
    onChange([...keys, key]);
    setResult({ secret, keyId: id });
    setAction(null);
    setConfirming(null);
  }
  function save(draft: KeyDraft) {
    if (!onChange || !action) return;
    if (action.kind === "edit") {
      onChange(keys.map(key => key.id === action.key.id ? { ...key, ...draft } : key));
      cancelEditor();
    } else {
      issue(draft, models.map(model => model.id));
    }
  }
  function confirmAction(event: MouseEvent<HTMLButtonElement>, key: KeyRecord, kind: KeyConfirmation["kind"]) {
    if (!onChange || key.revoked || event.detail > 1 || (kind === "reissue" && result)) return;
    if (confirming?.keyId !== key.id || confirming.kind !== kind) {
      setAction(null);
      setConfirming({ kind, keyId: key.id });
      return;
    }
    if (kind === "reissue") {
      issue({ name: t("keys.replacementName", { name: key.name }), dailyLimit: key.dailyLimit, expires: key.expires }, key.models);
      return;
    }
    onChange(keys.map(item => item.id === key.id ? { ...item, revoked: true } : item));
    setConfirming(null);
    if (result?.keyId === key.id) setResult(null);
    focusRow(key.id);
  }
  return <section className="api-keys-page">
    <header className="page-heading"><h1>{t("nav.keys")}</h1><button className="button primary-button" type="button" disabled={!onChange || !!result || action?.kind === "create"} title={result ? t("keys.saveTheNewlyCreatedKeyFirst") : undefined} onClick={event => open({ kind: "create" }, event.currentTarget)}><Plus aria-hidden="true" />{t("keys.createKey")}</button></header>
    {result && <section className="key-reveal" aria-label={t("keys.keyCreated")} ref={reveal}>
      <h2>{t("keys.keyCreated")}</h2><p>{t("keys.secretWarning")}</p>
      <code>{result.secret}</code>
      <div className="key-reveal-actions"><CopyButton text={result.secret} label={t("keys.copyKey")} /><button className="button" type="button" onClick={() => { const id = result.keyId; setResult(null); focusRow(id); }}>{t("common.done")}</button></div>
    </section>}
    {action?.kind === "create" && <KeyEditor key="create" mode="create" onSave={save} onCancel={cancelEditor} />}
    {!onChange ? <p className="empty-state" role="status">{t("keys.unavailable")}</p> : !keys.length ? <p className="empty-state">{t("keys.empty")}</p> : <div className="keys-list table-shell">
      <div className="keys-mobile-sort"><OptionMenu label={t("keys.sortKeys")} value={sort ? `${sort.field}:${sort.direction}` : "original"} onChange={value => {
        const [field, direction] = value.split(":");
        setSort(field === "original" ? null : { field: field as SortField, direction: direction as KeySort["direction"] });
        setConfirming(null);
      }} options={[
        { value: "original", label: t("keys.originalOrder") },
        ...columns.filter(column => column.field).flatMap(column => [
          { value: `${column.field}:ascending`, label: t("keys.sortAscendingBy", { column: t(column.label) }) },
          { value: `${column.field}:descending`, label: t("keys.sortDescendingBy", { column: t(column.label) }) },
        ]),
      ]} /></div>
      <table className="keys-table" aria-label={t("nav.keys")}>
        <colgroup>{columns.map(column => <col key={column.label} className={column.className} />)}</colgroup>
        <thead><tr>{columns.map(column => <th key={column.label} scope="col" aria-sort={column.field ? sort?.field === column.field ? sort.direction : "none" : undefined}>
          {column.field ? <button className="key-sort" type="button" onClick={() => sortBy(column.field!)} data-relay-tooltip={t(sort?.field === column.field && sort.direction === "ascending" ? "keys.sortDescending" : "keys.sortAscending")} data-relay-tooltip-placement="top">
            {t(column.label)}{sort?.field === column.field && sort.direction === "ascending" ? <ChevronUp /> : <ChevronDown />}
          </button> : t(column.label)}
        </th>)}</tr></thead>
        {orderedKeys.map(key => {
      const pending = confirming?.keyId === key.id ? confirming : null;
      const editor = action && "key" in action && action.key.id === key.id ? action : null;
      const names = modelNames(key);
      return <tbody key={key.id}>
        <tr className="key-row" data-revoked={key.revoked} aria-labelledby={`key-title-${key.id}`} ref={element => { if (element) rows.current.set(key.id, element); else rows.current.delete(key.id); }}>
        <td className="key-identity"><h2 id={`key-title-${key.id}`} tabIndex={-1} title={key.name}>{key.name}</h2></td>
        <td className="key-token"><code>{key.mask}</code></td>
        <td className="key-status"><span className="status-badge" data-tone={key.revoked ? "muted" : "success"}>{t(key.revoked ? "status.revoked" : "status.active")}</span></td>
        <td className="key-policy-cell key-model-cell"><span className="key-cell-label">{t("nav.models")}</span><span className="key-cell-value" title={names}>{names}</span></td>
        <td className="key-policy-cell key-limit-cell"><span className="key-cell-label">{t("keys.dailyLimit")}</span><span className="key-cell-value" title={key.dailyLimit === null ? undefined : t("keys.resetsAt0000Utc")}>{key.dailyLimit === null ? t("keys.workspacePolicy") : `${key.dailyLimit.toLocaleString(locale)} USD`}</span></td>
        <td className="key-policy-cell key-expiry-cell"><span className="key-cell-label">{t("keys.expiration")}</span><span className="key-cell-value">{key.expires ? displayCalendarDate(key.expires, locale) : t("keys.workspacePolicy")}</span></td>
        <td className="key-actions-cell">{!key.revoked && <div className={`key-actions${pending ? " key-actions-pending" : ""}`} ref={pending ? confirmation : undefined}>
          {pending ? <>
            <button className="button key-action" type="button" onClick={() => { setConfirming(null); focusRow(key.id, `[data-key-action="${pending.kind}"]`); }}>{t("common.cancel")}</button>
            <button className={`button key-confirm-action${pending.kind === "revoke" ? " danger-button key-confirm-revoke" : " key-confirm-reissue"}`} type="button" onKeyDown={event => { if (event.repeat && ["Enter", " "].includes(event.key)) event.preventDefault(); }} onClick={event => confirmAction(event, key, pending.kind)}>{t(pending.kind === "revoke" ? "keys.yesRevoke" : "keys.yesReissue")}</button>
          </> : <>
            <button className="button key-action" type="button" aria-expanded={editor?.kind === "edit"} onClick={event => editor?.kind === "edit" ? cancelEditor() : open({ kind: "edit", key }, event.currentTarget)}>{t("common.edit")}</button>
            <button className="button key-action" type="button" data-key-action="reissue" disabled={!!result} title={result ? t("keys.saveTheNewlyCreatedKeyFirst") : undefined} onClick={event => confirmAction(event, key, "reissue")}>{t("keys.reissue")}</button>
            <button className="button danger-button key-action" type="button" data-key-action="revoke" onClick={event => confirmAction(event, key, "revoke")}>{t("keys.revoke")}</button>
          </>}
        </div>}</td>
        </tr>
        {editor && <tr className="key-editor-row"><td colSpan={columns.length}><KeyEditor key={`${editor.kind}-${key.id}`} mode={editor.kind} initial={key} onSave={save} onCancel={cancelEditor} /></td></tr>}
      </tbody>;
    })}</table></div>}
  </section>;
}
