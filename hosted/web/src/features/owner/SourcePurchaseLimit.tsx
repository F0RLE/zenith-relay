import { useEffect, useId, useRef, useState } from "react";
import type { SavedSource } from "../../api/connections";
import { ControlError, observeControl } from "../../api/control";
import type { readOwnerOverview } from "../../api/ownerSession";
import { changePurchaseLimit, purchaseLimitInput, purchaseLimitMicros, readPurchaseLimit, type PurchaseLimitChange } from "../../api/purchaseLimit";
import { useLocale } from "../../i18n/LocaleProvider";
import type { MessageKey } from "../../i18n/types";
import { formatMoney } from "../usage/model";

type Session = Awaited<ReturnType<typeof readOwnerOverview>>;
type Draft = { capped: boolean; dollars: string; revision: string };

export function SourcePurchaseLimit({ source, session }: {
  source: SavedSource; session: Session;
}) {
  const { t, locale } = useLocale();
  const name = useId(), hintId = `${name}-hint`;
  const [view, setView] = useState<Awaited<ReturnType<typeof readPurchaseLimit>>>();
  const [draft, setDraft] = useState<Draft>();
  const [reload, setReload] = useState(0);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<MessageKey>();
  const flight = useRef(false), version = useRef(0);
  const pending = useRef<PurchaseLimitChange | undefined>(undefined);
  const readOnly = source.credential_state === "revoked" || source.serving?.state === "trashed";

  useEffect(() => {
    return observeControl(async () => {
      const current = version.current;
      try {
        if (!flight.current) return { current, view: await readPurchaseLimit(source.id) };
      } catch (error) {
        return { current, error };
      }
    }, result => {
      if (!result || flight.current || result.current !== version.current) return;
      if (result.view) {
        setView(result.view);
        setError(e => e === "purchaseLimit.readFailed" ? undefined : e);
      } else if (result.error instanceof ControlError && result.error.status === 401) {
        location.replace("/admin/login");
      } else {
        setView(undefined);
        setError("purchaseLimit.readFailed");
      }
    }, () => {});
  }, [source.id, reload]);

  const fields = draft ?? (view && { capped: view.limit_micro_usd !== null, dollars: purchaseLimitInput(view.limit_micro_usd), revision: view.revision });
  const limit = fields?.capped ? purchaseLimitMicros(fields.dollars) : null;
  const changed = view && fields && (limit !== view.limit_micro_usd || fields.revision !== view.revision);
  const invalid = !!fields?.capped && fields.dollars !== "" && limit === undefined;

  async function save() {
    if (flight.current || !view || !fields || !changed || limit === undefined || readOnly) return;
    flight.current = true; version.current++; setBusy(true); setError(undefined);
    const revision = fields.revision;
    if (!pending.current || pending.current.expected_revision !== revision || pending.current.limit_micro_usd !== limit)
      pending.current = { operation_id: crypto.randomUUID(), expected_revision: revision, limit_micro_usd: limit };
    const command = pending.current;
    try {
      await changePurchaseLimit(source.id, session.csrf, command);
      const next = await readPurchaseLimit(source.id);
      pending.current = undefined;
      setView(next); setDraft(undefined);
    } catch (e) {
      if (e instanceof ControlError && e.status === 401) location.replace("/admin/login");
      else if (e instanceof ControlError && e.code === "purchase_limit_changed") {
        pending.current = undefined;
        setView(undefined); setDraft(undefined); setError("purchaseLimit.changed"); setReload(n => n + 1);
      } else setError("catalog.retry");
    } finally { flight.current = false; setBusy(false); }
  }

  function money(n: bigint) {
    const digits = (n % 1_000_000n).toString().padStart(6, "0").replace(/0+$/, "").length;
    return formatMoney(n, Math.max(2, digits), locale) + " USD";
  }

  return <div className="source-purchase">
      {error && <small role="alert">{t(error)}</small>}
      {!view || !fields ? error === "purchaseLimit.readFailed"
        ? <button type="button" className="key-button key-secondary" onClick={() => { setError(undefined); setReload(n => n + 1); }}>{t("common.retry")}</button>
        : <small role="status">{t("unavailable.connecting")}</small>
        : <>
          <time dateTime={view.month_utc.slice(0, 7)}>{new Intl.DateTimeFormat(locale, { month: "long", year: "numeric", timeZone: "UTC" })
            .format(new Date(view.month_utc))} · UTC</time>
          <dl className="catalog-prices">
            <div><dt>{t("purchaseLimit.spent")}</dt><dd>{money(view.accounted)}</dd></div>
            <div><dt>{t("purchaseLimit.outstanding")}</dt><dd>{money(view.outstanding)}</dd></div>
            {view.remaining !== null && <div><dt>{t("purchaseLimit.remaining")}</dt><dd>{money(view.remaining)}</dd></div>}
          </dl>
          {view.unpriced && <small>{t("purchaseLimit.unpriced")}</small>}
          {readOnly ? <small>{view.limit_micro_usd === null ? t("purchaseLimit.unlimited")
            : <>{t("purchaseLimit.monthly")} · {money(BigInt(view.limit_micro_usd))}</>}</small>
            : <form className="owner-setup-form source-purchase-form" onSubmit={e => { e.preventDefault(); void save(); }}>
              <fieldset className="source-purchase-choices" disabled={busy}>
                <legend className="sr-only">{t("purchaseLimit.title")}</legend>
                <label><input type="radio" name={name} checked={!fields.capped}
                  onChange={() => { setError(undefined); setDraft({ ...fields, capped: false }); }} />{t("purchaseLimit.unlimited")}</label>
                <label><input type="radio" name={name} checked={fields.capped}
                  onChange={() => { setError(undefined); setDraft({ ...fields, capped: true }); }} />{t("purchaseLimit.monthly")}</label>
              </fieldset>
              <div className="source-purchase-action catalog-actions">
                {fields.capped && <label>{t("purchaseLimit.amount")}<input value={fields.dollars}
                  inputMode="decimal" spellCheck={false} autoComplete="off" maxLength={20} disabled={busy}
                  aria-invalid={invalid || undefined} aria-describedby={invalid ? hintId : undefined}
                  onChange={e => { setError(undefined); setDraft({ ...fields, dollars: e.currentTarget.value }); }} /></label>}
                <button type="submit" className="key-button key-secondary" disabled={busy || !changed || limit === undefined}>{t("common.save")}</button>
              </div>
              {invalid && <small id={hintId}>{t("purchaseLimit.invalid")}</small>}
            </form>}
        </>}
  </div>;
}
