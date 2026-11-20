import { useEffect, useRef, useState, type FormEvent } from "react";
import { changeCustomerAccess, readOwnerOverview, readService, signInOwner } from "../../api/ownerSession";
import { ControlError, uuidText } from "../../api/control";
import { useLocale } from "../../i18n/LocaleProvider";

type Session = Awaited<ReturnType<typeof readOwnerOverview>>;
export function IdentityQualification({ onSession }: { onSession: (session: Session) => void }) {
  const { t } = useLocale();
  const [view, setView] = useState<Awaited<ReturnType<typeof readService>>>();
  const [input, setInput] = useState("");
  const [failed, setFailed] = useState(false);
  const [busy, setBusy] = useState(false);
  const [retry, setRetry] = useState(0);
  const flight = useRef(false);
  const op = useRef<{ id: string; revision: string } | undefined>(undefined);
  useEffect(() => {
    let current = true;
    void readService().then(v => { if (current) setView(v); }).catch(() => { if (current) setFailed(true); });
    return () => { current = false; };
  }, [retry]);
  const pause = view?.mode === "qualification";
  const ids = pause ? [] : input.trim().split(/[\s,]+/).filter(Boolean);
  const valid = pause || (ids.length > 0 && ids.length <= 16 && ids.every(id => uuidText.test(id)) && new Set(ids).size === ids.length);
  async function submit(e: FormEvent) {
    e.preventDefault();
    if (flight.current || !view || !valid) return;
    flight.current = true; setBusy(true); setFailed(false);
    const command = op.current ??= { id: crypto.randomUUID(), revision: view.revision };
    try {
      await signInOwner();
      const renewed = await readOwnerOverview(AbortSignal.timeout(10000));
      onSession(renewed);
      await changeCustomerAccess(renewed.csrf, command.id, command.revision, ids, pause);
      const [next, session] = await Promise.all([readService(), readOwnerOverview(AbortSignal.timeout(10000))]);
      op.current = undefined; setInput(""); setView(next); onSession(session);
    } catch (err) {
      if (err instanceof ControlError && err.status === 409) { op.current = undefined; setRetry(n => n + 1); }
      setFailed(true);
    } finally { flight.current = false; setBusy(false); }
  }
  return <form className="owner-setup-form entry-scope" onSubmit={e => void submit(e)}>
    {!pause && <label>{t("customer.reviewIds")}<input value={input} spellCheck={false} autoComplete="off"
      disabled={busy} onChange={e => { op.current = undefined; setInput(e.target.value); }} /></label>}
    {failed && <p role="alert">{t("customer.scopeFailed")}</p>}
    {failed && !view ? <button className="key-button key-secondary" type="button" onClick={() => { setFailed(false); setRetry(n => n + 1); }}>{t("common.retry")}</button>
      : <button className={`key-button ${pause ? "key-secondary" : "key-primary"}`} type="submit" disabled={!view || !valid || busy}>
        {t(pause ? "customer.pause" : "customer.approve")}</button>}
  </form>;
}
