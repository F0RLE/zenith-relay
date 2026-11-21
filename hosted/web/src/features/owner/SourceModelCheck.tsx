import { useEffect, useId, useRef, useState } from "react";
import type { SavedSource } from "../../api/connections";
import { ControlError } from "../../api/control";
import { OwnerPasskeyError } from "../../api/ownerSession";
import { validDeclaredModel } from "../../api/sourceIntake";
import { approveModelCheck, checkLimits, checkPending, readModelCheck } from "../../api/sourceModelCheck";
import type { CheckCommand, CheckReceipt } from "../../api/sourceModelCheck";
import { useLocale } from "../../i18n/LocaleProvider";

const fields = [
  ["max_input_bytes", "modelCheck.inputBytes"],
  ["max_output_tokens", "modelCheck.outputTokens"],
  ["tokens_per_byte", "modelCheck.tokensPerByte"],
  ["overhead_tokens", "modelCheck.overhead"],
] as const;
const labels = {
  queued: "modelCheck.queued", running: "modelCheck.running", verified: "modelCheck.verified",
  not_verified: "modelCheck.notVerified", unconfirmed: "modelCheck.unconfirmed", expired: "modelCheck.expired",
} as const;
const empty = { max_input_bytes: "", max_output_tokens: "", tokens_per_byte: "", overhead_tokens: "" };

export function SourceModelCheck({ source, csrf, allowed, onVerified }: {
  source: SavedSource; csrf: string; allowed: boolean; onVerified: () => void;
}) {
  const { t } = useLocale();
  const form = useId();
  const [expanded, setExpanded] = useState(false);
  const [model, setModel] = useState(() => source.declared_models?.length === 1 ? source.declared_models[0]! : "");
  const [draft, setDraft] = useState(empty);
  const [check, setCheck] = useState<CheckReceipt | null>();
  const [pending, setPending] = useState<CheckCommand>();
  const [operation, setOperation] = useState<string>();
  const [sending, setSending] = useState(false);
  const [retry, setRetry] = useState(0);
  const [readFailed, setReadFailed] = useState(false);
  const [error, setError] = useState<"modelCheck.unconfirmedApproval" | "modelCheck.alreadyVerified" |
    "modelCheck.changed" | "modelCheck.limit" | "catalog.retry">();
  const refreshed = useRef<string | undefined>(undefined);
  const opener = useRef<HTMLButtonElement>(null);
  const active = checkPending(check ?? null);
  const redundantModel = check?.state === "verified" && source.verified_models.length === 1 && source.verified_models[0] === check.model;
  const valid = validDeclaredModel(model) && fields.every(([key]) => {
    const [min, max] = checkLimits[key];
    return /^\d{1,7}$/.test(draft[key]) && Number(draft[key]) >= min && Number(draft[key]) <= max;
  });

  useEffect(() => {
    const controller = new AbortController();
    let timer: number | undefined;
    const poll = async () => {
      try {
        const receipt = await readModelCheck(source.id, operation, controller.signal);
        if (controller.signal.aborted) return;
        setCheck(receipt); setReadFailed(false);
        if (receipt && operation === receipt.operation) {
          setPending(undefined); setError(undefined); setExpanded(false);
        }
        if (receipt?.state === "verified" && refreshed.current !== receipt.operation) {
          refreshed.current = receipt.operation;
          onVerified();
        }
        if (checkPending(receipt)) timer = window.setTimeout(next, 1500);
      } catch (e) {
        if (controller.signal.aborted) return;
        if (e instanceof ControlError && e.status === 401) location.replace("/admin/login");
        else setReadFailed(true);
      }
    };
    const next = () => {
      if (document.hidden) timer = window.setTimeout(next, 1500);
      else void poll();
    };
    void poll();
    return () => { controller.abort(); window.clearTimeout(timer); };
  }, [source.id, operation, retry, onVerified]);

  async function submit() {
    if (!allowed || !source.serving || sending || active || check === undefined || readFailed || !valid) return;
    const command = pending ?? { operation_id: crypto.randomUUID(), expected_revision: source.serving.revision,
      inputs: { model, max_input_bytes: Number(draft.max_input_bytes), max_output_tokens: Number(draft.max_output_tokens),
        tokens_per_byte: Number(draft.tokens_per_byte), overhead_tokens: Number(draft.overhead_tokens) } };
    setSending(true); setError(undefined);
    try {
      await approveModelCheck(source.id, csrf, command);
      setCheck(undefined); setExpanded(false);
      setOperation(command.operation_id); setRetry(n => n + 1);
    } catch (e) {
      if (e instanceof OwnerPasskeyError) setError("catalog.retry");
      else if (e instanceof ControlError && e.status < 500) {
        if (e.status === 401) { location.replace("/admin/login"); return; }
        setPending(undefined); setOperation(undefined); setRetry(n => n + 1);
        setError(e.code === "model_check_already_verified" ? "modelCheck.alreadyVerified"
          : e.code === "model_check_limit" ? "modelCheck.limit" : "modelCheck.changed");
      } else {
        // Read that operation before offering to send the same confirmation again.
        setPending(command); setCheck(undefined); setOperation(command.operation_id);
        setReadFailed(false); setRetry(n => n + 1);
        setError("modelCheck.unconfirmedApproval");
      }
    } finally { setSending(false); }
  }

  if (!allowed && !check && !readFailed) return null;
  return <section className="owner-model-check" aria-label={t("modelCheck.title")}>
    {check && <div className="owner-model-check-receipt" role="status">
      <code className={redundantModel ? "sr-only" : undefined}>{check.model}</code><span>{t(labels[check.state])}</span>
    </div>}
    {readFailed && <div className="owner-model-check-receipt"><small role="alert">{t("unavailable.server")}</small>
      <button type="button" className="key-button key-secondary" onClick={() => setRetry(n => n + 1)}>{t("sourceServing.refresh")}</button></div>}
    {error && <small role="alert">{t(error)}</small>}
    {allowed && !active && !readFailed && <>
      <button ref={opener} type="button" className="key-button key-secondary owner-model-check-action" aria-expanded={expanded}
        disabled={sending} aria-controls={expanded ? form : undefined} onClick={() => setExpanded(!expanded)}>{t("modelCheck.title")}</button>
      {expanded && <form id={form} className="owner-source-form" onSubmit={event => { event.preventDefault(); void submit(); }}>
        <div className="owner-source-fields">
          <label>{t("source.modelId")}<input value={model} onChange={e => setModel(e.currentTarget.value)} maxLength={128}
            autoComplete="off" spellCheck={false} disabled={sending || !!pending} /></label>
          <fieldset className="owner-model-check-limits"><legend>{t("modelCheck.limits")}</legend>
            {fields.map(([key, label]) => <label key={key}>{t(label)}
              <input value={draft[key]} onChange={e => setDraft(d => ({ ...d, [key]: e.target.value }))}
                inputMode="numeric" maxLength={7} disabled={sending || !!pending} /></label>)}
          </fieldset>
        </div>
        <small className="owner-source-preview-note">{t("modelCheck.cost")}</small>
        <div className="catalog-actions owner-source-form-actions">
          {!pending && <button type="button" className="key-button key-secondary" disabled={sending}
            onClick={() => { setExpanded(false); setError(undefined); opener.current?.focus(); }}>{t("common.cancel")}</button>}
          <button type="submit" className="key-button key-primary" disabled={!valid || sending || check === undefined || readFailed}>
            {t(sending ? "modelCheck.confirming" : pending ? "common.retry" : "modelCheck.run")}</button>
        </div>
      </form>}
    </>}
  </section>;
}
