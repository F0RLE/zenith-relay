import { useEffect, useId, useState } from "react";
import { Cable, Gauge, Plus, RefreshCw, Search, Trash, Users, X } from "../../components/Icons";
import { Dialog } from "../../components/Dialog";
import { useLocale } from "../../i18n/LocaleProvider";
import type { OwnerConnection } from "./model";
import { ResourceMenu } from "./OwnerResourceCard";
import { ResourceSelection } from "./ResourceSelection";
import { assignProxyAccounts, parseProxyImport, safeProxyEndpoint } from "./proxyPresentation";
import type { OwnerProxy } from "./proxyPresentation";
import "./owner-proxies.css";

export function OwnerProxies({ entries, accounts, onChange, active, importOpen, onImportClose, onImport }: {
  entries: OwnerProxy[]; accounts: OwnerConnection[]; onChange: ((entries: OwnerProxy[]) => void) | undefined;
  active: boolean; importOpen: boolean; onImportClose: () => void; onImport: () => void;
}) {
  const { t, locale } = useLocale();
  const [query, setQuery] = useState("");
  const [selected, setSelected] = useState<string[]>([]);
  const [renaming, setRenaming] = useState<OwnerProxy | null>(null);
  const [managing, setManaging] = useState<OwnerProxy | null>(null);
  const [deleting, setDeleting] = useState<string[]>([]);
  const [label, setLabel] = useState("");
  useEffect(() => { if (!active) { setRenaming(null); setManaging(null); setDeleting([]); setSelected([]); onImportClose(); } }, [active]);
  const visible = entries.filter(entry => `${entry.label ?? ""} ${safeProxyEndpoint(entry.endpoint) ?? ""}`.toLocaleLowerCase(locale).includes(query.trim().toLocaleLowerCase(locale)));
  const selectedVisible = visible.filter(entry => selected.includes(entry.id));
  const assigned = entries.filter(entry => entry.assignedAccountIds.length > 0).length;
  function remove() {
    onChange?.(entries.filter(entry => !deleting.includes(entry.id)));
    setSelected(current => current.filter(id => !deleting.includes(id)));
    setDeleting([]);
  }
  return <section className="owner-proxies" aria-label={t("ownerProxies.title")}>
    {entries.length > 0 && <div className="owner-proxy-controls">
      <dl className="owner-proxy-summary">
        {([["ownerProxies.total", entries.length], ["ownerProxies.free", entries.length - assigned], ["ownerProxies.assigned", assigned]] as const).map(([key, value]) => <div key={key}><dt>{t(key)}</dt><dd>{value.toLocaleString(locale)}</dd></div>)}
      </dl>
      <div className="owner-proxy-toolbar">
        <ResourceSelection label={t("ownerProxies.selectAll")} checked={visible.length > 0 && selectedVisible.length === visible.length} mixed={selectedVisible.length > 0 && selectedVisible.length < visible.length} disabled={!visible.length || !onChange} onChange={() => setSelected(current => selectedVisible.length === visible.length ? current.filter(id => !visible.some(entry => entry.id === id)) : [...new Set([...current, ...visible.map(entry => entry.id)])])} />
        {selected.length ? <><span role="status">{t("ownerProxies.selectedCount", { count: selected.length.toLocaleString(locale) })}</span><div className="owner-proxy-toolbar-actions"><button type="button" className="relay-icon-button danger-button" aria-label={t("ownerProxies.deleteSelected")} data-relay-tooltip={t("ownerProxies.deleteSelected")} onClick={() => setDeleting(selected)}><Trash aria-hidden="true" /></button><button type="button" className="relay-icon-button" aria-label={t("ownerProxies.clearSelection")} data-relay-tooltip={t("ownerProxies.clearSelection")} onClick={() => setSelected([])}><X aria-hidden="true" /></button></div></> : <><label className="owner-proxy-search"><Search aria-hidden="true" /><input type="search" aria-label={t("ownerProxies.search")} placeholder={t("ownerProxies.search")} value={query} onChange={event => setQuery(event.target.value)} /></label><span className="owner-disabled-action" tabIndex={0} data-relay-tooltip={t("ownerProxies.checkUnavailable")}><button type="button" className="relay-icon-button" aria-label={t("connections.refreshConnections")} disabled><RefreshCw aria-hidden="true" /></button></span></>}
      </div>
    </div>}
    {!entries.length ? <div className="owner-proxy-empty"><Cable aria-hidden="true" /><h2>{t("ownerProxies.empty")}</h2><button type="button" className="relay-button primary" disabled={!onChange} onClick={onImport}><Plus aria-hidden="true" />{t("ownerProxies.import")}</button></div> : !visible.length ? <p className="owner-resource-empty" role="status">{t("ownerProxies.noResults")}</p> : <div className="owner-proxy-list">
      {visible.map(entry => {
        const endpoint = safeProxyEndpoint(entry.endpoint) ?? t("ownerProxies.invalidEndpoint");
        const name = entry.label || endpoint;
        const check = entry.lastCheck;
        const success = Boolean(check?.ip && !check.errorCode);
        const status = t(success ? "ownerProxies.available" : check ? "ownerProxies.failed" : "ownerProxies.notChecked");
        const country = success && check?.countryCode ? (() => { try { return new Intl.DisplayNames([locale], { type: "region" }).of(check.countryCode); } catch { return check.countryCode; } })() : null;
        const location = [...new Set([country, success ? check?.city || check?.region : null].filter(Boolean))].join(" · ");
        return <article key={entry.id} className="owner-proxy-row" aria-label={name} data-selected={selected.includes(entry.id) || undefined}>
          <ResourceSelection label={t("ownerProxies.select", { name })} checked={selected.includes(entry.id)} disabled={!onChange} onChange={() => setSelected(current => current.includes(entry.id) ? current.filter(id => id !== entry.id) : [...current, entry.id])} />
          <span className="owner-proxy-icon" data-state={success ? "ready" : check ? "error" : "unknown"} data-relay-tooltip={status}><Cable aria-hidden="true" /></span>
          <div className="owner-proxy-identity"><h2>{name}</h2>{entry.label && <span>{endpoint}</span>}</div>
          <div className="owner-proxy-details"><span className="owner-proxy-location" tabIndex={0} data-relay-tooltip={status}>{location || status}</span><span tabIndex={0} data-relay-tooltip={t("ownerProxies.latency")}>{check && Number.isFinite(check.elapsedMs) ? t("ownerProxies.milliseconds", { value: check.elapsedMs.toLocaleString(locale) }) : "—"}</span><span tabIndex={0} data-relay-tooltip={t("ownerProxies.speed")}>—</span><span className="owner-proxy-account-count" tabIndex={0} data-relay-tooltip={t("ownerProxies.accounts")}><Users aria-hidden="true" />{entry.assignedAccountIds.length.toLocaleString(locale)}</span></div>
          <div className="owner-proxy-actions"><span className="owner-disabled-action" tabIndex={0} data-relay-tooltip={t("ownerProxies.checkUnavailable")}><button type="button" className="relay-icon-button" aria-label={t("ownerProxies.check")} disabled><RefreshCw aria-hidden="true" /></button></span><span className="owner-disabled-action" tabIndex={0} data-relay-tooltip={t("ownerProxies.checkUnavailable")}><button type="button" className="relay-icon-button" aria-label={t("ownerProxies.testSpeed")} disabled><Gauge aria-hidden="true" /></button></span><button type="button" className="relay-icon-button" aria-label={t("ownerProxies.manageAccounts")} data-relay-tooltip={t("ownerProxies.manageAccounts")} disabled={!onChange} onClick={() => setManaging(entry)}><Users aria-hidden="true" /></button><ResourceMenu name={name} active={active} actions={[{ label: t("ownerProxies.rename"), disabled: !onChange, run: () => { setRenaming(entry); setLabel(entry.label ?? ""); } }, { label: t("ownerProxies.delete"), danger: true, disabled: !onChange, run: () => setDeleting([entry.id]) }]} /></div>
        </article>;
      })}
    </div>}
    {importOpen && active && <ProxyImport onClose={onImportClose} existing={entries} onImport={imported => { onChange?.([...entries, ...imported]); }} />}
    {renaming && <Dialog title={t("ownerProxies.rename")} className="owner-proxy-dialog" onClose={() => setRenaming(null)}><form className="owner-proxy-form" onSubmit={event => { event.preventDefault(); onChange?.(entries.map(entry => entry.id === renaming.id ? { ...entry, label: label.trim() } : entry)); setRenaming(null); }}><div className="owner-proxy-context"><Cable aria-hidden="true" /><span>{safeProxyEndpoint(renaming.endpoint)}</span></div><label>{t("common.name")}<input data-initial-focus value={label} maxLength={80} autoComplete="off" onChange={event => setLabel(event.target.value)} /></label><div className="button-row"><button type="button" className="button" onClick={() => setRenaming(null)}>{t("common.cancel")}</button><button type="submit" className="button primary-button">{t("common.save")}</button></div></form></Dialog>}
    {managing && <ProxyAccounts proxy={managing} entries={entries} accounts={accounts} onChange={onChange} onClose={() => setManaging(null)} />}
    {deleting.length > 0 && <Dialog title={t("ownerProxies.deleteTitle")} className="owner-proxy-dialog" onClose={() => setDeleting([])}><div className="owner-proxy-form"><p>{t("ownerProxies.deleteConfirm", { count: deleting.length.toLocaleString(locale) })}</p>{entries.some(entry => deleting.includes(entry.id) && entry.assignedAccountIds.length > 0) && <p className="form-note">{t("ownerProxies.deleteAssigned")}</p>}<div className="button-row"><button type="button" className="button" data-initial-focus onClick={() => setDeleting([])}>{t("common.cancel")}</button><button type="button" className="button danger-button" onClick={remove}>{t("ownerProxies.delete")}</button></div></div></Dialog>}
  </section>;
}

