import { useEffect, useRef, useState, type FormEvent } from "react";
import { ControlError, uuidText } from "../../api/control";
import { captureReasons, readCapture, readCaptures, startCapture, stopCapture,
  type CaptureDetail, type CapturePage, type CaptureReason, type CaptureStart } from "../../api/diagnosticCapture";
import { readOwnerOverview, signInOwner } from "../../api/ownerSession";
import { useLocale } from "../../i18n/LocaleProvider";
import type { MessageKey } from "../../i18n/types";
import { ReportStatus } from "../usage/ReportTools";
import { utcStamp } from "./ownerUsageDisplay";

type OwnerSession = Awaited<ReturnType<typeof readOwnerOverview>>;
type Read = { page?: CapturePage; detail?: CaptureDetail | undefined; error?: string; detailError?: boolean };
type Summary = CapturePage["items"][number];
type Recovery = { operation: string } | { stopped: string };
const reasonLabel = { request_timing: "capture.timing", stream_interruption: "capture.interruption",
  pending_outcome: "capture.outcome" } as const;
const stateLabel = { active: "capture.active", stopped: "capture.stopped", expired: "capture.expired",
  exhausted: "capture.exhausted", revoked: "capture.revoked" } as const;
const eventLabel = { dispatch_committed: "capture.dispatch", upstream_response: "capture.response",
  first_stream_frame: "capture.firstFrame", terminal_observed: "capture.terminal", evidence_committed: "capture.evidence",
  accounting_settled: "capture.settled", accounting_pending: "capture.pending", request_interrupted: "capture.interrupted" } as const;
const problems: Record<string, MessageKey> = {
  capture_disabled: "capture.disabled", capture_busy: "capture.busy", capture_request_unavailable: "capture.requestUnavailable",
  capture_conflict: "capture.conflict", capture_retired: "capture.retired", capture_history_full: "capture.full",
  invalid_capture: "capture.invalid", fresh_passkey_required: "capture.verify",
};

function readTarget(page: CapturePage, selected: string) {
  return page.items.find(s => s.state === "active") ?? page.items.find(s => s.id === selected) ?? page.items[0];
}

function unauthenticated(error: unknown) {
  if (!(error instanceof ControlError) || error.status !== 401) return false;
  location.replace("/admin/login");
  return true;
}

function useCaptures(revision: number, localRevision: number, selected: string) {
  const [read, setRead] = useState<Read>({});
  useEffect(() => {
    const c = new AbortController();
    const signal = () => AbortSignal.any([c.signal, AbortSignal.timeout(8000)]);
    let timer: ReturnType<typeof setTimeout> | undefined, running = false, watch = false;
    async function refresh() {
      if (document.hidden || c.signal.aborted || running) return;
      clearTimeout(timer);
      running = true;
      try {
        const page = await readCaptures(signal());
        if (c.signal.aborted) return;
        const target = readTarget(page, selected);
        watch = page.items.some(s => s.state === "active" || s.auditPending);
        setRead(prev => ({ page, detail: prev.detail?.summary.id === target?.id ? prev.detail : undefined }));
        if (target) {
          try {
            const detail = await readCapture(target.id, signal());
            if (!c.signal.aborted) setRead({ page, detail });
          } catch (e) {
            if (!c.signal.aborted && !unauthenticated(e)) setRead({ page, detailError: true });
          }
        }
      } catch (e) {
        if (!c.signal.aborted && !unauthenticated(e)) setRead(prev => ({ ...prev, error: "unavailable" }));
      } finally {
        running = false;
        if (watch && !c.signal.aborted && !document.hidden) timer = setTimeout(() => void refresh(), 2000);
      }
    }
    const returned = () => void refresh();
    returned();
    window.addEventListener("focus", returned);
    document.addEventListener("visibilitychange", returned);
    return () => {
      window.removeEventListener("focus", returned);
      document.removeEventListener("visibilitychange", returned);
      clearTimeout(timer);
      c.abort();
    };
  }, [revision, localRevision, selected]);
  return read;
}

