import { useEffect, useState } from "react";
import { readConnections, type ConnectionPage } from "../../api/connections";
import { ControlError, observeControl } from "../../api/control";
import type { readOwnerOverview } from "../../api/ownerSession";
import { useLocale } from "../../i18n/LocaleProvider";
import { SourcePurchaseLimit } from "./SourcePurchaseLimit";

type Session = Awaited<ReturnType<typeof readOwnerOverview>>;
export function SavedConnections({ session, onSession }: { session: Session; onSession: (s: Session) => void }) {
  const { t } = useLocale();
  const [cursors, setCursors] = useState<string[]>([]);
  const after = cursors.at(-1);
  const [page, setPage] = useState<ConnectionPage>();
  const [failed, setFailed] = useState(false);
  const [retry, setRetry] = useState(0);
  useEffect(() => observeControl(() => readConnections(after), v => {
    setPage(v); setFailed(false);
  }, err => {
    setPage(undefined);
    if (err instanceof ControlError && err.status === 401) location.replace("/admin/login");
    else setFailed(true);
  }), [after, retry]);
  function go(next: string[]) {
    setPage(undefined); setFailed(false); setCursors(next);
  }
  return <section className="entry-catalog" aria-label={t("connections.apiProviders")}>
    {failed ? <div><p role="alert">{t("unavailable.server")}</p><button className="key-button key-secondary"
      onClick={() => { setFailed(false); setRetry(n => n + 1); }}>{t("common.retry")}</button></div>
      : !page ? <p role="status">{t("unavailable.connecting")}</p>
      : !page.items.length ? <p>{t("connections.noSavedConnections")}</p>
      : <ul className="catalog-list">{page.items.map(source => <li className="source-row" key={source.id}>
        <div className="catalog-identity"><strong>{source.label ?? t("connections.apiProvider")}</strong>
          <small><code>{source.id}</code></small></div>
        <div className="source-binding">{source.credential_state === "revoked" && <strong>{t("connections.credentialRevoked")}</strong>}
          <span>{t(source.proxy_bound ? "connections.proxyAssigned" : "connections.noProxyAssigned")}</span></div>
        {source.verified_models.length > 0 ? <div className="source-models"><span>{t("connections.verifiedModels")}</span>
          <ul>{source.verified_models.map(m => <li key={m}>{m}</li>)}</ul></div>
          : <small className="source-models">{t("connections.noVerifiedModels")}</small>}
        <SourcePurchaseLimit source={source} session={session} onSession={onSession} />
      </li>)}</ul>}
    {(after || page?.next_after) && <div className="catalog-actions">
      {after && <button className="key-button key-secondary" onClick={() => go(cursors.slice(0,-1))}>{t("pagination.previousPage")}</button>}
      {page?.next_after && <button className="key-button key-secondary" onClick={() => go([...cursors,page.next_after!])}>{t("pagination.nextPage")}</button>}
    </div>}
  </section>;
}