function ProxyImport({ existing, onImport, onClose }: { existing: OwnerProxy[]; onImport: (entries: OwnerProxy[]) => void; onClose: () => void }) {
  const { t, locale } = useLocale();
  const [content, setContent] = useState("");
  const [revealed, setRevealed] = useState(false);
  const [result, setResult] = useState<{ added: number; duplicates: number } | null>(null);
  const id = useId();
  const parsed = parseProxyImport(content, existing);
  return <Dialog title={t("ownerProxies.import")} className="owner-proxy-dialog" onClose={onClose}><form className="owner-proxy-form" onSubmit={event => {
    event.preventDefault();
    if (parsed.tooMany || parsed.invalidLines.length || !parsed.entries.length) return;
    onImport(parsed.entries.map(entry => ({ ...entry, id: crypto.randomUUID(), assignedAccountIds: [], createdAtMs: Date.now() })));
    setResult({ added: parsed.entries.length, duplicates: parsed.duplicates }); setContent("");
  }}>
    {result ? <p role="status">{t("ownerProxies.importResult", { added: result.added.toLocaleString(locale), duplicates: result.duplicates.toLocaleString(locale) })}</p> : <>
      <label><span id={`${id}-label`}>{t("ownerProxies.proxyList")}</span><textarea data-initial-focus className={revealed ? "" : "owner-proxy-secret"} value={content} rows={7} maxLength={65536} autoComplete="off" spellCheck={false} autoCorrect="off" autoCapitalize="none" aria-labelledby={`${id}-label`} aria-invalid={parsed.tooMany || parsed.invalidLines.length > 0 || undefined} aria-describedby={`${id}-note${parsed.tooMany || parsed.invalidLines.length ? ` ${id}-error` : ""}`} onChange={event => setContent(event.target.value)} /></label>
      <label className="owner-proxy-reveal"><input type="checkbox" checked={revealed} onChange={event => setRevealed(event.target.checked)} />{t("ownerProxies.reveal")}</label>
      <div className="owner-proxy-formats"><code>host:port:user:pass</code><code>user:pass@host:port</code><code>http(s)://…</code><code>socks5(h)://…</code></div>
      {parsed.tooMany && <p id={`${id}-error`} className="source-form-error" role="alert">{t("ownerProxies.tooMany")}</p>}
      {parsed.invalidLines.length > 0 && <p id={`${id}-error`} className="source-form-error" role="alert">{t("ownerProxies.invalidLines", { lines: parsed.invalidLines.join(", ") })}</p>}
    </>}
    <p className="form-note" id={`${id}-note`}>{t("ownerProxies.previewOnly")}</p>
    <div className="button-row">{result ? <><button type="button" className="button" onClick={() => setResult(null)}>{t("ownerProxies.addMore")}</button><button type="button" className="button primary-button" onClick={onClose}>{t("common.close")}</button></> : <><button type="button" className="button" onClick={onClose}>{t("common.cancel")}</button><button type="submit" className="button primary-button" disabled={parsed.tooMany || Boolean(parsed.invalidLines.length) || !parsed.entries.length}>{t("ownerProxies.importCount", { count: parsed.entries.length.toLocaleString(locale) })}</button></>}</div>
  </form></Dialog>;
}

