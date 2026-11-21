import { useEffect, useRef, useState, type ReactNode } from "react";
import { RefreshCw } from "lucide-react";
import { useTranslation } from "react-i18next";
import { preparePoolChange, readConnections, readOverview, readUsage } from "../../api/hostedDevice";
import type {
  HostedDevicePage, HostedDeviceReview, HostedOverview, HostedPoolChange, HostedUsage, HostedUsageFilter,
} from "../../api/types/hostedDevice";
import { Button, IconButton, PageHeader } from "../../components/Ui";
import { HostedConnections } from "./HostedConnections";
import { HostedOverviewPanel, HostedUsagePanel } from "./HostedObservations";
import { hostedErrorKey } from "./hostedErrors";

export type HostedView = "overview" | "connections" | "pool" | "usage";
type Snapshot =
  | { kind: "overview"; value: HostedOverview }
  | { kind: "usage"; value: HostedUsage }
  | { kind: "inventory"; value: HostedDevicePage };
type Selection = Pick<HostedUsageFilter, "hours" | "model" | "execution">;
type Query = { filter: Selection; cursors: string[]; anchor: number | null };
const initialQuery: Query = { filter: { hours: 24, model: null, execution: null }, cursors: [], anchor: null };

export function HostedPageFrame({ page, origin, refresh, children }: {
  page: HostedView; origin?: string | undefined; refresh?: { busy: boolean; run: () => void }; children: ReactNode;
}) {
  const { t } = useTranslation();
  return <section className="relay-page relay-workspace-page hosted-page">
    <PageHeader workspace title={t(`nav.${page}`)} actions={origin && <>
      <span className="hosted-active-server"><span className="sr-only">{t("hosted.server")}: </span>{origin}</span>
      {refresh && <IconButton label={t("common.refresh")} icon={<RefreshCw aria-hidden />} busy={refresh.busy} onClick={refresh.run} />}
    </>} />
    <div className="hosted-device-pane">{children}</div>
  </section>;
}

export function HostedDataPage({ page, device, expires, onReadFailure, children }: {
  page: HostedView; device: HostedDeviceReview; expires: number; onReadFailure: (error: unknown) => void; children?: ReactNode;
}) {
  const { t } = useTranslation();
  const [query, setQuery] = useState(initialQuery);
  const [settled, setSettled] = useState<{ query: Query; data: Snapshot } | { query: Query; error: unknown }>();
  const [model, setModel] = useState("");
  const [focusReview, setFocusReview] = useState<string>();
  const [preparing, setPreparing] = useState(false);
  const reviewInFlight = useRef(false);
  const mounted = useRef(false);
  const { invitation, destination } = device;
  const current = settled?.query === query ? settled : undefined;
  const data = current && "data" in current ? current.data : undefined;
  const inventory = data?.kind === "inventory" ? data.value : undefined;
  const usage = data?.kind === "usage" ? data.value : undefined;
  const busy = !current || preparing;

  useEffect(() => {
    mounted.current = true;
    return () => { mounted.current = false; };
  }, []);

  useEffect(() => {
    let active = true;
    const accept = (reply: { expires_at_s: number }, data: Snapshot) => {
      if (!active) return;
      if (Math.min(expires, reply.expires_at_s * 1000) <= Date.now()) throw "device_not_authorized";
      setSettled({ query, data });
    };
    async function load() {
      const after = query.cursors.at(-1);
      switch (page) {
        case "overview": {
          const reply = await readOverview(invitation, query.filter.hours);
          accept(reply, { kind: page, value: reply.result });
          break;
        }
        case "usage": {
          const reply = await readUsage(invitation, { ...query.filter, as_of_ms: query.anchor, before: after ?? null });
          accept(reply, { kind: page, value: reply.result });
          break;
        }
        default: {
          const reply = await readConnections(invitation, after);
          accept(reply, { kind: "inventory", value: reply });
        }
      }
    }
    queueMicrotask(() => {
      if (!active) return;
      void load().catch(error => {
        if (active) { onReadFailure(error); setSettled({ query, error }); }
      });
    });
    return () => { active = false; };
  }, [page, invitation, expires, query, onReadFailure]);

  async function review(change: HostedPoolChange) {
    if (reviewInFlight.current || busy || page !== "pool" || !inventory?.pool_access) return;
    reviewInFlight.current = true;
    setPreparing(true);
    try {
      const result = await preparePoolChange(invitation, change);
      if (!mounted.current) return;
      if (Date.now() >= expires) throw "device_not_authorized";
      setSettled({ query, data: { kind: "inventory", value: { ...inventory, pool_review: result } } });
      setFocusReview(result.operation_id);
    } catch (error) {
      if (mounted.current) { onReadFailure(error); setSettled({ query, error }); }
    } finally {
      reviewInFlight.current = false;
      setPreparing(false);
    }
  }

  const changeFilter = (filter: Selection) => setQuery({ filter, cursors: [], anchor: null });
  const pageRows = (cursor: string | null, anchor: number | null = null) => setQuery({
    ...query, anchor, cursors: cursor ? [...query.cursors, cursor] : query.cursors.slice(0, -1),
  });
  return <HostedPageFrame page={page} origin={destination.origin} refresh={{ busy, run: () => {
    setFocusReview(undefined);
    changeFilter(query.filter);
  } }}>
    {page === "overview" && <HostedOverviewPanel hours={query.filter.hours} busy={busy}
      value={data?.kind === "overview" ? data.value : undefined} onChange={hours => changeFilter({ ...query.filter, hours })} />}
    {page === "usage" && <HostedUsagePanel value={usage} filter={query.filter} model={model} setModel={setModel} busy={busy}
      onChange={changeFilter} canGoBack={query.cursors.length > 0}
      onPage={next => { if (usage && (!next || usage.next_before)) pageRows(next ? usage.next_before : null, usage.as_of_ms); }} />}
    {inventory && (page === "pool" && inventory.pool_access === undefined ? <p>{t("hostedPool.unavailable")}</p> : <>
      <HostedConnections page={inventory} busy={busy} poolView={page === "pool"} focusReview={focusReview}
        onReview={change => void review(change)} />
      {(query.cursors.length > 0 || inventory.next_after) && <div className="hosted-device-actions">
        {query.cursors.length > 0 && <Button disabled={busy} onClick={() => pageRows(null)}>{t("common.back")}</Button>}
        {inventory.next_after && <Button disabled={busy} onClick={() => pageRows(inventory.next_after)}>{t("hosted.next")}</Button>}
      </div>}
    </>)}
    {busy && <p className="hosted-read-state" role="status">{t("common.loading")}</p>}
    {current && "error" in current && <p className="hosted-read-state error-text" role="alert">{t(hostedErrorKey(current.error))}</p>}
    {children}
  </HostedPageFrame>;
}
