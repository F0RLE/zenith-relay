import { useEffect, useState } from "react";
import type { HostedSnapshot } from "../app/data";
import type { OwnerPage } from "../app/routes";
import { ownerPages } from "../app/routes";
import { ModelsPage } from "../models/ModelsPage";
import { UsagePage } from "../usage/UsagePage";
import { RequestDetails } from "../usage/RequestDetails";
import type { RequestRecord } from "../usage/model";
import { OverviewPage } from "../overview/OverviewPage";
import { ConnectionsPage, PoolPage } from "./OwnerResources";
import { OwnerNotificationsPage } from "./OwnerNotificationsPage";
import { updateConnections } from "./model";
import type { ConnectionAction } from "./model";
import { projectOwnerCatalog } from "./modelRules";
import type { OwnerCatalogSettings } from "./modelRules";
import { useLocale } from "../../i18n/LocaleProvider";
import "./owner-workspace.css";

export function OwnerWorkspace({ page, snapshot, catalogSettings, onCatalogSettingsChange }: {
  page: OwnerPage; snapshot?: HostedSnapshot; catalogSettings: OwnerCatalogSettings; onCatalogSettingsChange: (settings: OwnerCatalogSettings) => void;
}) {
  const { t } = useLocale();
  const [resources, setResources] = useState(() => ({ connections: snapshot?.owner.connections ?? [], events: snapshot?.owner.events ?? [] }));
  const { connections } = resources;
  const [proxies, setProxies] = useState(() => snapshot?.owner.proxies ?? []);
  const [request, setRequest] = useState<RequestRecord | null>(null);
  useEffect(() => { setRequest(null); }, [page]);
  function change(action: ConnectionAction) {
    if (!snapshot) return;
    setResources(current => {
      const next = updateConnections(current.connections, action);
      if (next === current.connections) return current;
      const names: Record<ConnectionAction["type"], string> = { add: "Connection added", addMembers: "Pool members added", saveSource: "API source configured", saveMember: "Pool member configured", pool: "Pool membership changed", pause: "Availability changed", trash: "Moved to trash", restore: "Restored outside pool", move: "Order changed" };
      const target = action.type === "add" ? action.connection.name : action.type === "saveSource" ? action.configuration.name : action.type === "addMembers" ? current.connections.filter(item => action.ids.includes(item.id) && !item.inPool && !item.trashed).map(item => item.name).join(", ") : current.connections.find(item => item.id === action.id)?.name ?? "Connection";
      return { connections: next, events: [{ id: crypto.randomUUID(), at: snapshot.asOf, action: names[action.type], target }, ...current.events] };
    });
  }
  const title = t(ownerPages.find(item => item.id === page)?.name ?? "owner.manageHosted");
  if (!snapshot) return <><header className="page-heading"><h1>{title}</h1></header><div className="hosted-empty"><h2>{t("shell.hostedApiIsNotConnected")}</h2><p>{t("owner.unavailable")}</p></div></>;
  return <>
    <div hidden={page !== "overview"}><OverviewPage records={snapshot.requests} asOf={snapshot.asOf} owner /></div>
    <div hidden={page !== "connections"}><ConnectionsPage connections={connections} models={snapshot.models} change={change} active={page === "connections"} asOf={snapshot.asOf} proxies={proxies} onProxiesChange={setProxies} /></div>
    <div hidden={page !== "pools"}><PoolPage connections={connections} models={snapshot.models} change={change} active={page === "pools"} asOf={snapshot.asOf} poolActivity={snapshot.owner.poolActivity} modelRules={catalogSettings.rules} onModelRulesChange={rules => onCatalogSettingsChange({ ...catalogSettings, rules })} modelDisplayOrder={catalogSettings.modelDisplayOrder} onModelOrderChange={modelDisplayOrder => onCatalogSettingsChange({ ...catalogSettings, modelDisplayOrder })} /></div>
    <div hidden={page !== "models"}><ModelsPage models={projectOwnerCatalog(snapshot.models, catalogSettings)} /></div>
    <div hidden={page !== "usage"}><UsagePage records={snapshot.requests} asOf={snapshot.asOf} onSelectRequest={setRequest} /></div>
    <div hidden={page !== "notifications"}><OwnerNotificationsPage /></div>
    {request && <RequestDetails record={request} onClose={() => setRequest(null)} />}
  </>;
}
