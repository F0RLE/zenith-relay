import { useEffect, useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import { checkCopy, clearCopy, confirmCopy, getCopyReview, listCopySources, prepareCopy } from "../../api/hostedDevice";
import type { HostedCopyReview, HostedCopyState } from "../../api/types/hostedDevice";
import { Button, CopyButton, OptionMenu } from "../../components/Ui";

const errors = {
  copy_source_unavailable: "hostedCopy.sourceUnavailable",
  invalid_copy_review: "hostedCopy.sourceUnavailable",
  copy_source_changed: "hostedCopy.sourceChanged",
  copy_not_authorized: "hostedCopy.notAuthorized",
  device_storage_unavailable: "hosted.storage",
} as const;

function CopyForm({ invitation }: { invitation: string }) {
  const { t } = useTranslation();
  const [sources, setSources] = useState<Awaited<ReturnType<typeof listCopySources>>>();
  const [review, setReview] = useState<HostedCopyReview>();
  const [source, setSource] = useState("");
  const [state, setState] = useState<HostedCopyState>("awaiting_approval");
  const [error, setError] = useState<keyof typeof errors | "unconfirmed">();
  const [busy, setBusy] = useState(false);
  const [clearing, setClearing] = useState(false);
  const [reload, setReload] = useState(0);
  const running = useRef(false);

  useEffect(() => {
    let live = true;
    void getCopyReview(invitation).then(async pending => {
      if (!live) return;
      if (pending) setReview(pending);
      else {
        const rows = await listCopySources(invitation);
        if (live) setSources(rows);
      }
    }).catch(() => { if (live) setError("device_storage_unavailable"); });
    return () => { live = false; };
  }, [invitation, reload]);

  async function run(action: "prepare" | "check" | "confirm" | "clear") {
    if (running.current) return;
    running.current = true;
    setBusy(true);
    setError(undefined);
    setClearing(false);
    try {
      if (action === "prepare") {
        setReview(await prepareCopy(invitation, source));
        setState("awaiting_approval");
      } else if (review) {
        const op = review.plan.operation_id;
        if (action === "clear") {
          await clearCopy(invitation, op);
          setReview(undefined);
          setSources(undefined);
          setSource("");
          setReload(n => n + 1);
        } else {
          setState(await (action === "check" ? checkCopy : confirmCopy)(invitation, op));
        }
      }
    } catch (e) {
      setError(typeof e === "string" && Object.hasOwn(errors, e) ? e as keyof typeof errors : "unconfirmed");
      if (action === "confirm") setState("awaiting_approval");
    } finally {
      running.current = false;
      setBusy(false);
    }
  }

  const plan = review?.plan;
  return <div className="hosted-copy-body">
    {error && <p role="alert">{t(error === "unconfirmed" ? "hostedCopy.unconfirmed" : errors[error])}</p>}
    {!plan && (sources ? sources.length > 0
      ? <form onSubmit={e => { e.preventDefault(); if (source) void run("prepare"); }}>
        <div className="relay-field"><span>{t("hostedCopy.source")}</span>
          <OptionMenu label={t("hostedCopy.source")} value={source} onChange={setSource} disabled={busy} align="start"
            options={[{ value: "", label: t("hostedCopy.selectSource") }, ...sources.map(s => ({ value: s.id, label: s.label }))]} />
        </div>
        <Button type="submit" disabled={busy || !source}>{t("hostedCopy.preview")}</Button>
      </form>
      : <p>{t("hostedCopy.empty")}</p>
      : !error ? <p role="status">{t("common.loading")}</p>
      : <Button onClick={() => { setError(undefined); setReload(n => n + 1); }}>{t("common.retry")}</Button>)}
    {plan && <>
      <dl className="hosted-copy-plan">
        <div><dt>{t("hostedCopy.source")}</dt><dd>{plan.label}<small>{plan.endpoint}</small></dd></div>
        <div><dt>{t("hostedCopy.route")}</dt><dd>{plan.proxy?.endpoint ?? t("hostedCopy.direct")}
          {plan.proxy?.authenticated && <small>{t("hostedCopy.proxyAuth")}</small>}</dd></div>
      </dl>
      <small>{t("hostedCopy.unchanged")}</small>
      {state === "awaiting_approval" && review && <label className="relay-field">
        <span>{t("hostedCopy.paste")}</span>
        <input readOnly value={review.code} onFocus={e => e.currentTarget.select()} />
      </label>}
      {state !== "awaiting_approval" && <p role="status">{t(`hostedCopy.${state}`)}</p>}
      <div className="hosted-device-actions">
        {state === "awaiting_approval" && review && <CopyButton value={review.code} label={t("common.copy")} />}
        {state === "approved" && <Button variant="primary" disabled={busy} onClick={() => void run("confirm")}>{t("hostedCopy.confirm")}</Button>}
        {(state === "awaiting_approval" || state === "approved") &&
          <Button disabled={busy} onClick={() => void run("check")}>{t("hostedCopy.check")}</Button>}
        <Button variant="ghost" disabled={busy} onClick={() => clearing ? void run("clear") : setClearing(true)}>
          {t(clearing ? "hostedCopy.confirmClear" : "hostedCopy.clear")}
        </Button>
        {clearing && <Button disabled={busy} onClick={() => setClearing(false)}>{t("common.cancel")}</Button>}
      </div>
      <small><code>{plan.operation_id}</code></small>
    </>}
  </div>;
}

export function HostedSourceCopy({ invitation }: { invitation: string }) {
  const { t } = useTranslation();
  const [open, setOpen] = useState(false);
  return <details className="hosted-copy" onToggle={e => setOpen(e.currentTarget.open)}>
    <summary>{t("hostedCopy.title")}</summary>
    {open && <CopyForm key={invitation} invitation={invitation} />}
  </details>;
}
