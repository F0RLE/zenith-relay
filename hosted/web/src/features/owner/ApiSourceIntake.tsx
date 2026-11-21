import { useRef, useState } from "react";
import { ControlError, uuidText } from "../../api/control";
import { OwnerPasskeyError } from "../../api/ownerSession";
import { readSourceReceipt, saveApiSource } from "../../api/sourceIntake";
import { Dialog } from "../../components/Dialog";
import { Plus, RefreshCw } from "../../components/Icons";
import { OptionMenu } from "../../components/OptionMenu";
import { useLocale } from "../../i18n/LocaleProvider";
import type { MessageKey } from "../../i18n/types";
import { SourceEditor } from "./OwnerSourceDialog";
import type { OwnerSourceConfiguration } from "./sourcePresentation";

const operationKey = "relay:owner:web-source-save";
function pendingOperation() {
  try {
    const ref = sessionStorage.getItem(operationKey);
    if (ref && uuidText.test(ref)) return ref;
  } catch { /* The current dialog can still look up its receipt. */ }
  return undefined;
}

function rememberOperation(id?: string) {
  // Only the operation reference may survive a reload, never the credential or permit.
  try { if (id) sessionStorage.setItem(operationKey, id); else sessionStorage.removeItem(operationKey); }
  catch { /* Storage can be disabled. */ }
}

export function ApiSourceIntake({ csrf, onSaved }: { csrf: string; onSaved: () => void }) {
  const { t } = useLocale();
  const [open, setOpen] = useState(false);
  const [operation, setOperation] = useState(pendingOperation);
  const [message, setMessage] = useState<MessageKey>();
  const [busy, setBusy] = useState(false);
  const flight = useRef(false);
  const key = useRef<HTMLInputElement>(null);
  const proxy = useRef<HTMLInputElement>(null);
  const [proxied, setProxied] = useState(false);
  const [keyReady, setKeyReady] = useState(false);
  const [proxyReady, setProxyReady] = useState(false);

  function resetCredentials() {
    if (key.current) key.current.value = "";
    if (proxy.current) proxy.current.value = "";
    setKeyReady(false); setProxyReady(false); setProxied(false);
  }

  function clearPending() { rememberOperation(); setOperation(undefined); }
  function saved() { resetCredentials(); clearPending(); setOpen(false); onSaved(); }
  function close() { if (!flight.current) { resetCredentials(); setOpen(false); setMessage(undefined); } }

  async function save(draft: OwnerSourceConfiguration) {
    if (flight.current || !keyReady || (proxied && !proxyReady) || !key.current) return;
    const op = crypto.randomUUID();
    let sent = false;
    flight.current = true; setBusy(true); setMessage(undefined);
    try {
      await saveApiSource(csrf, op, {
        label: draft.name.trim(), endpoint: draft.baseUrl, declared_models: draft.models,
      }, key.current.value, proxied ? proxy.current?.value.trim() : undefined, () => {
        sent = true;
        rememberOperation(op);
      });
      saved();
    } catch (error) {
      if (error instanceof ControlError && error.code === "invalid_source_intake_secret") {
        clearPending();
        setMessage("sourceIntake.secretInvalid");
      } else if (sent) {
        resetCredentials();
        setOperation(op);
      } else if (error instanceof ControlError && error.status === 401) {
        location.replace("/admin/login");
      } else {
        setMessage(error instanceof OwnerPasskeyError ? "access.passkeyFailed" : "sourceIntake.saveFailed");
      }
    } finally {
      flight.current = false; setBusy(false);
    }
  }

  async function check() {
    if (!operation || flight.current) return;
    flight.current = true; setBusy(true); setMessage(undefined);
    try {
      const receipt = await readSourceReceipt(operation);
      if (receipt.state === "saved") saved();
      else if (receipt.state === "pending") setMessage("sourceIntake.pending");
      else { clearPending(); setMessage("sourceIntake.saveFailed"); }
    } catch (error) {
      if (error instanceof ControlError && error.status === 401) location.replace("/admin/login");
      else setMessage("unavailable.server");
    } finally {
      flight.current = false; setBusy(false);
    }
  }

  return <>
    <button className="relay-button primary" type="button" aria-haspopup="dialog" onClick={() => setOpen(true)}>
      {operation ? <RefreshCw aria-hidden="true" /> : <Plus aria-hidden="true" />}
      {t(operation ? "sourceIntake.check" : "source.addTitle")}
    </button>
    {open && (operation
      ? <Dialog className="owner-source-dialog" title={t("sourceIntake.check")} onClose={close} dismissible={!busy}>
        <p className="form-note" role="status">{t(message ?? "sourceIntake.unconfirmed")}</p>
        <div className="button-row owner-source-form-actions">
          <button type="button" className="button primary-button" data-initial-focus="true" onClick={() => void check()} disabled={busy}>
            {t("copy.checkResult")}
          </button>
        </div>
      </Dialog>
      : <SourceEditor onSave={draft => void save(draft)} onClose={close} intake={{
        ready: keyReady && (!proxied || proxyReady), busy,
        feedback: message && <p className="form-note source-form-error" role="alert">{t(message)}</p>,
        fields: <>
          <label>{t("sourceIntake.key")}<input ref={key} type="password" maxLength={1024} autoComplete="new-password" disabled={busy}
            onChange={event => setKeyReady(/^[!-~]{1,1024}$/.test(event.currentTarget.value))} /></label>
          <div className="owner-source-protocol-field"><span>{t("sourceIntake.route")}</span>
            <OptionMenu label={t("sourceIntake.route")} value={proxied ? "proxy" : "direct"} disabled={busy} onChange={value => {
              setProxied(value === "proxy"); setProxyReady(false);
            }} options={[{ value: "direct", label: t("sourceIntake.direct") }, { value: "proxy", label: t("sourceIntake.proxy") }]} />
          </div>
          {proxied && <label>{t("sourceIntake.proxyUrl")}<input ref={proxy} type="password" maxLength={2048} autoComplete="new-password" disabled={busy}
            onChange={event => setProxyReady(!!event.currentTarget.value.trim())} /></label>}
        </>,
      }} />)}
  </>;
}
