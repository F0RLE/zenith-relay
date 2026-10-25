import { useEffect, useRef, useState } from "react";
import { Activity, Bell, Layers2, LayoutDashboard, LogOut, Menu, Moon, SlidersHorizontal, Sun, Wallet } from "lucide-react";
import { Dialog } from "../components/Dialog";
import { ProfileMenu } from "../components/ProfileMenu";
import { BillingPage } from "../features/billing/BillingPage";
import type { PaymentOperation } from "../features/billing/BillingPage";
import { HelpPanel } from "../features/help/HelpPanel";
import { ModelsPage } from "../features/models/ModelsPage";
import { NotificationPanel } from "../features/notifications/NotificationPanel";
import { OverviewPage } from "../features/overview/OverviewPage";
import { RequestDetails } from "../features/usage/RequestDetails";
import { UsagePage } from "../features/usage/UsagePage";
import { formatMoney } from "../features/usage/model";
import type { RequestRecord } from "../features/usage/model";
import { previewRequests } from "./fixtures";

const pages = [
  { id: "overview", name: "Обзор", icon: LayoutDashboard },
  { id: "usage", name: "Использование", icon: Activity },
  { id: "models", name: "Модели", icon: SlidersHorizontal },
  { id: "billing", name: "Баланс", icon: Wallet },
] as const;
type PageId = (typeof pages)[number]["id"];
type PanelId = "signOut" | "help" | "notifications";
const panelTitles: Record<PanelId, string> = { signOut: "Выйти из аккаунта?", help: "Подключение и помощь", notifications: "Уведомления" };
const asOf = "2026-10-20T14:32:00Z";
const catalog = [...new Map(previewRequests.map(record => [record.model.id, { id: record.model.id, name: record.model.name, protocols: [record.protocol], rates: record.billing.rates, revision: record.billing.revision, description: "Демонстрационная модель для проверки каталога и подключения клиента. Возможности реальной модели определит Hosted API." }])).values()];
function pageFromUrl(): PageId {
  const candidate = new URL(location.href).searchParams.get("page");
  return pages.find(page => page.id === candidate)?.id ?? "overview";
}