function ProxyAccounts({ proxy, entries, accounts, onChange, onClose }: { proxy: OwnerProxy; entries: OwnerProxy[]; accounts: OwnerConnection[]; onChange: ((entries: OwnerProxy[]) => void) | undefined; onClose: () => void }) {
  const { t, locale } = useLocale();
  const [selected, setSelected] = useState(proxy.assignedAccountIds.filter(id => accounts.some(account => account.id === id)));
  const [query, setQuery] = useState("");
  const visible = accounts.filter(account => account.name.toLocaleLowerCase(locale).includes(query.trim().toLocaleLowerCase(locale)));
  const allVisible = visible.length > 0 && visible.every(account => selected.includes(account.id));
  return <Dialog title={t("ownerProxies.manageAccounts")} className="owner-proxy-dialog owner-proxy-accounts-dialog" onClose={onClose}><form className="owner-proxy-form" onSubmit={event => { event.preventDefault(); onChange?.(assignProxyAccounts(entries, proxy.id, selected, accounts.map(account => account.id))); onClose(); }}>
    <div className="owner-proxy-context"><Cable aria-hidden="true" /><strong>{proxy.label || safeProxyEndpoint(proxy.endpoint)}</strong></div>
    <div className="owner-proxy-account-toolbar"><label className="owner-proxy-reveal"><input type="checkbox" checked={allVisible} disabled={!visible.length} onChange={() => setSelected(current => allVisible ? current.filter(id => !visible.some(account => account.id === id)) : [...new Set([...current, ...visible.map(account => account.id)])])} />{t("ownerProxies.selectAll")}</label><input type="search" aria-label={t("ownerProxies.searchAccounts")} placeholder={t("ownerProxies.searchAccounts")} data-initial-focus value={query} onChange={event => setQuery(event.target.value)} /></div>
    <div className="owner-proxy-account-grid">{visible.map(account => <label key={account.id}><input type="checkbox" checked={selected.includes(account.id)} onChange={() => setSelected(current => current.includes(account.id) ? current.filter(id => id !== account.id) : [...current, account.id])} /><strong>{account.name}</strong></label>)}</div>
    {!visible.length && <p className="owner-resource-empty">{t("ownerProxies.noResults")}</p>}
    <p className="form-note">{t("ownerProxies.previewOnly")}</p>
    <div className="owner-proxy-assignment-footer"><span>{t("ownerProxies.selectedCount", { count: selected.length.toLocaleString(locale) })}</span><div className="button-row"><button type="button" className="button" onClick={onClose}>{t("common.cancel")}</button><button type="submit" className="button primary-button">{t("common.save")}</button></div></div>
  </form></Dialog>;
}
