import { useCallback, useEffect, useRef, useState } from "react";
import { changeRule, confirmsRuleAction, readRules, type Rule, type RuleAction, type RuleFields } from "../../api/catalog";
import { ControlError, observeControl } from "../../api/control";
import { OwnerPasskeyError } from "../../api/ownerSession";
import { useLocale } from "../../i18n/LocaleProvider";
import type { MessageKey } from "../../i18n/types";

export type ApplyRule = (row: Rule, action: RuleAction, fields?: RuleFields) => Promise<void>;

const conflicts = {
  price_notice_required: "catalog.noticeRequired",
  purchase_prices_required: "catalog.purchaseRequired",
  rule_changed: "catalog.changed",
  price_review_stale: "catalog.reviewStale",
  no_price_proposal: "catalog.noProposal",
  unrepresentable_markup: "catalog.markupPrecision",
} satisfies Record<string, MessageKey>;

export function useOwnerRules(csrf: string) {
  const { t } = useLocale();
  const [rows, setRows] = useState<Rule[]>();
  const [failedRead, setFailedRead] = useState(false);
  const [error, setError] = useState<MessageKey>();
  const [busy, setBusy] = useState(false);
  const [reload, setReload] = useState(0);
  const [retryRule, setRetryRule] = useState<Parameters<ApplyRule>>();
  const op = useRef<{ signature: string; id: string } | undefined>(undefined);
  const flight = useRef(false);
  const epoch = useRef(0);
  const refresh = useCallback(() => {
    epoch.current++;
    setReload(v => v + 1);
  }, []);
  useEffect(() => observeControl(async () => {
    const version = epoch.current;
    return flight.current ? null : { version, rows: await readRules() };
  }, result => {
    if (result && result.version === epoch.current && !flight.current) {
      setRows(result.rows);
      setFailedRead(false);
    }
  }, e => {
    if (flight.current) return;
    if (e instanceof ControlError && e.status === 401) location.replace("/admin/login");
    else setFailedRead(true);
  }), [reload]);

  const apply: ApplyRule = async (row, action, fields) => {
    if (flight.current || failedRead) return;
    flight.current = true; epoch.current++; setBusy(true); setError(undefined); setRetryRule(undefined);
    const signature = JSON.stringify([row.id, row.revision, action, fields]);
    if (op.current?.signature !== signature) op.current = { signature, id: crypto.randomUUID() };
    try {
      await changeRule(csrf, row, action, op.current.id, fields);
      setRows(await readRules()); op.current = undefined;
    } catch (e) {
      if (e instanceof ControlError && e.status === 401) location.replace("/admin/login");
      else if (e instanceof ControlError && e.code && Object.hasOwn(conflicts, e.code)) {
        setError(conflicts[e.code as keyof typeof conflicts]);
        if (["purchase_prices_required", "rule_changed", "price_review_stale"].includes(e.code)) refresh();
      } else {
        setError("catalog.retry");
        if (confirmsRuleAction(action) && !(e instanceof OwnerPasskeyError)) setRetryRule([row, action, fields]);
      }
    } finally { flight.current = false; setBusy(false); }
  };

  return {
    rows, apply, busy, blocked: busy || failedRead, refresh,
    clearError: () => { if (!retryRule) setError(undefined); },
    feedback: <>
      {error && <p role="alert">{t(error)}</p>}
      {failedRead && <div className="catalog-actions">
        <small role="alert">{t("unavailable.server")}</small>
        <button className="key-button key-secondary" type="button" onClick={refresh}>{t("common.retry")}</button>
      </div>}
      {retryRule && !failedRead && <button type="button" className="key-button key-secondary" disabled={busy}
        onClick={() => void apply(...retryRule)}>{t("common.retry")}</button>}
      {!rows && !failedRead && <p role="status">{t("unavailable.connecting")}</p>}
    </>,
  };
}
