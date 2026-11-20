import { useEffect, useRef, useState } from "react";
import { changeRule, readRules, type Rule } from "../../api/catalog";
import { ControlError, observeControl } from "../../api/control";
import { readOwnerOverview, signInOwner } from "../../api/ownerSession";
import { useLocale } from "../../i18n/LocaleProvider";
import type { MessageKey } from "../../i18n/types";
import { ModelPriceEditor, type ApplyRule } from "./ModelPriceEditor";

type Session = Awaited<ReturnType<typeof readOwnerOverview>>;

export function QualifiedPool({ session, onSession }: { session: Session; onSession: (session: Session) => void }) {
  const { t } = useLocale();
  const [rows, setRows] = useState<Rule[]>();
  const [rules, setRules] = useState(false);
  const [error, setError] = useState<MessageKey>();
  const [busy, setBusy] = useState(false);
  const [reload, setReload] = useState(0);
  const op = useRef<{ signature: string; id: string } | undefined>(undefined);
  const flight = useRef(false);
  const epoch = useRef(0);
  useEffect(() => observeControl(async () => {
    const version = epoch.current;
    return flight.current ? null : { version, rows: await readRules() };
  }, result => {
    if (result && result.version === epoch.current && !flight.current) {
      setRows(result.rows);
      setError(e => e === "unavailable.server" ? undefined : e);
    }
  }, e => {
      if (flight.current) return;
      if (e instanceof ControlError && e.status === 401) location.replace("/admin/login");
      else setError("unavailable.server");
  }), [reload]);
  const apply: ApplyRule = async (row, action, fields) => {
    if (flight.current) return;
    flight.current = true; epoch.current++; setBusy(true); setError(undefined);
    const signature = JSON.stringify([row.id,row.revision,action,fields]);
    if (op.current?.signature !== signature) op.current = { signature, id: crypto.randomUUID() };
    const id = op.current.id;
    const send = (csrf: string) => changeRule(csrf,row,action,id,fields);
    try {
      try { await send(session.csrf); } catch (e) {
        if (!(e instanceof ControlError) || e.code !== "fresh_passkey_required") throw e;
        await signInOwner();
        const next = await readOwnerOverview(AbortSignal.timeout(10000));
        onSession(next); await send(next.csrf);
      }
      setRows(await readRules()); op.current = undefined;
    } catch (e) {
      if (e instanceof ControlError && e.status === 401) location.replace("/admin/login");
      else if (e instanceof ControlError && e.code === "price_notice_required") setError("catalog.noticeRequired");
      else if (e instanceof ControlError && e.code === "purchase_prices_required") { setError("catalog.purchaseRequired"); setReload(v => v + 1); }
      else if (e instanceof ControlError && e.code === "rule_changed") { setError("catalog.changed"); setReload(v => v + 1); }
      else if (e instanceof ControlError && e.code === "price_review_stale") { setError("catalog.reviewStale"); setReload(v => v + 1); }
      else if (e instanceof ControlError && e.code === "no_price_proposal") setError("catalog.noProposal");
      else if (e instanceof ControlError && e.code === "unrepresentable_markup") setError("catalog.markupPrecision");
      else setError("catalog.retry");
    } finally { flight.current = false; setBusy(false); }
  };
  const visible = rules ? rows?.filter(r => r.admitted) : rows;
  const groups = new Map<string, Rule[]>();
  if (rules) for (const row of visible ?? []) {
    const sources = groups.get(row.model);
    if (sources) sources.push(row);
    else groups.set(row.model, [row]);
  }
  return <section className="entry-catalog" aria-label={t("nav.pool")}>
    <div className="history-period" role="group" aria-label={t("pool.poolSection")}>{[false,true].map(v =>
      <button type="button" key={String(v)} className="key-button key-secondary" aria-pressed={rules===v} disabled={busy}
        onClick={() => { setRules(v); setError(undefined); }}>{t(v ? "modelRules.title" : "pool.members")}</button>)}</div>
    {error && <p role="alert">{t(error)}</p>}
    {!rows && (error ? <button className="key-button key-secondary" onClick={() => { setError(undefined); setReload(v => v + 1); }}>{t("common.retry")}</button>
      : <p role="status">{t("unavailable.connecting")}</p>)}
    {visible?.length === 0 && <p>{t("modelRules.empty")}</p>}
    {!!visible?.length && <>{rules && <div className="catalog-unit">{t("modelRules.units")}</div>}
      <ul className="catalog-list">{rules ? Array.from(groups, ([model, sources]) =>
        <ModelPriceEditor key={model} sources={sources} busy={busy} apply={apply} />)
        : visible.map(r => <li key={r.id} className="catalog-member"><div className="catalog-identity"><strong>{r.source}</strong><small>{r.model}{r.proxy && <> · {t("catalog.viaProxy")}</>}</small></div>
          {!r.ready ? <span>{t("catalog.checkRequired")}</span> : r.admitted ? <span>{t("connections.inPool")}</span>
            : <button className="key-button key-primary" disabled={busy} onClick={() => void apply(r,"admit")}>{t("connections.addToPool")}</button>}
        </li>)}</ul></>}
  </section>;
}
