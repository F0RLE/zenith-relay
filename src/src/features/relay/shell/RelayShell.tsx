import { Activity, Cable, ChevronDown, CircleHelp, Gauge, LayoutDashboard, PanelLeftClose, PanelLeftOpen, Server, Settings, SlidersHorizontal, UserRoundCog } from "lucide-react";
import { useState } from "react";
import { useTranslation } from "react-i18next";
import type { PageId, RelayMode } from "../api/types";
import { OverviewPage } from "../pages/overview/OverviewPage";
import { ConnectionsPage } from "../pages/connections/ConnectionsPage";
import { PoolPage } from "../pages/pool/PoolPage";
import { GatewayPage } from "../pages/gateway/GatewayPage";
import { UsagePage } from "../pages/usage/UsagePage";
import { ProfilesPage } from "../pages/profiles/ProfilesPage";
import { SettingsPage } from "../pages/settings/SettingsPage";
import { useRelayState } from "../state/RelayStateProvider";
import { IconButton } from "../components/Ui";

const pages: Array<{ id: PageId; icon: typeof LayoutDashboard }> = [
  { id: "overview", icon: LayoutDashboard }, { id: "connections", icon: Cable }, { id: "pool", icon: SlidersHorizontal }, { id: "gateway", icon: Gauge }, { id: "usage", icon: Activity }, { id: "profiles", icon: UserRoundCog }, { id: "settings", icon: Settings },
];

export function RelayShell() {
  const { t } = useTranslation(); const { mode, setMode, page, setPage, feedback, clearFeedback, loading } = useRelayState(); const [collapsed, setCollapsed] = useState(false); const [modeOpen, setModeOpen] = useState(false);
  const visiblePages = pages.filter((item) => !(item.id === "pool" && mode === "zenith"));
  return <div className={`relay-shell ${collapsed ? "sidebar-collapsed" : ""}`}><aside className="relay-sidebar"><div className="mode-picker"><button type="button" aria-label={t(`modes.${mode}`)} title={t(`modes.${mode}`)} aria-haspopup="menu" aria-expanded={modeOpen} onClick={() => setModeOpen((value) => !value)}><ModeIcon mode={mode} /><span>{t(`modes.${mode}`)}</span><ChevronDown aria-hidden /></button>{modeOpen ? <div className="mode-menu" role="menu">{(["local","remote","zenith"] as RelayMode[]).map((value) => <button role="menuitem" key={value} type="button" onClick={() => { setMode(value); setModeOpen(false); }}><ModeIcon mode={value} /><span>{t(`modes.${value}`)}</span></button>)}</div> : null}</div><nav aria-label={t("nav.label")}>{visiblePages.map(({id,icon:Icon}) => <button key={id} type="button" className={page === id ? "active" : ""} aria-label={t(`nav.${id}`)} aria-current={page === id ? "page" : undefined} title={t(`nav.${id}`)} onClick={() => setPage(id)}><Icon aria-hidden /><span>{t(`nav.${id}`)}</span></button>)}</nav><div className="sidebar-footer"><button type="button" aria-label={t("common.help")} title={t("common.help")}><CircleHelp aria-hidden /><span>{t("common.help")}</span></button><small>v1.0.5</small><IconButton label={collapsed ? t("shell.expand") : t("shell.collapse")} icon={collapsed ? <PanelLeftOpen aria-hidden /> : <PanelLeftClose aria-hidden />} onClick={() => setCollapsed((value) => !value)} /></div></aside><div className="relay-content">{feedback ? <div className={`global-feedback ${feedback.kind}`} role="status"><span>{t(feedback.key)}</span><button type="button" onClick={clearFeedback}>×</button></div> : null}{loading ? <div className="relay-loading">{t("common.loading")}</div> : <Page page={page} />}</div></div>;
}

function ModeIcon({ mode }: { mode: RelayMode }) { return mode === "local" ? <LayoutDashboard aria-hidden /> : mode === "remote" ? <Server aria-hidden /> : <Gauge aria-hidden />; }
function Page({ page }: { page: PageId }) { if (page === "overview") return <OverviewPage />; if (page === "connections") return <ConnectionsPage />; if (page === "pool") return <PoolPage />; if (page === "gateway") return <GatewayPage />; if (page === "usage") return <UsagePage />; if (page === "profiles") return <ProfilesPage />; return <SettingsPage />; }