export function OwnerDiagnosticCapture({ session, onSession, revision }: {
  session: OwnerSession; onSession: (s: OwnerSession) => void; revision: number;
}) {
  const { t, locale } = useLocale();
  const [selected, setSelected] = useState("");
  const [localRevision, reload] = useState(0);
  const { page, detail, detailError, error } = useCaptures(revision, localRevision, selected);
  const [problem, setProblem] = useState<{ message: MessageKey; recovery?: Recovery }>();
  const [busy, setBusy] = useState(false);
  const [editorOpen, setEditorOpen] = useState(false);
  const [request, setRequest] = useState("");
  const [reason, setReason] = useState<CaptureReason>("request_timing");
  const [duration, setDuration] = useState<string>();
  const pending = useRef<CaptureStart | null>(null);
  const running = useRef(false), heading = useRef<HTMLHeadingElement>(null);
  const requestField = useRef<HTMLInputElement>(null), newCapture = useRef<HTMLButtonElement>(null);
  const chosen = page ? readTarget(page, selected) : undefined;
  const isActive = chosen?.state === "active";
  const refresh = () => reload(n => n + 1);
  const config = page?.config;
  const showForm = !chosen || editorOpen;
  const detailForChosen = detail?.summary.id === chosen?.id ? detail : undefined;
  const n = (value: number) => value.toLocaleString(locale);
  const recovery = problem?.recovery;
  const confirmed = recovery && page?.items.some(s => "operation" in recovery
    ? recovery.operation === s.operation : recovery.stopped === s.id && s.state !== "active");

  useEffect(() => {
    if (editorOpen) requestField.current?.focus();
  }, [editorOpen]);

  async function act(run: () => Promise<Summary>, fallback: MessageKey, recovery: Recovery) {
    if (running.current) return;
    running.current = true;
    setBusy(true);
    setProblem(undefined);
    try {
      const done = await run();
      pending.current = null;
      setEditorOpen(false);
      setSelected(done.id);
      heading.current?.focus();
    } catch (e) {
      if (!unauthenticated(e)) {
        const message = e instanceof ControlError && e.code ? problems[e.code] : undefined;
        setProblem(message ? { message } : { message: fallback, recovery });
        heading.current?.focus();
      }
    } finally {
      refresh();
      running.current = false;
      setBusy(false);
    }
  }

  function begin(event: FormEvent<HTMLFormElement>) {
    event.preventDefault();
    if (!config || isActive || running.current || error) return;
    const id = request.trim().toLowerCase(), seconds = Number(duration ?? config.duration);
    if (!uuidText.test(id) || !Number.isInteger(seconds) || seconds < 1 || seconds > config.max) {
      setProblem({ message: "capture.invalid" });
      return;
    }
    const prior = pending.current;
    const command = prior?.request_id === id && prior.reason === reason && prior.seconds === seconds ? prior
      : { request_id: id, operation_id: crypto.randomUUID(), reason, seconds };
    pending.current = command;
    void act(async () => {
      try {
        return await startCapture(session.csrf, command);
      } catch (e) {
        if (!(e instanceof ControlError) || e.code !== "fresh_passkey_required") throw e;
        try { await signInOwner(); } catch { throw e; }
        const newSession = await readOwnerOverview(AbortSignal.timeout(8000));
        onSession(newSession);
        return startCapture(newSession.csrf, command);
      }
    }, "capture.startUnconfirmed", { operation: command.operation_id });
  }

  return <section className="capture-section" aria-labelledby="temporary-capture">
    <div className="capture-heading">
      <h2 id="temporary-capture" ref={heading} tabIndex={-1}>{t("capture.title")}</h2>
      <div className="capture-heading-actions">
        <a className="entry-account-link" href="/admin/usage">{t("common.requests")}</a>
        {config && chosen && !isActive && !error && !showForm && <button type="button" ref={newCapture}
          className="key-button key-secondary" onClick={() => { setProblem(undefined); setEditorOpen(true); }}>{t("capture.new")}</button>}
      </div>
    </div>
    <div className="capture-frame" aria-busy={busy}>
      {(!page || error) && <div className="capture-block"><ReportStatus error={error} retry={refresh} /></div>}
      {page && !isActive && !error && (config ? showForm && <form className="capture-block" onSubmit={begin}
        aria-labelledby="temporary-capture" aria-describedby="capture-scope">
        <div className="capture-fields">
          <label>{t("ownerUsage.requestId")}
            <input ref={requestField} type="text" name="request_id" value={request} required maxLength={36} spellCheck={false} autoComplete="off"
              disabled={busy} onChange={e => setRequest(e.target.value)} /></label>
          <label>{t("capture.reason")}
            <select name="reason" value={reason} disabled={busy} onChange={e => setReason(e.target.value as CaptureReason)}>
              {captureReasons.map(r => <option value={r} key={r}>{t(reasonLabel[r])}</option>)}
            </select></label>
          <label>{t("capture.duration")}
            <input type="number" inputMode="numeric" name="seconds" min={1} max={config.max} required
              value={duration ?? config.duration} disabled={busy} onChange={e => setDuration(e.target.value)} /></label>
          <div className="capture-form-actions">
            {chosen && <button type="button" className="key-button key-secondary" disabled={busy} onClick={() => {
              setProblem(undefined);
              setEditorOpen(false);
              requestAnimationFrame(() => newCapture.current?.focus());
            }}>{t("common.cancel")}</button>}
            <button type="submit" className="key-button key-primary" disabled={busy}>{t("capture.start")}</button>
          </div>
        </div>
        <p className="capture-note" id="capture-scope">{t("capture.scope", {
          events: n(config.events), bytes: n(config.bytes), rate: n(config.rate),
        })}</p>
      </form> : <div className="capture-block"><p>{t("capture.disabled")}</p></div>)}
      {problem && !confirmed && <p className="capture-problem" role="alert">{t(problem.message)}</p>}
      {page && chosen && <div className="capture-block capture-record">
        {!isActive && page.items.length > 1 && <label className="capture-history">{t("capture.recent")}
          <select value={chosen.id} disabled={busy} onChange={e => setSelected(e.target.value)}>
            {page.items.map(s => <option key={s.id} value={s.id}>{utcStamp(s.started, locale)} · {t(stateLabel[s.state])}</option>)}
          </select></label>}
        <div className="capture-toolbar">
          <div className="capture-reference">
            <strong role="status" className="operation-state" data-state={isActive && !error ? "succeeded" : undefined}>
              {t(isActive && error ? "capture.lastKnownActive" : stateLabel[chosen.state])}</strong>
            <a className="entry-account-link" href={`/admin/usage/requests/${chosen.request}`} aria-label={`${t("ownerUsage.requestId")}: ${chosen.request}`}>
              <code>{chosen.request}</code></a>
          </div>
          {isActive && <button type="button" className="key-button key-secondary" disabled={busy}
            onClick={() => void act(() => stopCapture(session.csrf, chosen.id), "capture.stopUnconfirmed", { stopped: chosen.id })}>{t("capture.stop")}</button>}
        </div>
        <div className="capture-meta">
          <span>{t(reasonLabel[chosen.reason])}</span>
          {isActive && <time dateTime={new Date(chosen.expires).toISOString()} title={utcStamp(chosen.expires, locale)}>
            {error ? t("capture.expires", { time: utcStamp(chosen.expires, locale) })
              : t("capture.remaining", { seconds: n(Math.max(0, Math.ceil((chosen.expires - page.asOf) / 1000))) })}</time>}
          {chosen.ended !== null && <time dateTime={new Date(chosen.ended).toISOString()}>{utcStamp(chosen.ended, locale)}</time>}
          <span>{t("capture.events", { used: n(chosen.events), limit: n(chosen.maxEvents) })}</span>
          <span>{t("capture.bytes", { used: n(chosen.bytes), limit: n(chosen.maxBytes) })}</span>
          <span>{t("capture.rate", { limit: n(chosen.rate) })}</span>
        </div>
        {chosen.auditPending && (chosen.ended === null || page.asOf - chosen.ended > 30_000) &&
          <p className="capture-note">{t("capture.auditPending")}</p>}
        <div className="capture-events">
          <h3>{t("capture.milestones")}</h3>
          {detailError ? <ReportStatus error="unavailable" retry={refresh} /> : !detailForChosen ? <ReportStatus /> : <>
            <p className="capture-note">{t(detailForChosen.events.length ? "capture.partial" : "capture.noEvents")}</p>
            {detailForChosen.events.length > 0 && <ol>
              {detailForChosen.events.map(event => <li key={event.sequence}>
                <span>{t(eventLabel[event.code])}</span>
                <time dateTime={new Date(event.at).toISOString()}>{utcStamp(event.at, locale, true)}</time>
              </li>)}
            </ol>}
          </>}
        </div>
      </div>}
    </div>
  </section>;
}
