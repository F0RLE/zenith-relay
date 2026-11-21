import { useEffect, useRef, useState } from "react";
import { ControlError } from "../../api/control";
import { authorizePasskeyChange, continuePasskeyChange, ownerPasskeyLimit, readOwnerPasskeys, type OwnerPasskey, type PasskeyContinuation } from "../../api/ownerPasskeys";
import { supportsOwnerPasskey } from "../../api/ownerSession";
import { useLocale } from "../../i18n/LocaleProvider";
import type { MessageKey } from "../../i18n/types";
import "./owner-passkeys.css";

type Flow = { step: "add" } | { step: "revoke"; id: string } | PasskeyContinuation;
const errors: Partial<Record<string, MessageKey>> = {
  invalid_credential: "ownerPasskeys.verifyFailed",
  passkey_ceremony_expired: "ownerPasskeys.expired",
  passkey_changed: "ownerPasskeys.changed",
  last_passkey: "ownerPasskeys.last",
  passkey_limit: "ownerPasskeys.limit",
  passkey_rate_limited: "ownerPasskeys.rate",
};

export function OwnerPasskeys({ csrf, setup }: { csrf: string; setup: boolean }) {
  const { t, locale } = useLocale();
  const summary = useRef<HTMLElement>(null);
  const addButton = useRef<HTMLButtonElement>(null);
  const running = useRef(false);
  const field = useRef<HTMLInputElement>(null);
  const commit = useRef<HTMLButtonElement>(null);
  const initiator = useRef<HTMLButtonElement>(null);
  const [open, setOpen] = useState(false);
  const [keys, setKeys] = useState<OwnerPasskey[]>();
  const [version, setVersion] = useState(0);
  const [readFailed, setReadFailed] = useState(false);
  const [flow, setFlow] = useState<Flow>();
  const [name, setName] = useState("");
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<MessageKey>();
  const [ack, setAck] = useState<MessageKey>();
  const supported = supportsOwnerPasskey();
  const canCreate = supported && typeof PublicKeyCredential.parseCreationOptionsFromJSON === "function";

  useEffect(() => {
    if (!open && !setup) return;
    const controller = new AbortController();
    void readOwnerPasskeys(controller.signal).then(keys => {
      if (!controller.signal.aborted) { setKeys(keys); setReadFailed(false); }
    }).catch(err => {
      if (controller.signal.aborted) return;
      if (err instanceof ControlError && err.status === 401) location.replace("/admin/login");
      else setReadFailed(true);
    });
    return () => controller.abort();
  }, [open, setup, version]);

  useEffect(() => {
    if (flow) (flow.step === "add" ? field.current : commit.current)?.focus();
  }, [flow]);

  function begin(button: HTMLButtonElement, next: Flow) {
    initiator.current = button;
    setError(undefined);
    setAck(undefined);
    setFlow(next);
  }

  function cancel() {
    setFlow(undefined);
    setName("");
    setError(undefined);
    requestAnimationFrame(() => (initiator.current?.isConnected ? initiator.current : addButton.current ?? summary.current)?.focus());
  }

  async function apply() {
    if (!flow || running.current) return;
    running.current = true;
    setBusy(true);
    setError(undefined);
    try {
      const result = flow.step === "add"
        ? await authorizePasskeyChange(csrf, { action: "add", label: name.trim() || null })
        : flow.step === "revoke"
          ? await authorizePasskeyChange(csrf, { action: "revoke", passkey_id: flow.id })
          : await continuePasskeyChange(csrf, flow);
      if (result.step === "create" || result.step === "confirm") setFlow(result);
      else if ("sessionEnded" in result && result.sessionEnded) location.replace("/admin/login");
      else {
        setFlow(undefined);
        setName("");
        setAck(result.step === "added" ? "ownerPasskeys.added" : "ownerPasskeys.revoked");
        setVersion(n => n + 1);
        summary.current?.focus();
      }
    } catch (err) {
      if (err instanceof ControlError && err.status === 401) location.replace("/admin/login");
      else {
        const cancelled = err instanceof DOMException && err.name === "NotAllowedError";
        setError(cancelled ? "ownerPasskeys.verifyFailed" : err instanceof ControlError && err.code
          ? errors[err.code] ?? "ownerPasskeys.unconfirmed" : "ownerPasskeys.unconfirmed");
        if (!cancelled) {
          setFlow(flow.step === "add" || flow.step === "create" || flow.step === "confirm" ? { step: "add" } : undefined);
          setVersion(n => n + 1);
        }
      }
    } finally { running.current = false; setBusy(false); }
  }

  const revoke = flow?.step === "revoke" ? keys?.find(key => key.id === flow.id) : undefined;
  const title = t(setup && keys?.length === 1 ? "ownerPasskeys.setup" : "ownerPasskeys.title");
  const date = new Intl.DateTimeFormat(locale, { dateStyle: "medium", timeStyle: "short" });
  return <details className="owner-passkeys" onToggle={event => {
    const expanded = event.currentTarget.open;
    setOpen(expanded);
    if (!expanded) { setFlow(undefined); setName(""); setError(undefined); setAck(undefined); }
  }} onKeyDown={event => {
    if (event.key === "Escape" && flow && !running.current) { event.preventDefault(); cancel(); }
    if (event.repeat && (event.key === "Enter" || event.key === " ")) event.preventDefault();
  }}>
    <summary ref={summary} onClick={event => { if (running.current) event.preventDefault(); }}>{title}</summary>
    {open && <div className="owner-passkey-body" aria-busy={busy}>
      {!keys && !readFailed && <p role="status">{t("unavailable.connecting")}</p>}
      {readFailed && <div className="owner-passkey-actions">
        <p role="alert">{t("unavailable.server")}</p>
        <button type="button" className="key-button key-secondary" onClick={() => setVersion(n => n + 1)}>{t("common.retry")}</button>
      </div>}
      {keys && <ul aria-label={t("ownerPasskeys.title")}>{keys.map(key => {
        const label = key.label ?? t("ownerPasskeys.unnamed");
        return <li key={key.id}>
          <div><strong>{label}</strong>
            <span><time dateTime={new Date(key.createdMs).toISOString()}>{date.format(key.createdMs)}</time>
              {key.current && <small>{t("ownerPasskeys.current")}</small>}</span></div>
          {keys.length > 1 && supported && <button type="button" className="key-button key-secondary"
            aria-label={t("ownerPasskeys.revokeLabel", { name: label })} disabled={!!flow || readFailed}
            onClick={event => begin(event.currentTarget, { step: "revoke", id: key.id })}>{t("ownerPasskeys.revoke")}</button>}
        </li>;
      })}</ul>}
      {keys && !flow && canCreate && keys.length < ownerPasskeyLimit && !readFailed && <button type="button"
        ref={addButton} className="key-button key-secondary" onClick={event => begin(event.currentTarget, { step: "add" })}>{t("ownerPasskeys.add")}</button>}
      {!canCreate && !!keys && <p>{t("owner.passkeyUnsupported")}</p>}
      {flow && (flow.step !== "revoke" || revoke) && <form className="owner-passkey-form"
        onSubmit={event => { event.preventDefault(); void apply(); }}>
        {flow.step === "add" && <label>{t("ownerPasskeys.name")}<input ref={field} value={name} maxLength={48} type="text"
          disabled={busy} onChange={event => setName(event.target.value)} autoComplete="off" /></label>}
        {flow.step === "revoke" && <p>{t("ownerPasskeys.revokeHint", { name: revoke?.label ?? t("ownerPasskeys.unnamed") })}</p>}
        {flow.step === "create" && <p>{t("ownerPasskeys.otherDevice")}</p>}
        <div className="owner-passkey-actions">
          <button ref={commit} type="submit" className={`key-button ${flow.step === "revoke" ? "key-secondary owner-passkey-revoke" : "key-primary"}`}
            disabled={busy || readFailed}>{t(flow.step === "add" ? "ownerPasskeys.verifyExisting" : flow.step === "revoke"
              ? "ownerPasskeys.confirmRevoke" : flow.step === "create" ? "ownerPasskeys.create" : "ownerPasskeys.confirmNew")}</button>
          <button type="button" className="key-button key-secondary" disabled={busy} onClick={cancel}>{t("common.cancel")}</button>
        </div>
      </form>}
      {ack && <p role="status">{t(ack)}</p>}
      {error && <p className="owner-passkey-error" role="alert">{t(error)}</p>}
    </div>}
  </details>;
}
