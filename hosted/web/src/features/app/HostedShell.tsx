import { useEffect, useRef, useState } from "react";
import type { ReactNode } from "react";
import { Activity, Bell, Cable, CreditCard, KeyRound, Layers2, LayoutDashboard, LogOut, Menu, Models, Radio, Wallet, X } from "../../components/Icons";
import { HeaderLanguageSwitcher } from "../../components/HeaderLanguageSwitcher";
import { HostedTooltips } from "../../components/HostedTooltips";
import { RelayMark } from "../../components/RelayMark";
import { ThemeIcon } from "../../components/ThemeIcon";
import type { useTheme } from "../../components/useTheme";
import { useLocale } from "../../i18n/LocaleProvider";
import { useNavigationDrawer } from "./useNavigationDrawer";
import "./hosted.css";
import "./desktop.css";
import "./workspace-theme.css";
import "./workspace-content.css";
import "./workspace-menus.css";

const icons = { overview: LayoutDashboard, usage: Activity, models: Models, keys: KeyRound, billing: Wallet, status: Radio, connections: Cable, pools: Layers2, notifications: Bell, "payment-methods": CreditCard };

export type HostedNavItem = {
  id: keyof typeof icons;
  name: Parameters<ReturnType<typeof useLocale>["t"]>[0];
  current?: "page" | "location" | undefined;
} & ({ href: string } | { onSelect: () => void });

function savedCollapse(key: string | undefined) {
  if (!key) return null;
  try {
    const value = sessionStorage.getItem(key);
    return value === null ? null : value === "true";
  } catch { return null; }
}

