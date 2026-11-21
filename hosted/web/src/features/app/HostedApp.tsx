import { useEffect, useRef, useState } from "react";
import { Bell, Wallet } from "../../components/Icons";
import { Dialog } from "../../components/Dialog";
import { RelayMark } from "../../components/RelayMark";
import { useTheme } from "../../components/useTheme";
import { BillingPage } from "../billing/BillingPage";
import { isTopUpAmount } from "../billing/model";
import type { TopUpRequest } from "../billing/model";
import { ModelsPage } from "../models/ModelsPage";
import { NotificationPanel } from "../notifications/NotificationPanel";
import { OverviewPage } from "../overview/OverviewPage";
import { StatusPage } from "../status/StatusPage";
import { RequestDetails } from "../usage/RequestDetails";
import { UsagePage } from "../usage/UsagePage";
import { useFormatters } from "../../i18n/format";
import { useLocale } from "../../i18n/LocaleProvider";
import type { RequestRecord } from "../usage/model";
import type { HostedSnapshot } from "./data";
import { customerAccountPages, customerMainPages, customerPages, hostedPageUrl, ownerPages, readHostedRoute } from "./routes";
import type { HostedPage, OwnerPage } from "./routes";
import { KeysPage } from "../keys/KeysPage";
import { OwnerWorkspace } from "../owner/OwnerWorkspace";
import { projectOwnerCatalog } from "../owner/modelRules";
import type { OwnerCatalogSettings } from "../owner/modelRules";
import { HostedShell } from "./HostedShell";
import type { HostedNavItem } from "./HostedShell";
import "../overview/overview-charts.css";
import "../billing/billing-history.css";
import "../usage/usage-lists.css";
import "../owner/owner-resource-cards.css";

type PanelId = "signOut" | "notifications";

