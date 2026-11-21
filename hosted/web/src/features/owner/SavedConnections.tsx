import { Suspense, lazy, useCallback, useEffect, useRef, useState } from "react";
import { readConnections, type ConnectionPage, type ConnectionView } from "../../api/connections";
import { ControlError, observeControl } from "../../api/control";
import type { readOwnerOverview } from "../../api/ownerSession";
import { Dialog } from "../../components/Dialog";
import { Cable, Layers2, Pause, RefreshCw, Search, Users } from "../../components/Icons";
import { OptionMenu } from "../../components/OptionMenu";
import { useLocale } from "../../i18n/LocaleProvider";
import { ApiSourceIntake } from "./ApiSourceIntake";
import { OwnerSectionHeader } from "./OwnerSectionHeader";
import { ConnectionModels } from "./ConnectionModels";
import { DesktopSourceCopy } from "./DesktopSourceCopy";
import { ResourceCounts } from "./OwnerResourceSummary";
import { PairedDevices } from "./PairedDevices";
import { SavedConnectionCard } from "./SavedConnectionCard";
import { SourcePurchaseLimit } from "./SourcePurchaseLimit";
import { useSourceServing } from "./SourceServing";
import "./owner-workspace.css";
import "./owner-resource-cards.css";
import "./owner-source-editor.css";
import "./saved-connections.css";

type Session = Awaited<ReturnType<typeof readOwnerOverview>>;
const DesktopPoolReview = lazy(() => import("./DesktopPoolReview").then(module => ({ default: module.DesktopPoolReview })));
const tabs = [["api", "connections.apiProviders"], ["trash", "connections.trash"]] as const;
const filters = [["all", "connections.allStates"], ["pool", "connections.inPool"], ["outside", "connections.outsidePool"],
  ["paused", "connections.paused"], ["revoked", "connections.credentialRevoked"]] as const;
type Overlay = { section: "desktop" } | { section: "models" | "purchase"; id: string };