export function HostedShell({ page, brand = "Relay", navigation, accountNavigation = [], appearance,
  onSignOut, signingOut, serverWorkspace, beforeLanguage, onOpenLanguage, children, overlays }: {
  page: string;
  brand?: string;
  navigation: readonly HostedNavItem[];
  accountNavigation?: readonly HostedNavItem[];
  appearance: ReturnType<typeof useTheme>;
  onSignOut: () => void;
  signingOut?: boolean;
  serverWorkspace?: "customer" | "owner";
  beforeLanguage?: (languageOpen: boolean) => ReactNode;
  onOpenLanguage?: () => void;
  children: ReactNode;
  overlays?: ReactNode;
}) {
  const { t, locale } = useLocale();
  const { theme, toggleTheme } = appearance;
  const { open: menu, setOpen: setMenu, mobile, sidebar, trigger: menuTrigger } = useNavigationDrawer();
  const serverOwner = serverWorkspace === "owner";
  const preferenceKey = serverWorkspace ? `relay-hosted-${serverWorkspace}-sidebar-collapsed` : undefined;
  const [preference, setPreference] = useState(() => savedCollapse(preferenceKey));
  const [compact, setCompact] = useState(() => matchMedia("(max-width: 1023px)").matches);
  const [languageOpen, setLanguageOpen] = useState(false);
  const collapsed = preference ?? compact;
  const rail = collapsed && !mobile;
  const drawer = menu && mobile;
  const content = useRef<HTMLDivElement>(null);
  useEffect(() => {
    const media = matchMedia("(max-width: 1023px)");
    const resize = () => setCompact(media.matches);
    media.addEventListener("change", resize);
    return () => media.removeEventListener("change", resize);
  }, []);
  useEffect(() => { content.current?.scrollTo(0, 0); setLanguageOpen(false); setMenu(false); }, [page, setMenu]);

  function toggleSidebar() {
    if (mobile) { setMenu(false); return; }
    setPreference(!collapsed);
    if (preferenceKey) {
      try { sessionStorage.setItem(preferenceKey, String(!collapsed)); } catch { /* Storage is optional. */ }
    }
  }

  function navItem(item: HostedNavItem) {
    const Icon = icons[item.id];
    const label = t(item.name);
    const props = { className: item.current ? "active" : undefined, "aria-label": label,
      "aria-current": item.current, "data-relay-tooltip": rail ? label : undefined };
    const row = <><Icon aria-hidden="true" /><span>{label}</span></>;
    if ("href" in item) return <a key={item.id} {...props} href={item.href}>{row}</a>;
    return <button key={item.id} type="button" {...props} onClick={() => { setMenu(false); item.onSelect(); }}>{row}</button>;
  }

  const toggleLabel = t(mobile ? "shell.closeMenu" : rail ? "shell.expandMenu" : "shell.collapseMenu");
  return <div className={`relay-app hosted-app${serverOwner ? " hosted-entry" : ""}`} data-theme={theme}
    data-workspace={serverWorkspace} style={{ colorScheme: theme }} lang={locale}>
    <HostedTooltips />
    <a className="skip-link" href="#hosted-content" inert={drawer}>{t("shell.skipToContent")}</a>
    <div className={`relay-shell${rail ? " sidebar-collapsed" : ""}`} data-mode="hosted" data-page={page}>
      <aside ref={sidebar} id="hosted-sidebar" className="relay-sidebar" data-open={menu} role={drawer ? "dialog" : undefined} aria-modal={drawer || undefined} aria-label={drawer ? t("shell.navigation") : undefined}>
        <div className="mode-picker"><button type="button" className="hosted-mode" aria-label={toggleLabel} aria-expanded={mobile ? menu : !collapsed} data-relay-tooltip={mobile ? undefined : toggleLabel} onClick={toggleSidebar}><RelayMark className="hosted-brand-mark" /><span className="mode-picker-label"><strong>{brand}</strong><small>Hosted</small></span></button></div>
        {drawer && <button type="button" className="icon-button drawer-close" aria-label={t("shell.closeMenu")} onClick={() => setMenu(false)}><X aria-hidden="true" /></button>}
        <div className="sidebar-navigation"><nav className="sidebar-main-nav" aria-label={t("shell.sections")}>
          <div className="sidebar-nav-group">{navigation.map(navItem)}</div>
          {accountNavigation.length > 0 && <div className="sidebar-nav-group sidebar-account-nav">{accountNavigation.map(navItem)}</div>}
        </nav></div>
        <div className="sidebar-bottom"><footer className="sidebar-footer"><nav className="sidebar-nav-utilities" aria-label={t("common.account")}>
          <button type="button" disabled={signingOut} data-relay-tooltip={rail ? t("shell.signOut") : undefined} aria-label={t("shell.signOut")} onClick={() => { if (serverWorkspace) setMenu(false); onSignOut(); }}><LogOut aria-hidden="true" /><span>{t("shell.signOut")}</span></button>
        </nav></footer></div>
      </aside>
      {drawer && <div className="drawer-backdrop" onClick={() => setMenu(false)} aria-hidden="true" />}
      <div ref={content} className="relay-content" inert={drawer}>
        <header className="hosted-tools" data-language-open={languageOpen}>
          <button ref={menuTrigger} type="button" className="icon-button mobile-menu" aria-label={t("shell.menu")} aria-expanded={menu} aria-controls="hosted-sidebar" onClick={() => setMenu(!menu)}><Menu aria-hidden="true" /></button>
          {serverOwner && <span className="hosted-owner-role" data-language-companion inert={languageOpen} aria-hidden={languageOpen || undefined}>{t("nav.owner")}</span>}
          {beforeLanguage?.(languageOpen)}
          <HeaderLanguageSwitcher open={languageOpen} onOpenChange={next => { setLanguageOpen(next); if (next) onOpenLanguage?.(); }} />
          <button type="button" className="icon-button" data-theme-toggle data-language-companion inert={languageOpen} aria-hidden={languageOpen || undefined} aria-label={t(theme === "light" ? "theme.dark" : "theme.light")} data-relay-tooltip={t(theme === "light" ? "theme.dark" : "theme.light")} onClick={toggleTheme}><ThemeIcon theme={theme} /></button>
        </header>
        <main className={`relay-page${serverOwner ? " hosted-entry-content hosted-owner-page" : ""}`} id="hosted-content" tabIndex={-1}>{children}</main>
      </div>
    </div>
    {overlays}
  </div>;
}
