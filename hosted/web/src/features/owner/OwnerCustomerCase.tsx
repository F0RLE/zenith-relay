import { useCallback, useEffect, useRef, useState } from "react";
import { ControlError, uuidText } from "../../api/control";
import {
  accessActions, accessReasons, accessStates, checkCustomerChange, confirmCustomerChange,
  readCustomerCase, reasonsFor, type AccessAction, type AccessChange, type AccessReason,
} from "../../api/customerAccess";
import { OwnerPasskeyError } from "../../api/ownerSession";
import { defaultOwnerUsage, ownerUsageHref } from "../../api/ownerUsage";
import { ChevronLeft } from "../../components/Icons";
import { useLocale } from "../../i18n/LocaleProvider";
import type { MessageKey } from "../../i18n/types";
import { ReportStatus, useReportRead } from "../usage/ReportTools";
import "./owner-customer-case.css";

const consequences = { suspend: "access.suspendHint", reinstate: "access.reinstateHint", close: "access.closeHint" } as const;
const applied = { suspend: "access.suspended", reinstate: "access.reinstated", close: "access.closed" } as const;
type Review = Omit<AccessChange, "reason_code"> & { reason_code: AccessReason | "" };

export function OwnerCustomerCase({ csrf }: { csrf: string }) {
  const { t, locale } = useLocale();
  const workspace = location.pathname.split("/").at(-1) ?? "";
  const valid = uuidText.test(workspace);
  const [seq, setSeq] = useState(0);
  const [review, setReview] = useState<Review>();
  const [unknown, setUnknown] = useState(false);
  const [busy, setBusy] = useState(false);
  const [problem, setProblem] = useState<MessageKey>();
  const running = useRef(false), actionGroup = useRef<HTMLDivElement>(null), initial = useRef<HTMLElement>(null);
  const focusAfterRead = useRef(false);
  const returnFocus = useRef<AccessAction | null>(null);
  const confirm = useRef<HTMLButtonElement>(null);
  const load = useCallback((signal: AbortSignal) => readCustomerCase(workspace, signal), [workspace, seq]);
  const { data, error } = useReportRead(valid ? load : null, "/admin/login");
  const refresh = () => setSeq(n => n + 1);
  const date = new Intl.DateTimeFormat(locale, { dateStyle: "medium", timeStyle: "short" });

  useEffect(() => {
    if (review && !unknown) initial.current?.focus();
  }, [review?.operation_id, unknown]);
  useEffect(() => {
    if (!busy && (unknown || problem === "access.passkeyFailed")) confirm.current?.focus();
  }, [busy, unknown, problem]);
  useEffect(() => {
    if (!data || !focusAfterRead.current) return;
    focusAfterRead.current = false;
    actionGroup.current?.focus();
  }, [data]);

  function cancel() {
    returnFocus.current = review?.action ?? null;
    setReview(undefined); setProblem(undefined);
  }

  async function submit() {
    if (running.current || !review?.reason_code) return;
    const command: AccessChange = { ...review, reason_code: review.reason_code };
    running.current = true; setBusy(true); setProblem(undefined);
    try {
      if (unknown && !await checkCustomerChange(csrf, workspace, command)) {
        setUnknown(false);
        setProblem("access.notApplied");
        return;
      }
      if (!unknown) await confirmCustomerChange(csrf, workspace, command);
      setReview(undefined); setUnknown(false);
      focusAfterRead.current = true;
      refresh();
    } catch (err) {
      if (err instanceof ControlError && err.status === 401) location.replace("/admin/login");
      else if (err instanceof ControlError && err.code === "customer_access_changed") {
        setReview(undefined); setUnknown(false); setProblem("access.changed");
        focusAfterRead.current = true; refresh();
      } else if (err instanceof OwnerPasskeyError) setProblem("access.passkeyFailed");
      else { setUnknown(true); setProblem("access.unknown"); }
    } finally { running.current = false; setBusy(false); }
  }

  return <section className="owner-customer-case" aria-label={t("access.case")}
    aria-busy={busy || (valid && !error && !data)} onKeyDown={e => {
      if (e.key === "Escape" && review && !busy && !unknown) { e.preventDefault(); cancel(); }
    }}>
    <a className="account-sessions-back entry-account-link"
      href={valid ? ownerUsageHref({ ...defaultOwnerUsage, workspace }) : "/admin/usage"}>
      <ChevronLeft aria-hidden="true" />{t("nav.usage")}
    </a>
    {valid && <div className="customer-case-reference">
      <span>{t("ownerUsage.workspaceId")}</span><code>{workspace}</code>
    </div>}
    {!valid || error === "not_found" ? <p role="status">{t("access.missing")}</p>
      : !data ? <ReportStatus error={error} retry={refresh} />
      : <div className="customer-case-panel" tabIndex={-1} ref={actionGroup}>
        <div className="customer-case-status" aria-live="polite">
          <h2>{t("access.title")}</h2>
          <strong data-state={data.state}>{t(accessStates[data.state])}</strong>
        </div>
        {data.last && <dl className="customer-case-last">
          <dt>{t("access.lastChange")}</dt>
          <dd>{t(applied[data.last.action])} · {t(accessReasons[data.last.reason])}</dd>
          <dt>{t("access.operation")}</dt>
          <dd><code>{data.last.id}</code></dd>
          <dt>{t("access.changedAt")}</dt>
          <dd><time dateTime={new Date(data.last.at).toISOString()}>{date.format(data.last.at)}</time></dd>
        </dl>}
        {review ? <div className="customer-case-review">
          <h3>{t(accessActions[review.action])}</h3>
          <p>{t(consequences[review.action])}</p>
          {review.action === "suspend" && <label className="customer-case-reason">{t("access.reason")}
            <select ref={el => { initial.current = el; }} value={review.reason_code} disabled={busy || unknown}
              onChange={e => setReview({ ...review, reason_code: e.target.value as Review["reason_code"] })}>
              <option value="">{t("access.chooseReason")}</option>
              {reasonsFor(review.action).map(reason => <option key={reason} value={reason}>{t(accessReasons[reason])}</option>)}
            </select>
          </label>}
          {problem && <p className="customer-case-feedback" role="alert">{t(problem)}</p>}
          <div className="customer-case-actions">
            {!unknown && <button type="button" className="key-button key-secondary" disabled={busy}
              ref={el => { if (review.action !== "suspend") initial.current = el; }} onClick={cancel}>{t("common.cancel")}</button>}
            <button type="button" className={`key-button key-secondary${review.action === "close" && !unknown ? " customer-close-action" : ""}`}
              ref={confirm} disabled={busy || !review.reason_code} onClick={() => void submit()}>
              {t(unknown ? "access.check" : "access.confirm")}
            </button>
          </div>
        </div> : <>
          {problem && <p className="customer-case-feedback" role="alert">{t(problem)}</p>}
          {data.state !== "closed" && <div className="customer-case-actions">
            {([data.state === "active" ? "suspend" : "reinstate", "close"] as const).map(action =>
              <button type="button" key={action} disabled={busy} className={`key-button key-secondary${action === "close" ? " customer-close-action" : ""}`}
                ref={element => {
                  if (element && returnFocus.current === action) {
                    returnFocus.current = null;
                    element.focus();
                  }
                }}
                onClick={() => {
                  setReview({ action, operation_id: crypto.randomUUID(), expected_revision: data.revision,
                    reason_code: action === "suspend" ? "" : reasonsFor(action)[0]! });
                  setProblem(undefined);
                }}>{t(accessActions[action])}</button>)}
          </div>}
        </>}
      </div>}
  </section>;
}
