import { useEffect, useRef, useState } from "react";
import { ControlError, uuidText } from "../../api/control";
import { issueKey, readKeys, revokeKey, type OwnKey } from "../../api/keys";
import { useLocale } from "../../i18n/LocaleProvider";
import type { MessageKey } from "../../i18n/types";
import { KeyDisclosure } from "./KeyDisclosure";

type Intent = { kind: "reissue" | "revoke"; key: OwnKey };
function pendingOperation(storage: string) {
  try { const id = sessionStorage.getItem(storage); return id && uuidText.test(id) ? id : null; } catch { return null; }
}

export function OwnKeys({ csrf, workspace }: { csrf: string; workspace: string }) {
  const { t, locale } = useLocale();
  const storage = "relay.pendingKey." + workspace;
  const op = useRef(pendingOperation(storage));
  const locked = useRef(false);
  const [busy, setBusy] = useState(false);
  const [retry, setRetry] = useState(0);
  const [page, setPage] = useState<Awaited<ReturnType<typeof readKeys>>>();
  const [error, setError] = useState<MessageKey | null>(null);
  const [recovered, setRecovered] = useState(false);
  const [secret, setSecret] = useState<string | null>(null);
  const [intent, setIntent] = useState<Intent | null>(null);
  const [name, setName] = useState("");
  const nameField = useRef<HTMLInputElement>(null);
  const returnFocus = () => requestAnimationFrame(() => nameField.current?.focus());
  const remember = (id: string | null) => {
    op.current = id;
    try { if (id) sessionStorage.setItem(storage,id); else sessionStorage.removeItem(storage); } catch { /* In-memory recovery still works. */ }
  };
  function failure(e: unknown) {
    if (e instanceof ControlError && e.status === 401) { setSecret(null); location.replace("/login"); }
    else setError(e instanceof ControlError && e.status === 429 ? "keys.issueLimit" : "unavailable.server");
  }
  useEffect(() => {
    const c = new AbortController();
    let active = true;
    const clearSecret = () => setSecret(null);
    window.addEventListener("pagehide", clearSecret);
    // Recovery is a read. A reload must never repeat issuance or redisplay a secret.
    const saved = op.current;
    void Promise.all([readKeys(undefined,c.signal), saved ? readKeys({operation_id:saved},c.signal) : null]).then(([p, found]) => {
      if (!active) return;
      setPage(p); setError(null);
      if (found?.keys.length) { setRecovered(true); remember(null); }
      else if (saved) setError("keys.notConfirmed");
    }).catch(e => { if (active) failure(e); });
    return () => { active = false; c.abort(); window.removeEventListener("pagehide",clearSecret); };
  }, [workspace,retry]);

  async function act(fn: () => Promise<void>) {
    if (locked.current) return;
    locked.current = true; setBusy(true); setError(null);
    try { await fn(); } catch (e) { failure(e); } finally { locked.current = false; setBusy(false); }
  }
  async function issue(label: string, original: string | null) {
    const id = op.current ?? crypto.randomUUID();
    remember(id); setRecovered(false);
    try {
      const result = await issueKey(csrf,id,label,original);
      remember(null); setSecret(result.secret); setRecovered(!result.secret); setIntent(null); setName("");
      setPage(await readKeys());
    } catch (e) {
      if (!op.current) throw e;
      if (e instanceof ControlError && [400,401,403,404,429].includes(e.status)) { remember(null); throw e; }
      const found = await readKeys({operation_id:id});
      if (!found.keys.length) { setError("keys.notConfirmed"); return; }
      remember(null); setRecovered(true); setIntent(null); setPage(await readKeys());
    }
  }
  const disabled = busy || !page || !!secret;
  return <section className="own-keys" aria-label={t("nav.keys")}>
    {secret ? <KeyDisclosure secret={secret} onDone={() => { setSecret(null); returnFocus(); }} />
      : <form className="owner-setup-form own-key-form" onSubmit={e => {
        e.preventDefault(); if (!disabled && name.trim()) void act(() => issue(name.trim(),null));
      }}>
        <label>{t("keys.name")}<input ref={nameField} value={name} required maxLength={80} autoComplete="off" disabled={busy}
          onChange={e => { setName(e.target.value); setIntent(null); }} /></label>
        <button className="key-button key-primary" type="submit" disabled={disabled || !name.trim()}>{t("keys.createKey")}</button>
      </form>}
    {recovered && <p role="status">{t("keys.issueRecovered")}</p>}
    {error && <div role="alert"><p>{t(error)}</p><button className="button" disabled={busy} onClick={() => setRetry(n => n+1)}>{t("common.retry")}</button></div>}
    {!page && !error && <p role="status">{t("unavailable.connecting")}</p>}
    {page && !page.keys.length && <p>{t("keys.empty")}</p>}
    {!!page?.keys.length && <ul className="own-key-list">{page.keys.map(key => <li key={key.id}>
      <div className="own-key-detail"><strong>{key.name}</strong><code>{key.prefix}…</code>
        <span>{t("keys.validUntil")} <time dateTime={new Date(key.expires_at_s*1000).toISOString()}>{new Intl.DateTimeFormat(locale,
          { dateStyle:"medium", timeStyle:"short" }).format(key.expires_at_s*1000)}</time></span></div>
      <div className="own-key-tools"><span>{t(key.status === "active" ? "status.active" : key.status === "expired" ? "account.expired" : "status.revoked")}</span>
        {key.status === "active" && (intent?.key.id === key.id ? <div className="own-key-actions">
          <button className="button" disabled={busy} onClick={() => setIntent(null)}>{t("common.cancel")}</button>
          <button className="button" disabled={disabled} onKeyDown={e => { if (e.repeat && ["Enter"," "].includes(e.key)) e.preventDefault(); }}
            onClick={e => { if (e.detail > 1) return; void act(async () => {
              if (intent.kind === "reissue") await issue(key.name,key.id);
              else { await revokeKey(csrf,key.id); setIntent(null); setPage(await readKeys()); }
            }); }}>{t(intent.kind === "revoke" ? "keys.yesRevoke" : "keys.yesReissue")}</button>
        </div> : <div className="own-key-actions">
          <button className="button" disabled={disabled} onClick={() => setIntent({kind:"reissue",key})}>{t("keys.reissue")}</button>
          <button className="button" disabled={disabled} onClick={() => setIntent({kind:"revoke",key})}>{t("keys.revoke")}</button>
        </div>)}</div>
    </li>)}</ul>}
    {page?.next && <button type="button" className="button" disabled={busy} onClick={() => void act(async () => {
      const more = await readKeys({before:page.next!}); setPage({keys:[...page.keys,...more.keys],next:more.next});
    })}>{t("pagination.nextPage")}</button>}
  </section>;
}