export function HostedApp({ snapshot, catalogController }: { snapshot?: HostedSnapshot; catalogController?: { settings: OwnerCatalogSettings; onChange: (settings: OwnerCatalogSettings) => void } }) {
  const { t } = useLocale();
  const { formatMoney } = useFormatters();
  const records = snapshot?.requests ?? [];
  const [localCatalogSettings, setLocalCatalogSettings] = useState<OwnerCatalogSettings>(() => ({ rules: snapshot?.owner.modelRules ?? {}, modelDisplayOrder: snapshot?.owner.modelDisplayOrder ?? [] }));
  const catalogSettings = catalogController?.settings ?? localCatalogSettings;
  const changeCatalogSettings = catalogController?.onChange ?? setLocalCatalogSettings;
  const catalog = projectOwnerCatalog(snapshot?.models ?? [], catalogSettings);
  const asOf = snapshot?.asOf ?? "";
  const appearance = useTheme();
  const { theme } = appearance;
  const [route, setRoute] = useState(() => readHostedRoute(new URL(location.href)));
  const { area, page } = route;
  const owner = area === "owner";
  const pages = owner ? ownerPages : customerPages;
  const [panel, setPanel] = useState<PanelId | null>(null);
  const [selected, setSelected] = useState<RequestRecord | null>(null);
  const [balance, setBalance] = useState(snapshot?.balance ?? null);
  const [read, setRead] = useState(false);
  const [signedOut, setSignedOut] = useState(false);
  const notificationTrigger = useRef<HTMLButtonElement>(null);
  const [operations, setOperations] = useState(snapshot?.payments ?? []);
  const [keys, setKeys] = useState(snapshot?.keys ?? []);
  const pending = records.find(record => record.accounting === "pending");
  useEffect(() => {
    const pop = () => { setRoute(readHostedRoute(new URL(location.href))); setPanel(null); setSelected(null); };
    window.addEventListener("popstate", pop);
    return () => window.removeEventListener("popstate", pop);
  }, []);
  useEffect(() => {
    document.title = `${t(pages.find(item => item.id === page)?.name ?? "shell.pageNotFound")} · Relay Hosted${owner ? ` · ${t("nav.owner")}` : ""}`;
    if (location.pathname === "/app" || location.pathname === "/app/" || location.pathname === "/admin" || location.pathname === "/admin/") history.replaceState(null, "", hostedPageUrl(area, "overview", new URL(location.href)));
  }, [area, owner, page, pages, t]);
  function navigate(next: HostedPage) {
    if (!pages.some(item => item.id === next)) return;
    setRoute({ area, page: next, found: true }); setPanel(null); setSelected(null);
    const url = hostedPageUrl(area, next, new URL(location.href));
    if (url.href === location.href) return;
    history.pushState(null, "", url);
  }
  function topUp({ microUsd, methodId }: TopUpRequest) {
    const method = snapshot?.paymentMethods.find(item => item.id === methodId);
    if (!snapshot || !method || !isTopUpAmount(microUsd)) return;
    setBalance(current => current === null ? null : current + microUsd);
    setOperations(current => [{ id: `demo_payment_${current.length + 1}`, at: asOf, kind: "billing.topUpOperation", amount: microUsd, method: method.label, status: "billing.credited" }, ...current]);
  }
  if (signedOut) return <div className="relay-app hosted-app" data-theme={theme} style={{ colorScheme: theme }}><main className="demo-signed-out hosted-state">
    <a href="/" className="hosted-state-brand" aria-label={t("shell.relayHome")}><RelayMark className="hosted-brand-mark" /><strong>Relay</strong></a><h1>{t("shell.previewEnded")}</h1>
    <div className="button-row"><button type="button" className="button primary-button" onClick={() => location.reload()}>{t("shell.returnToDashboard")}</button><a className="button" href="/">{t("shell.home")}</a></div>
  </main></div>;
  if (!route.found) return <div className="relay-app hosted-app" data-theme={theme} style={{ colorScheme: theme }}><main className="integration-unavailable hosted-state"><a href="/" className="hosted-state-brand" aria-label={t("shell.relayHome")}><RelayMark className="hosted-brand-mark" /><strong>Relay</strong></a><h1>{t("shell.pageNotFound")}</h1><div className="button-row"><a className="button primary-button" href={owner ? "/admin/overview" : "/app/overview"}>{t("shell.backToOverview")}</a><a className="button" href="/">{t("shell.home")}</a></div></main></div>;
  const navItem = ({ id, name }: (typeof pages)[number]): HostedNavItem => ({ id, name,
    current: page === id ? "page" : undefined, onSelect: () => navigate(id) });
  return <HostedShell key={area} appearance={appearance} page={page}
    navigation={(owner ? ownerPages : customerMainPages).map(navItem)}
    accountNavigation={owner ? [] : customerAccountPages.map(navItem)}
    onSignOut={() => setPanel("signOut")}
    onOpenLanguage={() => setPanel(current => current === "notifications" ? null : current)}
    beforeLanguage={languageOpen => <>
      {!owner && <button type="button" className="hosted-balance" aria-label={balance === null ? t("shell.balanceUnavailable") : t("shell.balanceAmount", { amount: formatMoney(balance, 2) })} onClick={() => navigate("billing")}><Wallet aria-hidden="true" /><span>{balance === null ? "—" : `$${formatMoney(balance, 2)}`}</span></button>}
      <button ref={notificationTrigger} type="button" className="icon-button notification-button" data-language-companion inert={languageOpen} aria-hidden={languageOpen || undefined} data-unread={!!pending && !read} aria-label={t(!pending || read ? "shell.notificationsNoNewNotifications" : "shell.notificationsOneNewNotification")} aria-haspopup="dialog" aria-expanded={panel === "notifications"} aria-controls={panel === "notifications" ? "notifications-panel" : undefined} onClick={() => setPanel(current => current === "notifications" ? null : "notifications")}><Bell aria-hidden="true" /></button>
    </>}
    overlays={<>
      {panel === "signOut" && <Dialog title={t("shell.signOutOfYourAccount")} onClose={() => setPanel(null)} className="sign-out-dialog">
        <p>{t("shell.signOutDetails")}</p>
        {!snapshot && <p className="form-note">{t("shell.noSession")}</p>}
        <div className="button-row"><button type="button" className="button" data-initial-focus onClick={() => setPanel(null)}>{t("shell.stay")}</button>
          <button type="button" className="button danger-button" onClick={() => { setPanel(null); setSignedOut(true); }}>{t("shell.signOut")}</button></div>
      </Dialog>}
      {panel === "notifications" && pending && <NotificationPanel anchor={notificationTrigger} read={read} modelName={pending.model.name} onRead={() => setRead(true)} onClose={() => setPanel(null)} onOpenRequest={() => { setRead(true); setPanel(null); navigate("usage"); setSelected(pending); }} />}
      {panel === "notifications" && !pending && <Dialog title={t("shell.notifications")} onClose={() => setPanel(null)}><p>{t("notifications.unavailable")}</p></Dialog>}
      {selected && <RequestDetails key={selected.id} record={selected} onClose={() => setSelected(null)} />}
    </>}>
      {owner ? <OwnerWorkspace page={page as OwnerPage} catalogSettings={catalogSettings} onCatalogSettingsChange={changeCatalogSettings} {...(snapshot ? { snapshot } : {})} /> : <>
      {snapshot && <><div hidden={page !== "overview"}><OverviewPage records={records} asOf={asOf} /></div>
      <div hidden={page !== "usage"}><UsagePage records={records} asOf={asOf} onSelectRequest={setSelected} /></div>
      <div hidden={page !== "models"}><ModelsPage models={catalog} /></div>
      <div hidden={page !== "billing"}><BillingPage balance={balance!} operations={operations} paymentMethods={snapshot.paymentMethods} asOf={asOf} active={page === "billing"} onTopUp={topUp} /></div></>}
      {page === "keys" && <KeysPage keys={keys} models={catalog} {...(snapshot ? { onChange: setKeys } : {})} />}
      {page === "status" && <StatusPage models={catalog} asOf={asOf} />}
      {!snapshot && page !== "keys" && page !== "status" && <><header className="page-heading"><h1>{t(pages.find(item => item.id === page)!.name)}</h1></header><div className="hosted-empty"><h2>{t("shell.hostedApiIsNotConnected")}</h2><p>{t("shell.unavailableDetails")}</p></div></>}
      </>}
  </HostedShell>;
}
