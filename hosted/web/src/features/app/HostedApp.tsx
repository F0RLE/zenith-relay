import { useEffect, useRef, useState } from "react";
import { Activity, Bell, Cable, KeyRound, Layers2, LayoutDashboard, LogOut, Menu, Models, Radio, Wallet, X } from "../../components/Icons";
import { Dialog } from "../../components/Dialog";
import { HostedTooltips } from "../../components/HostedTooltips";
import { HeaderLanguageSwitcher } from "../../components/HeaderLanguageSwitcher";
import { RelayMark } from "../../components/RelayMark";
import { useTheme } from "../../components/useTheme";
import { ThemeIcon } from "../../components/ThemeIcon";
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
import { customerPages, hostedPageUrl, ownerPages, readHostedRoute } from "./routes";
import type { HostedPage, OwnerPage } from "./routes";
import { KeysPage } from "../keys/KeysPage";
import { OwnerWorkspace } from "../owner/OwnerWorkspace";
import { projectOwnerCatalog } from "../owner/modelRules";
import type { OwnerCatalogSettings } from "../owner/modelRules";
import { useNavigationDrawer } from "./useNavigationDrawer";
import "./hosted.css";
import "./desktop.css";
import "./workspace-theme.css";
import "./workspace-content.css";
import "./workspace-menus.css";
import "../overview/overview-charts.css";
import "../billing/billing-history.css";
import "../usage/usage-lists.css";
import "../owner/owner-resource-cards.css";

const icons = { overview: LayoutDashboard, usage: Activity, models: Models, keys: KeyRound, billing: Wallet, status: Radio, connections: Cable, pools: Layers2, notifications: Bell };
type PanelId = "signOut" | "notifications";

