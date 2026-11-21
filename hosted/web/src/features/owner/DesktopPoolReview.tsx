import { useRef, useState } from "react";
import { ControlError } from "../../api/control";
import { approveDesktopPoolReview, readDesktopPoolReview, validPoolReviewCode, type DesktopPoolPreview } from "../../api/desktopPool";
import { OwnerPasskeyError } from "../../api/ownerSession";
import { useLocale } from "../../i18n/LocaleProvider";
import type { MessageKey } from "../../i18n/types";

export function DesktopPoolReview({ csrf, onApplied }: { csrf: string; onApplied: () => void }) {
  const { t } = useLocale();
  const [code, setCode] = useState("");
  const [preview, setPreview] = useState<DesktopPoolPreview>();
  const [error, setError] = useState<MessageKey>();
  const [busy, setBusy] = useState(false);
  const flight = useRef(false);
  const field = useRef<HTMLInputElement>(null);
  const submit = useRef<HTMLButtonElement>(null);

  async function run(mode: "review" | "approve") {
    const operation = code.trim();
    if (flight.current || !validPoolReviewCode(operation) || (mode === "approve" && (!preview || preview.applied))) return;
    flight.current = true;
    setBusy(true);
    setError(undefined);
    try {
      if (mode === "approve" && preview) {
        await approveDesktopPoolReview(preview, csrf);
        setPreview({ ...preview, applied: true });
        onApplied();
      } else {
        setPreview(await readDesktopPoolReview(operation));
        requestAnimationFrame(() => submit.current?.focus());
      }
    } catch (e) {
      if (e instanceof ControlError && e.status === 401) location.replace("/admin/login");
      else if (e instanceof ControlError && (e.status === 404 || e.status === 409 && e.code === "desktop_pool_changed")) {
        setError("desktopPool.changed");
        setPreview(undefined);
        requestAnimationFrame(() => field.current?.focus());
      } else setError(e instanceof OwnerPasskeyError ? "access.passkeyFailed" : "desktopPool.retry");
    } finally {
      flight.current = false;
      setBusy(false);
    }
  }

  const reset = () => {
    setPreview(undefined); setError(undefined); setCode("");
    requestAnimationFrame(() => field.current?.focus());
  };
  return <details className="device-panel">
    <summary>{t("desktopPool.title")}</summary>
    <div className="device-manager entry-catalog" aria-busy={busy}>
      {error && <p role="alert">{t(error)}</p>}
      {!preview ? <form className="owner-setup-form" onSubmit={e => { e.preventDefault(); void run("review"); }}>
        <label>{t("desktopPool.code")}
          <input ref={field} value={code} onChange={e => setCode(e.target.value)} disabled={busy}
            autoComplete="off" spellCheck={false} maxLength={64} />
        </label>
        <button className="key-button key-secondary" disabled={busy || !validPoolReviewCode(code.trim())}>{t("copy.review")}</button>
      </form> : <>
        <div className="catalog-identity">
          <strong>{preview.source_label}</strong>
          <small><code>{preview.source_id}</code></small>
          <small>{t("copy.device")}: {preview.device_label}</small>
          <small>{t("pool.memberModels")}: {preview.models.join(", ")}</small>
        </div>
        {preview.applied ? <span className="catalog-live" role="status">{t("desktopPool.applied")}</span>
          : <p>{t(preview.action === "include" ? "desktopPool.includeEffect" : "sourceServing.parkEffect")}</p>}
        <div className="catalog-actions">
          <button type="button" className="key-button key-secondary" disabled={busy} onClick={reset}>
            {t(preview.applied ? "copy.another" : "common.cancel")}
          </button>
          {!preview.applied && <button ref={submit} className="key-button key-primary" disabled={busy}
            onClick={() => void run("approve")}>
            {t(preview.action === "include" ? "desktopPool.include" : "connections.removeFromPool")}
          </button>}
        </div>
      </>}
    </div>
  </details>;
}