export function SavedConnections({ session }: { session: Session }) {
  const { t, locale } = useLocale();
  const root = useRef<HTMLDivElement>(null);
  const [tab, setTab] = useState<ConnectionView>("api");
  const [query, setQuery] = useState("");
  const [filter, setFilter] = useState("all");
  const [overlay, setOverlay] = useState<Overlay>();
  const [cursors, setCursors] = useState<string[]>([]);
  const after = cursors.at(-1);
  const [page, setPage] = useState<ConnectionPage>();
  const [failed, setFailed] = useState(false);
  const [refreshing, setRefreshing] = useState(false);
  const [retry, setRetry] = useState(0);
  const [added, setAdded] = useState(false);
  function focusTab() {
    root.current?.querySelector<HTMLButtonElement>(".relay-tabs button[aria-pressed=true]")?.focus();
  }
  useEffect(() => observeControl(() => readConnections(tab, after), v => {
    const focus = document.activeElement;
    setPage(v); setFailed(false); setRefreshing(false);
    if (focus?.closest("[data-source-id]")) requestAnimationFrame(() => {
      if (!focus.isConnected && document.activeElement === document.body) focusTab();
    });
  }, err => {
    if (err instanceof ControlError && err.status === 401) location.replace("/admin/login");
    else { setFailed(true); setRefreshing(false); }
  }), [tab, after, retry]);
  const refresh = useCallback(() => { setRefreshing(true); setRetry(n => n + 1); }, []);
  const items = page?.items ?? [];
  const lifecycle = useSourceServing({ sources: items, csrf: session.csrf, stale: failed || refreshing || !page, refresh, focusTab });
  function go(next: string[]) {
    setPage(undefined); setFailed(false); setRefreshing(false); setCursors(next);
  }
  function paginate(next: string[]) {
    focusTab();
    go(next);
  }
  const close = () => setOverlay(undefined);
  const source = overlay && "id" in overlay ? items.find(row => row.id === overlay.id) : undefined;
  const search = query.trim().toLocaleLowerCase(locale);
  const paged = !!(after || page?.next_after);
  const visible = items.filter(item =>
    (!search || [item.label ?? t("connections.apiProvider"), item.id, item.endpoint ?? "",
      ...item.verified_models, ...(item.declared_models ?? [])]
      .some(value => value.toLocaleLowerCase(locale).includes(search))) &&
    (filter === "all" || filter === "pool" && item.serving?.in_pool ||
      filter === "outside" && item.serving && !item.serving.in_pool ||
      filter === "paused" && item.serving?.state === "paused" || filter === "revoked" && item.credential_state === "revoked"));
  return <div ref={root} className="saved-connections">
    <OwnerSectionHeader title="nav.connections" group="connections.connectionType" tabs={tabs} tab={tab} select={next => {
      if (tab === next) return;
      go([]); setTab(next); setFilter("all"); setQuery(""); setAdded(false);
    }}
      action={tab === "api" && <>
        {session.webApiIntake && <ApiSourceIntake csrf={session.csrf} onSaved={() => {
          if (after) go([]);
          setFilter("all"); setQuery(""); setAdded(true); refresh();
        }} />}
        <button className={`relay-button${session.webApiIntake ? "" : " primary"}`} type="button" aria-haspopup="dialog" onClick={() => setOverlay({ section: "desktop" })}>
          <Cable aria-hidden="true" />{t("connections.desktop")}
        </button>
      </>} />
    <section className="owner-resource-controls" aria-label={t("connections.manageConnections")}>
      {tab === "api" && page && !paged && <div className="owner-resource-summary-band"><ResourceCounts stats={[
        { label: "connections.total", value: items.length, icon: <Users aria-hidden="true" /> },
        { label: "connections.inPool", value: items.filter(item => item.serving?.in_pool).length, icon: <Layers2 aria-hidden="true" /> },
        { label: "connections.paused", value: items.filter(item => item.serving?.state === "paused").length, icon: <Pause aria-hidden="true" /> },
      ]} /></div>}
      <div className="owner-resource-toolbar">
        <div className="owner-resource-filters">
          <label className="owner-resource-search"><Search aria-hidden="true" />
            <input type="search" aria-label={t(paged ? "connections.searchPage" : "connections.searchConnections")}
              placeholder={t(paged ? "connections.searchPage" : "connections.findAConnection")} value={query}
              onChange={event => setQuery(event.currentTarget.value)} />
          </label>
          {tab === "api" && <OptionMenu label={t("connections.connectionState")} value={filter} onChange={setFilter}
            options={filters.map(([value, label]) => ({ value, label: t(label) }))} />}
          <button className="relay-icon-button" type="button" aria-label={t("connections.refreshConnections")}
            data-relay-tooltip={t("connections.refreshConnections")} aria-disabled={refreshing}
            onClick={() => { if (!refreshing) refresh(); }}><RefreshCw aria-hidden="true" /></button>
        </div>
      </div>
    </section>
    {added && <p className="form-note" role="status">{t("copy.committed")}</p>}
    {lifecycle.result}
    {failed && <div className="source-serving"><small role="alert">{t("unavailable.server")}</small>
      <button className="key-button key-secondary" type="button" aria-disabled={refreshing}
        onClick={() => { if (!refreshing) refresh(); }}>{t("common.retry")}</button></div>}
    {page && visible.length > 0 ? <div className="owner-resource-grid">{visible.map(item =>
      <SavedConnectionCard key={item.id} source={item} blocked={lifecycle.blocked || lifecycle.waiting(item)}
        webChecks={session.webModelChecks}
        refreshOnly={lifecycle.waiting(item) || !!item.serving?.draining}
        refresh={refresh} change={action => lifecycle.select(item, action)}
        models={() => setOverlay({ section: "models", id: item.id })} purchase={() => setOverlay({ section: "purchase", id: item.id })} />
    )}</div> : !failed && <p className="owner-resource-empty" role="status">{t(!page ? "unavailable.connecting" :
      paged ? "connections.noResultsOnPage" : search || filter !== "all" ? "connections.noConnectionsFound"
        : tab === "trash" ? "connections.trashIsEmpty" : "connections.noSavedConnections")}</p>}
    {paged && <div className="catalog-actions saved-connections-pager">
      {after && <button className="key-button key-secondary" type="button" onClick={() => paginate(cursors.slice(0, -1))}>{t("pagination.previousPage")}</button>}
      {page?.next_after && <button className="key-button key-secondary" type="button" onClick={() => paginate([...cursors, page.next_after!])}>{t("pagination.nextPage")}</button>}
    </div>}
    {overlay?.section === "desktop" && <Dialog className="owner-source-dialog" title={t("connections.desktop")} onClose={close}>
      <Suspense fallback={<small role="status">{t("unavailable.connecting")}</small>}>
        <DesktopPoolReview csrf={session.csrf} onApplied={refresh} />
      </Suspense>
      {location.protocol === "https:" && <DesktopSourceCopy csrf={session.csrf} onCommitted={refresh} />}
      <PairedDevices csrf={session.csrf} />
    </Dialog>}
    {source && overlay?.section === "models" && <ConnectionModels source={source} csrf={session.csrf} onClose={close}
      webChecks={session.webModelChecks} canCheck={session.mode === "qualification" && !failed} refresh={refresh} />}
    {source && overlay?.section === "purchase" && <Dialog className="owner-source-dialog" title={t("purchaseLimit.title")} onClose={close}>
      <div className="catalog-identity">{source.label && <strong>{source.label}</strong>}<small><code>{source.id}</code></small></div>
      <SourcePurchaseLimit source={source} session={session} />
    </Dialog>}
    {lifecycle.dialog}
  </div>;
}
