import { useEffect, useRef, useState, type FormEvent } from "react";
import { changeCustomerAccess, readOwnerOverview, readService } from "../../api/ownerSession";
import { ControlError, uuidText } from "../../api/control";
import { useLocale } from "../../i18n/LocaleProvider";
import type { MessageKey } from "../../i18n/types";

type Session = Awaited<ReturnType<typeof readOwnerOverview>>;
const errors: Partial<Record<string, MessageKey>> = {
  invalid_scope: "customer.reviewInvalid",
  revision_conflict: "customer.scopeChanged",
  invalid_credential: "ownerPasskeys.verifyFailed",
  passkey_changed: "ownerPasskeys.verifyFailed",
  passkey_ceremony_expired: "ownerPasskeys.expired",
  passkey_rate_limited: "ownerPasskeys.rate",
};

export function IdentityQualification({ csrf, onSession }: { csrf: string; onSession: (session: Session) => void }) {
  const { t } = useLocale();
  const [view, setView] = useState<Awaited<ReturnType<typeof readService>>>();
  const [input, setInput] = useState("");
  const [error, setError] = useState<MessageKey>();
  const [busy, setBusy] = useState(false);
  const [retry, setRetry] = useState(0);
  const flight = useRef(false);
  const op = useRef<{ id: string; revision: string } | undefined>(undefined);
  useEffect(() => {
    let current = true;
    void readService().then(v => { if (current) setView(v); }).catch(err => {
      if (!current) return;
      if (err instanceof ControlError && err.status === 401) location.replace("/admin/login");
      else setError("unavailable.server");
    });
    return () => { current = false; };
  }, [retry]);
  const pause = view?.mode === "qualification";
  const ids = pause ? [] : input.trim().split(/[\s,]+/).filter(Boolean);
  const valid = pause || (ids.length > 0 && ids.length <= 16 && ids.every(id => uuidText.test(id)) && new Set(ids).size === ids.length);
  async function submit(e: FormEvent) {
    e.preventDefault();
    if (flight.current || !view || !valid) return;
    flight.current = true; setBusy(true); setError(undefined);
    const command = op.current ??= { id: crypto.randomUUID(), revision: view.revision };
    try {
      await changeCustomerAccess(csrf, command.id, command.revision, ids, pause);
      const [next, session] = await Promise.all([readService(), readOwnerOverview(AbortSignal.timeout(10000))]);
      op.current = undefined; setInput(""); setView(next); onSession(session);
    } catch (err) {
      if (err instanceof ControlError && err.status === 401) location.replace("/admin/login");
      else if (err instanceof ControlError && err.code) {
        if (err.code === "revision_conflict") { op.current = undefined; setRetry(n => n + 1); }
        setError(errors[err.code] ?? "customer.scopeFailed");
      } else setError(err instanceof DOMException && err.name === "NotAllowedError"
        ? "ownerPasskeys.verifyFailed" : "customer.scopeFailed");
    } finally { flight.current = false; setBusy(false); }
  }
  return <form className="owner-setup-form entry-scope" aria-busy={busy} onSubmit={e => void submit(e)}>
    {!pause && <label>{t("customer.reviewIds")}<input value={input} spellCheck={false} autoComplete="off"
      disabled={busy} onChange={e => { op.current = undefined; setError(undefined); setInput(e.target.value); }} /></label>}
    {error && <p role="alert">{t(error)}</p>}
    {error && !view ? <button className="key-button key-secondary" type="button" onClick={() => { setError(undefined); setRetry(n => n + 1); }}>{t("common.retry")}</button>
      : <button className={`key-button ${pause ? "key-secondary" : "key-primary"}`} type="submit" disabled={!view || !valid || busy}>
        {t(pause ? "customer.pause" : "customer.approve")}</button>}
  </form>;
}