export function HostedApp({ snapshot, catalogController }: { snapshot?: HostedSnapshot; catalogController?: { settings: OwnerCatalogSettings; onChange: (settings: OwnerCatalogSettings) => void } }) {
  const { t, locale } = useLocale();
  const { formatMoney } = useFormatters();
  const records = snapshot?.requests ?? [];
  const [localCatalogSettings, setLocalCatalogSettings] = useState<OwnerCatalogSettings>(() => ({ rules: snapshot?.owner.modelRules ?? {}, modelDisplayOrder: snapshot?.owner.modelDisplayOrder ?? [] }));
  const catalogSettings = catalogController?.settings ?? localCatalogSettings;
  const changeCatalogSettings = catalogController?.onChange ?? setLocalCatalogSettings;
  const catalog = projectOwnerCatalog(snapshot?.models ?? [], catalogSettings);
  const asOf = snapshot?.asOf ?? "";
  const { theme, toggleTheme } = useTheme();
  const { open: menu, setOpen: setMenu, mobile, sidebar, trigger: menuTrigger } = useNavigationDrawer();
  const [collapsed, setCollapsed] = useState(() => matchMedia("(max-width: 1023px)").matches);
  const sidebarPreference = useRef<boolean | null>(null);
  const content = useRef<HTMLDivElement>(null);
  useEffect(() => {
    const compact = matchMedia("(max-width: 1023px)");
    const resize = () => setCollapsed(sidebarPreference.current ?? compact.matches);
    compact.addEventListener("change", resize);
    return () => compact.removeEventListener("change", resize);
  }, []);
  const [route, setRoute] = useState(() => readHostedRoute(new URL(location.href)));
  const { area, page } = route;
  const owner = area === "owner";
  const pages = owner ? ownerPages : customerPages;
  const [panel, setPanel] = useState<PanelId | null>(null);
  const [selected, setSelected] = useState<RequestRecord | null>(null);
  const [balance, setBalance] = useState(snapshot?.balance ?? null);
  const [read, setRead] = useState(false);
  const [languageOpen, setLanguageOpen] = useState(false);
  const [signedOut, setSignedOut] = useState(false);
  const notificationTrigger = useRef<HTMLButtonElement>(null);
  const [operations, setOperations] = useState(snapshot?.payments ?? []);
  const [keys, setKeys] = useState(snapshot?.keys ?? []);
  const pending = records.find(record => record.accounting === "pending");
  useEffect(() => { content.current?.scrollTo(0, 0); setLanguageOpen(false); }, [page, area]);
  useEffect(() => {
    const pop = () => { setRoute(readHostedRoute(new URL(location.href))); setMenu(false); setPanel(null); setSelected(null); };
    window.addEventListener("popstate", pop);
    return () => window.removeEventListener("popstate", pop);
  }, []);
  useEffect(() => {
    document.title = `${t(pages.find(item => item.id === page)?.name ?? "shell.pageNotFound")} · Relay Hosted${owner ? ` · ${t("nav.owner")}` : ""}`;
    if (location.pathname === "/app" || location.pathname === "/app/" || location.pathname === "/admin" || location.pathname === "/admin/") history.replaceState(null, "", hostedPageUrl(area, "overview", new URL(location.href)));
  }, [area, owner, page, pages, t]);
  function navigate(next: HostedPage) {
    if (!pages.some(item => item.id === next)) return;
    setRoute({ area, page: next, found: true }); setMenu(false); setPanel(null); setSelected(null);
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
  function toggleSidebar() {
    if (mobile) { setMenu(false); return; }
    sidebarPreference.current = !collapsed;
    setCollapsed(!collapsed);
  }
  if (signedOut) return <div className="relay-app hosted-app" data-theme={theme} style={{ colorScheme: theme }}><main className="demo-signed-out hosted-state">
    <a href="/" className="hosted-state-brand" aria-label={t("shell.relayHome")}><RelayMark className="hosted-brand-mark" /><strong>Relay</strong></a><h1>{t("shell.previewEnded")}</h1>
    <div className="button-row"><button type="button" className="button primary-button" onClick={() => location.reload()}>{t("shell.returnToDashboard")}</button><a className="button" href="/">{t("shell.home")}</a></div>
  </main></div>;
  if (!route.found) return <div className="relay-app hosted-app" data-theme={theme} style={{ colorScheme: theme }}><main className="integration-unavailable hosted-state"><a href="/" className="hosted-state-brand" aria-label={t("shell.relayHome")}><RelayMark className="hosted-brand-mark" /><strong>Relay</strong></a><h1>{t("shell.pageNotFound")}</h1><div className="button-row"><a className="button primary-button" href={owner ? "/admin/overview" : "/app/overview"}>{t("shell.backToOverview")}</a><a className="button" href="/">{t("shell.home")}</a></div></main></div>;
  const navigationButton = ({ id, name }: (typeof pages)[number]) => {
    const Icon = icons[id];
    return <button key={id} type="button" className={page === id ? "active" : ""} aria-label={t(name)} data-relay-tooltip={collapsed && !mobile ? t(name) : undefined} aria-current={page === id ? "page" : undefined} onClick={() => navigate(id)}><Icon aria-hidden="true" /><span>{t(name)}</span></button>;
  };
  return <div className="relay-app hosted-app" data-theme={theme} style={{ colorScheme: theme }} lang={locale}>
    <HostedTooltips />
    <a className="skip-link" href="#hosted-content">{t("shell.skipToContent")}</a>
    <div className={`relay-shell ${collapsed && !mobile ? "sidebar-collapsed" : ""}`} data-mode="hosted" data-page={page}><aside ref={sidebar} className="relay-sidebar" data-open={menu} role={menu && mobile ? "dialog" : undefined} aria-modal={menu && mobile ? true : undefined} aria-label={menu && mobile ? t("shell.navigation") : undefined}>
      <div className="mode-picker"><button type="button" className="hosted-mode" aria-label={t(collapsed && !mobile ? "shell.expandMenu" : "shell.collapseMenu")} aria-expanded={mobile ? menu : !collapsed} data-relay-tooltip={mobile ? undefined : t(collapsed ? "shell.expandMenu" : "shell.collapseMenu")} onClick={toggleSidebar}><RelayMark className="hosted-brand-mark" /><span className="mode-picker-label"><strong>Relay</strong><small>Hosted</small></span></button></div>
      {menu && mobile && <button type="button" className="icon-button drawer-close" aria-label={t("shell.closeMenu")} onClick={() => setMenu(false)}><X aria-hidden="true" /></button>}
      <div className="sidebar-navigation"><nav className="sidebar-main-nav" aria-label={t("shell.sections")}>
        <div className="sidebar-nav-group">{pages.filter(item => item.id !== "keys" && item.id !== "billing").map(navigationButton)}</div>
        {!owner && <div className="sidebar-nav-group sidebar-account-nav">{customerPages.filter(item => item.id === "keys" || item.id === "billing").map(navigationButton)}</div>}
      </nav></div>
      <div className="sidebar-bottom">
        <footer className="sidebar-footer"><nav className="sidebar-nav-utilities" aria-label={t("common.account")}><button type="button" data-relay-tooltip={collapsed && !mobile ? t("shell.signOut") : undefined} aria-label={t("shell.signOut")} onClick={() => setPanel("signOut")}><LogOut aria-hidden="true" /><span>{t("shell.signOut")}</span></button></nav></footer>
      </div>
    </aside>{menu && mobile && <div className="drawer-backdrop" onClick={() => setMenu(false)} aria-hidden="true" />}<div ref={content} className="relay-content" inert={menu && mobile}><header className="hosted-tools" data-language-open={languageOpen}>
      <button ref={menuTrigger} type="button" className="icon-button mobile-menu" aria-label={t("shell.menu")} aria-expanded={menu} onClick={() => setMenu(!menu)}><Menu aria-hidden="true" /></button>
      {!owner && <button type="button" className="hosted-balance" aria-label={balance === null ? t("shell.balanceUnavailable") : t("shell.balanceAmount", { amount: formatMoney(balance, 2) })} onClick={() => navigate("billing")}><Wallet aria-hidden="true" /><span>{balance === null ? "—" : `$${formatMoney(balance, 2)}`}</span></button>}
      <button ref={notificationTrigger} type="button" className="icon-button notification-button" data-language-companion inert={languageOpen} aria-hidden={languageOpen || undefined} data-unread={!!pending && !read} aria-label={t(!pending || read ? "shell.notificationsNoNewNotifications" : "shell.notificationsOneNewNotification")} aria-haspopup="dialog" aria-expanded={panel === "notifications"} aria-controls={panel === "notifications" ? "notifications-panel" : undefined} onClick={() => setPanel(current => current === "notifications" ? null : "notifications")}><Bell aria-hidden="true" /></button>
      <HeaderLanguageSwitcher open={languageOpen} onOpenChange={next => { setLanguageOpen(next); if (next) setPanel(current => current === "notifications" ? null : current); }} />
      <button type="button" className="icon-button" data-theme-toggle data-language-companion inert={languageOpen} aria-hidden={languageOpen || undefined} aria-label={t(theme === "light" ? "theme.dark" : "theme.light")} data-relay-tooltip={t(theme === "light" ? "theme.dark" : "theme.light")} onClick={toggleTheme}><ThemeIcon theme={theme} /></button>
    </header><main className="relay-page" id="hosted-content" tabIndex={-1}>
      {owner ? <OwnerWorkspace page={page as OwnerPage} catalogSettings={catalogSettings} onCatalogSettingsChange={changeCatalogSettings} {...(snapshot ? { snapshot } : {})} /> : <>
      {snapshot && <><div hidden={page !== "overview"}><OverviewPage records={records} asOf={asOf} /></div>
      <div hidden={page !== "usage"}><UsagePage records={records} asOf={asOf} onSelectRequest={setSelected} /></div>
      <div hidden={page !== "models"}><ModelsPage models={catalog} /></div>
      <div hidden={page !== "billing"}><BillingPage balance={balance!} operations={operations} paymentMethods={snapshot.paymentMethods} asOf={asOf} active={page === "billing"} onTopUp={topUp} /></div></>}
      {page === "keys" && <KeysPage keys={keys} models={catalog} {...(snapshot ? { onChange: setKeys } : {})} />}
      {page === "status" && <StatusPage models={catalog} asOf={asOf} />}
      {!snapshot && page !== "keys" && page !== "status" && <><header className="page-heading"><h1>{t(pages.find(item => item.id === page)!.name)}</h1></header><div className="hosted-empty"><h2>{t("shell.hostedApiIsNotConnected")}</h2><p>{t("shell.unavailableDetails")}</p></div></>}
      </>}
    </main></div></div>
    {panel === "signOut" && <Dialog title={t("shell.signOutOfYourAccount")} onClose={() => setPanel(null)} className="sign-out-dialog">
      <p>{t("shell.signOutDetails")}</p>
      {!snapshot && <p className="form-note">{t("shell.noSession")}</p>}
      <div className="button-row"><button type="button" className="button" data-initial-focus onClick={() => setPanel(null)}>{t("shell.stay")}</button>
        <button type="button" className="button danger-button" onClick={() => { setPanel(null); setMenu(false); setSignedOut(true); }}>{t("shell.signOut")}</button></div>
    </Dialog>}
    {panel === "notifications" && pending && <NotificationPanel anchor={notificationTrigger} read={read} modelName={pending.model.name} onRead={() => setRead(true)} onClose={() => setPanel(null)} onOpenRequest={() => { setRead(true); setPanel(null); navigate("usage"); setSelected(pending); }} />}
    {panel === "notifications" && !pending && <Dialog title={t("shell.notifications")} onClose={() => setPanel(null)}><p>{t("notifications.unavailable")}</p></Dialog>}
    {selected && <RequestDetails key={selected.id} record={selected} onClose={() => setSelected(null)} />}
  </div>;
}
