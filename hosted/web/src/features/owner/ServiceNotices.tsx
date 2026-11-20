import { useEffect, useRef, useState } from "react";
import { Bell, Plus } from "../../components/Icons";
import { ControlError } from "../../api/control";
import { changeNotice, noticeFields, ownerNotices, type NoticeCommand, type OwnerNotice } from "../../api/notices";
import { readOwnerOverview, signInOwner } from "../../api/ownerSession";
import { useLocale } from "../../i18n/LocaleProvider";
import type { MessageKey } from "../../i18n/types";
import { NoticeFields } from "./NoticeFields";
import "./owner-notifications.css";

type Session = Awaited<ReturnType<typeof readOwnerOverview>>;
type Intent = { action: "save"; id: string; revision: number; title: string; message: string }
  | { action: "discard" | "publish"; id: string; revision: number };
const controlId = (id: string, action: Intent["action"]) => `notice-${id}-${action}`;

export function ServiceNotices({ session, onSession }: { session: Session; onSession: (s: Session) => void }) {
  const { t, locale } = useLocale();
  const [rows, setRows] = useState<OwnerNotice[]>();
  const [reload, setReload] = useState(0);
  const [readFailed, setReadFailed] = useState(false);
  const [intent, setIntent] = useState<Intent>();
  const [fault, setFault] = useState<MessageKey>();
  const [busy, setBusy] = useState(false);
  const pending = useRef<NoticeCommand | undefined>(undefined);
  const originControl = useRef<string | undefined>(undefined);
  const flight = useRef(false);
  const create = useRef<HTMLButtonElement>(null);

  useEffect(() => {
    const abort = new AbortController();
    void ownerNotices(AbortSignal.any([abort.signal, AbortSignal.timeout(12000)])).then(current => {
      if (!abort.signal.aborted) setRows(current);
    }, error => {
      if (abort.signal.aborted) return;
      if (error instanceof ControlError && error.status === 401) location.replace("/admin/login");
      else setReadFailed(true);
    });
    return () => abort.abort();
  }, [reload]);

  const refresh = () => { setRows(undefined); setReadFailed(false); setReload(n => n + 1); };
  const close = (check: boolean) => {
    const target = !check && originControl.current;
    pending.current = undefined;
    setIntent(undefined);
    if (check) refresh();
    requestAnimationFrame(() => ((target ? document.getElementById(target) : null) ?? create.current)?.focus());
  };
  const begin = (item: Intent) => {
    originControl.current = item.revision > 0 ? controlId(item.id, item.action) : undefined;
    setFault(undefined); setIntent(item);
  };
  const uncertain = fault === "ownerNotices.actionFailed";

  async function mutate() {
    if (!intent || flight.current) return;
    const fields = intent.action === "save" ? noticeFields(intent.title, intent.message) : undefined;
    if (intent.action === "save" && !fields) return;
    const reviewed: NoticeCommand["command"] = fields ? { action: "save", ...fields }
      : { action: intent.action === "discard" ? "discard" : "publish" };
    const command = pending.current ??= {
      operation_id: crypto.randomUUID(), expected_revision: intent.revision, command: reviewed,
    };
    setBusy(true); setFault(undefined); flight.current = true;
    try {
      try {
        await changeNotice(intent.id, session.csrf, command);
      } catch (error) {
        if (!(error instanceof ControlError) || error.code !== "fresh_passkey_required") throw error;
        let renewed: Session;
        try {
          await signInOwner();
          renewed = await readOwnerOverview(AbortSignal.timeout(10000));
        } catch {
          setFault("ownerNotices.verifyFailed");
          return;
        }
        onSession(renewed);
        await changeNotice(intent.id, renewed.csrf, command);
      }
      close(true);
    } catch (error) {
      const code = error instanceof ControlError ? error.code : undefined;
      if (error instanceof ControlError && error.status === 401) location.replace("/admin/login");
      else if (code === "notice_changed") {
        close(true); setFault("ownerNotices.changed");
      } else if (code === "notice_no_audience" || code === "notice_capacity" || code === "invalid_notice") {
        pending.current = undefined;
        setFault(code === "notice_no_audience" ? "ownerNotices.noAudience"
          : code === "notice_capacity" ? "ownerNotices.capacity" : "ownerNotices.invalid");
      } else setFault("ownerNotices.actionFailed");
    } finally { setBusy(false); flight.current = false; }
  }

  const confirmation = intent && intent.action !== "save" ? intent : undefined;
  const date = new Intl.DateTimeFormat(locale, { dateStyle: "medium", timeStyle: "short" });
  return <section className="owner-notices-surface entry-owner-notices" aria-label={t("shell.notifications")}>
    <header className="owner-notices-toolbar"><span><Bell aria-hidden="true" />{t("ownerNotices.inApp")}</span>
      {!intent && <button ref={create} type="button" className="key-button key-primary"
        onClick={() => begin({ action: "save", id: crypto.randomUUID(), revision: 0, title: "", message: "" })}>
        <Plus aria-hidden="true" />{t("ownerNotices.create")}</button>}
    </header>
    {fault && <p className="notice-feedback" role="alert">{t(fault)}</p>}
    {intent?.action === "save" && <form className="owner-notice-editor" aria-label={t("ownerNotices.editor")}
      onSubmit={event => { event.preventDefault(); void mutate(); }}>
      <NoticeFields title={intent.title} message={intent.message} disabled={busy || uncertain} onChange={fields => {
        pending.current = undefined; setFault(undefined); setIntent({ ...intent, ...fields });
      }} />
      <div className="owner-notice-actions">
        <button type="button" className="key-button key-secondary" disabled={busy} onClick={() => { setFault(undefined); close(uncertain); }}>{t("common.cancel")}</button>
        <button type="submit" className="key-button key-primary" disabled={busy || !noticeFields(intent.title, intent.message)}>
          {t(uncertain ? "common.retry" : "ownerNotices.saveDraft")}</button>
      </div>
    </form>}
    {readFailed && <div className="notice-feedback"><p role="alert">{t("unavailable.server")}</p>
      <button type="button" className="key-button key-secondary" onClick={refresh}>{t("common.retry")}</button></div>}
    {!rows && !readFailed && <p className="notice-feedback" role="status">{t("unavailable.connecting")}</p>}
    {rows?.length === 0 && !intent && <div className="owner-notices-empty"><h2>{t("notifications.empty")}</h2></div>}
    {rows && rows.length > 0 && <ul className="owner-notice-list" aria-label={t("shell.notifications")} inert={intent?.action === "save" || undefined}>
      {rows.map(n => <li key={n.id}><article className="owner-notice-row" aria-label={n.title}>
        <div className="owner-notice-copy"><h2 dir="auto">{n.title}</h2><p dir="auto">{n.message}</p>
          <span className="owner-notice-draft-label">{n.publishedAt === null ? t("ownerNotices.draft") : <>
            {t("ownerNotices.publishedTo", { count: n.recipients.toLocaleString(locale) })} · <time dateTime={new Date(n.publishedAt).toISOString()}>{date.format(n.publishedAt)}</time>
          </>}</span>
        </div>
        {n.publishedAt === null && intent?.action !== "save" && <div className="owner-notice-actions">
          {confirmation?.id === n.id ? <>
            <p>{t(confirmation.action === "publish" ? "ownerNotices.publishQuestion" : "ownerNotices.deleteQuestion")}</p>
            <button autoFocus type="button" className="key-button key-secondary" disabled={busy} onClick={() => { setFault(undefined); close(uncertain); }}>{t("common.cancel")}</button>
            <button type="button" className={`key-button ${confirmation.action === "publish" ? "key-primary" : "key-secondary owner-notice-delete"}`}
              disabled={busy} onClick={() => void mutate()}>{t(uncertain ? "common.retry" :
                confirmation.action === "publish" ? "ownerNotices.confirmPublish" : "ownerNotices.delete")}</button>
          </> : !intent && <>
            <button id={controlId(n.id, "save")} type="button" className="text-button" onClick={() => begin({ action: "save", id: n.id, revision: n.revision, title: n.title, message: n.message })}>{t("common.edit")}</button>
            <button id={controlId(n.id, "discard")} type="button" className="text-button owner-notice-delete" onClick={() => begin({ action: "discard", id: n.id, revision: n.revision })}>{t("ownerNotices.delete")}</button>
            <button id={controlId(n.id, "publish")} type="button" className="key-button key-primary" disabled={session.mode !== "qualification"}
              onClick={() => begin({ action: "publish", id: n.id, revision: n.revision })}>{t("ownerNotices.send")}</button>
          </>}
        </div>}
      </article></li>)}
    </ul>}
  </section>;
}
