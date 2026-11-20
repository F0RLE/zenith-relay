import { useRef, useState } from "react";
import { ControlError } from "../../api/control";
import { readOwnerOverview, signInOwner } from "../../api/ownerSession";
import { reviewSourceCopy, validSourceCopyCode, type CopyPreview } from "../../api/sourceCopy";
import { useLocale } from "../../i18n/LocaleProvider";
import type { MessageKey } from "../../i18n/types";

type Session = Awaited<ReturnType<typeof readOwnerOverview>>;
type Props = { session: Session; onSession: (s: Session) => void; onCommitted: () => void };
const stateLabels = {
  approved: "copy.approved",
  expired: "copy.expired",
  committed: "copy.committed",
} as const;

export function DesktopSourceCopy({ session, onSession, onCommitted }: Props) {
  const { t } = useLocale();
  const [code, setCode] = useState("");
  const [preview, setPreview] = useState<CopyPreview>();
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<MessageKey>();
  const running = useRef(false);
  const codeField = useRef<HTMLInputElement>(null);

  async function apply(approve: boolean) {
    if (running.current || !validSourceCopyCode(code.trim())) return;
    if (approve && preview?.state !== "awaiting_approval") return;
    running.current = true;
    setBusy(true);
    setError(undefined);
    const input = code.trim();
    try {
      let result: CopyPreview;
      try { result = await reviewSourceCopy(input, approve, session.csrf); }
      catch (e) {
        if (!(e instanceof ControlError && e.code === "fresh_passkey_required")) throw e;
        await signInOwner();
        const next = await readOwnerOverview(AbortSignal.timeout(10000));
        onSession(next);
        result = await reviewSourceCopy(input, approve, next.csrf);
      }
      setPreview(result);
      if (result.state === "committed") onCommitted();
    } catch (e) {
      if (e instanceof ControlError && e.status === 401) location.replace("/admin/login");
      else setError(e instanceof ControlError && e.status === 409 ? "copy.conflict"
        : e instanceof ControlError && e.status === 400 ? "copy.invalid"
        : e instanceof ControlError && e.status === 429 ? "copy.limit" : "copy.retry");
    } finally {
      running.current = false;
      setBusy(false);
    }
  }

  return <details className="device-panel">
    <summary>{t("copy.title")}</summary>
    <div className="device-manager entry-catalog">
      {error && <p role="alert">{t(error)}</p>}
      {!preview ? <form className="owner-setup-form" onSubmit={e => { e.preventDefault(); void apply(false); }}>
        <label>{t("copy.code")}
          <input ref={codeField} value={code} onChange={e => setCode(e.target.value)} maxLength={4096}
            disabled={busy} autoComplete="off" spellCheck={false} />
        </label>
        <button className="key-button key-secondary" disabled={busy || !validSourceCopyCode(code.trim())}>{t("copy.review")}</button>
      </form> : <>
        <div className="catalog-identity">
          <strong>{preview.label}</strong>
          <small>{preview.endpoint}</small>
          <small>{t("copy.device")}: {preview.device}</small>
          <small>{t("copy.route")}: {preview.proxy?.endpoint ?? t("copy.direct")}
            {preview.proxy?.authenticated ? ` · ${t("copy.proxyAuth")}` : ""}</small>
          <small><code>{preview.operation}</code></small>
        </div>
        {preview.state !== "awaiting_approval" && <span className="catalog-live" role="status">{t(stateLabels[preview.state])}</span>}
        <div className="catalog-actions">
          {preview.state === "awaiting_approval" && <button className="key-button key-primary" disabled={busy}
            onClick={() => void apply(true)}>{t("copy.approve")}</button>}
          {preview.state === "approved" && <button className="key-button key-secondary" disabled={busy}
            onClick={() => void apply(false)}>{t("copy.checkResult")}</button>}
          <button className="key-button key-secondary" disabled={busy} onClick={() => {
            setPreview(undefined);
            setCode("");
            setError(undefined);
            requestAnimationFrame(() => codeField.current?.focus());
          }}>{t("copy.another")}</button>
        </div>
      </>}
    </div>
  </details>;
}