export function PreviewApp() {
  const [theme, setTheme] = useState<"light" | "dark">(matchMedia("(prefers-color-scheme: dark)").matches ? "dark" : "light");
  const [menu, setMenu] = useState(false);
  const [page, setPage] = useState<PageId>(pageFromUrl);
  const [panel, setPanel] = useState<PanelId | null>(null);
  const [selected, setSelected] = useState<RequestRecord | null>(null);
  const [balance, setBalance] = useState(42800000n);
  const [read, setRead] = useState(false);
  const [signedOut, setSignedOut] = useState(false);
  const notificationTrigger = useRef<HTMLButtonElement>(null);
  const [operations, setOperations] = useState<PaymentOperation[]>([
    { id: "demo_payment_0003", at: "2026-10-20T14:05:00Z", kind: "Пополнение", amount: 25000000n, status: "Зачислено · демо" },
    { id: "demo_payment_0002", at: "2026-10-19T11:40:00Z", kind: "Возврат платежа", amount: -5000000n, status: "Возвращено · демо" },
    { id: "demo_payment_0001", at: "2026-10-17T10:05:00Z", kind: "Пополнение", amount: 20000000n, status: "Зачислено · демо" },
  ]);
  const pending = previewRequests.find(record => record.accounting === "pending")!;
  useEffect(() => {
    const pop = () => { setPage(pageFromUrl()); setMenu(false); setPanel(null); setSelected(null); };
    window.addEventListener("popstate", pop);
    return () => window.removeEventListener("popstate", pop);
  }, []);
  function navigate(next: PageId) {
    setPage(next); setMenu(false);
    const url = new URL(location.href); url.searchParams.set("page", next);
    history.pushState(null, "", url);
  }
  function topUp(amount: bigint) {
    setBalance(current => current + amount);
    setOperations(current => [{ id: `demo_payment_${current.length + 1}`, at: asOf, kind: "Демо-пополнение", amount, status: "Зачислено · только в этом окне" }, ...current]);
  }
  if (signedOut) return <div className="relay-app" style={{ colorScheme: theme }}><main className="demo-signed-out">
    <LogOut aria-hidden="true" /><h1>Вы вышли из демо</h1><p>Настоящая авторизация ещё не подключена.</p>
    <button type="button" className="button" onClick={() => location.reload()}>Вернуться в демо</button>
  </main></div>;
  return <div className="relay-app" style={{ colorScheme: theme }}>
    <div className="app-shell"><aside className="sidebar" data-open={menu}>
      <div className="brand"><Layers2 aria-hidden="true" /><strong>Relay</strong><span>Hosted</span></div>
      <nav aria-label="Разделы">{pages.map(({ id, name, icon: Icon }) => <button key={id} type="button" aria-current={page === id ? "page" : undefined} onClick={() => navigate(id)}><Icon aria-hidden="true" />{name}</button>)}</nav>
      <div className="sidebar-bottom">
        <ProfileMenu onSignOut={() => setPanel("signOut")} />
        <footer className="sidebar-footer"><span>v0.1.0-preview</span></footer>
      </div>
    </aside><div className="app-body"><header className="topbar">
      <button type="button" className="icon-button mobile-menu" aria-label="Меню" aria-expanded={menu} onClick={() => setMenu(!menu)}><Menu aria-hidden="true" /></button>
      <button type="button" className="topbar-balance" aria-label={`Баланс: ${formatMoney(balance, 2)} USD`} onClick={() => navigate("billing")}><span className="topbar-user">demo</span><span className="topbar-amount">${formatMoney(balance, 2)}</span></button>
      <button ref={notificationTrigger} type="button" className="icon-button notification-button" data-unread={!read} aria-label={`Уведомления: ${read ? "нет новых" : "одно новое"}`} aria-haspopup="dialog" aria-expanded={panel === "notifications"} aria-controls={panel === "notifications" ? "notifications-panel" : undefined} onClick={() => setPanel(current => current === "notifications" ? null : "notifications")}><Bell aria-hidden="true" /></button>
      <button type="button" className="icon-button" aria-label={theme === "light" ? "Тёмная тема" : "Светлая тема"} onClick={() => setTheme(theme === "light" ? "dark" : "light")}>{theme === "light" ? <Moon aria-hidden="true" /> : <Sun aria-hidden="true" />}</button>
    </header><main className="page">
      <div hidden={page !== "overview"}><OverviewPage records={previewRequests} asOf={asOf} /></div>
      <div hidden={page !== "usage"}><UsagePage records={previewRequests} asOf={asOf} onSelectRequest={setSelected} /></div>
      <div hidden={page !== "models"}><ModelsPage models={catalog} onHelp={() => setPanel("help")} /></div>
      <div hidden={page !== "billing"}><BillingPage balance={balance} reserved={180000n} operations={operations} onTopUp={topUp} /></div>
      {page !== "overview" && <footer className="preview-note"><span>Демо · вымышленные данные и тарифы</span><span>Изменения сохраняются до перезагрузки</span></footer>}
    </main></div></div>
    {panel && panel !== "notifications" && <Dialog title={panelTitles[panel]} onClose={() => setPanel(null)} className={panel === "signOut" ? "sign-out-dialog" : ""}>
      {panel === "signOut" && <><p>Вы покинете кабинет. Для продолжения работы потребуется снова войти.</p>
        <p className="form-note">Это демо: настоящей серверной сессии нет.</p>
        <div className="button-row"><button type="button" className="button" data-initial-focus onClick={() => setPanel(null)}>Остаться</button>
          <button type="button" className="button danger-button" onClick={() => { setPanel(null); setSignedOut(true); }}>Выйти</button></div></>}
      {panel === "help" && <HelpPanel />}
    </Dialog>}
    {panel === "notifications" && <NotificationPanel anchor={notificationTrigger} read={read} modelName={pending.model.name} onRead={() => setRead(true)} onClose={() => setPanel(null)} onOpenRequest={() => { setRead(true); setPanel(null); navigate("usage"); setSelected(pending); }} />}
    {selected && <RequestDetails key={selected.id} record={selected} onClose={() => setSelected(null)} />}
  </div>;
}
