import { useEffect, useRef, useState } from "react";
import { ControlError } from "../../api/control";
import { endCustomerSession, readCustomerSessions, type BrowserSession } from "../../api/customerSessions";
import { ChevronLeft } from "../../components/Icons";
import { useLocale } from "../../i18n/LocaleProvider";
import type { MessageKey } from "../../i18n/types";
import "./account-sessions.css";

export const accountPanelUrl = "/app/overview?panel=account";

export function AccountSessions({ csrf, workspace, support }: { csrf: string; workspace: string; support: string | undefined }) {
  const { t, locale } = useLocale();
  const [rows, setRows] = useState<BrowserSession[]>();
  const [target, setTarget] = useState<string>();
  const [revision, setRevision] = useState(0);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<MessageKey>();
  const [ended, setEnded] = useState(false);
  const heading = useRef<HTMLHeadingElement>(null);
  const trigger = useRef<HTMLButtonElement>(null);
  const cancelButton = useRef<HTMLButtonElement>(null);
  const locked = useRef(false);
  const date = new Intl.DateTimeFormat(locale, { dateStyle: "medium", timeStyle: "medium" });

  const focusHeading = () => heading.current?.focus({ preventScroll: true });
  useEffect(() => { focusHeading(); }, []);
  useEffect(() => {
    const request = new AbortController();
    void readCustomerSessions(request.signal).then(result => {
      if (!request.signal.aborted) setRows(result);
    }).catch(err => {
      if (request.signal.aborted) return;
      if (err instanceof ControlError && err.status === 401) location.replace("/login");
      else setError("unavailable.server");
    });
    return () => request.abort();
  }, [csrf, revision]);
  useEffect(() => {
    if (target) cancelButton.current?.focus({ preventScroll: true });
  }, [target]);

  function cancel() {
    setTarget(undefined);
    requestAnimationFrame(() => trigger.current?.focus({ preventScroll: true }));
  }

  async function revoke(id: string) {
    if (locked.current || target !== id) return;
    locked.current = true;
    setBusy(true);
    try {
      if (await endCustomerSession(csrf, id)) {
        setRows(undefined);
        location.replace("/login");
        return;
      }
      setRows(rows => rows?.filter(row => row.id !== id));
      setEnded(true);
    } catch (err) {
      setRows(undefined);
      if (err instanceof ControlError && err.status === 401) location.replace("/login");
      else setError("sessions.unconfirmed");
    } finally {
      setTarget(undefined);
      setBusy(false);
      locked.current = false;
      focusHeading();
    }
  }

  const time = (value: number) => <time dateTime={new Date(value).toISOString()}>{date.format(value)}</time>;
  return <div className="account-sessions" onKeyDown={e => {
    if (e.key === "Escape" && target && !locked.current && !e.defaultPrevented) { e.preventDefault(); cancel(); }
  }}>
    <header className="page-heading customer-page-heading"><h1 ref={heading} tabIndex={-1}>{t("sessions.accountSecurity")}</h1></header>
    <a className="account-sessions-back" href="/app/overview"><ChevronLeft />{t("nav.overview")}</a>
    <section className="account-session-panel" aria-labelledby="browser-sessions-title" aria-busy={busy || (!rows && !error)}>
      <h2 id="browser-sessions-title">{t("sessions.browserSessions")}</h2>
      {!rows && !error && <p role="status">{t("unavailable.connecting")}</p>}
      {error && <div className="account-session-feedback">
        <p role="alert">{t(error)}</p>
        <button className="key-button key-secondary" type="button" onClick={() => {
          focusHeading(); setError(undefined); setRevision(n => n + 1);
        }}>{t("sessions.refresh")}</button>
      </div>}
      {rows && <ul>{rows.map(row => <li key={row.id}>
        <div className="account-session-detail">
          <strong data-current={row.current}>{t(row.current ? "sessions.current" : "sessions.browser")}</strong>
          <dl>
            <dt>{t("sessions.signedInAt")}</dt><dd>{time(row.createdAt)}</dd>
            <dt>{t("sessions.lastActivity")}</dt><dd>{time(row.lastSeenAt)}</dd>
          </dl>
        </div>
        <button type="button" className="key-button key-secondary" aria-expanded={target === row.id}
          aria-controls={target === row.id ? "session-confirmation" : undefined}
          aria-label={row.current ? undefined : t("sessions.endLabel", { time: date.format(row.createdAt) })}
          disabled={!!target} onClick={e => { trigger.current = e.currentTarget; setEnded(false); setTarget(row.id); }}>
          {t(row.current ? "shell.signOut" : "sessions.end")}
        </button>
        {target === row.id && <div id="session-confirmation" className="account-session-confirm">
          <p id="session-consequence">{t(row.current ? "sessions.confirmCurrent" : "sessions.confirmOther")}</p>
          <div>
            <button ref={cancelButton} className="key-button key-secondary" type="button" disabled={busy} onClick={cancel}>
              {t("common.cancel")}
            </button>
            <button className="key-button key-secondary account-session-destructive" type="button"
              aria-describedby="session-consequence" disabled={busy}
              onClick={e => { if (e.detail <= 1) void revoke(row.id); }}>
              {t(row.current ? "sessions.confirmSignOut" : "sessions.confirmEnd")}
            </button>
          </div>
        </div>}
      </li>)}</ul>}
      {ended && <p className="account-session-feedback" role="status">{t("sessions.ended")}</p>}
    </section>
    {support && <section className="account-session-panel account-closure" aria-labelledby="account-closure-title">
      <h2 id="account-closure-title">{t("access.close")}</h2>
      <p>{t("access.supportHint")}</p>
      <div className="account-closure-reference"><span>{t("ownerUsage.workspaceId")}</span><code>{workspace}</code></div>
      <a className="key-button key-secondary" href={support} rel="noreferrer">{t("public.policy.support")}</a>
    </section>}
  </div>;
}
