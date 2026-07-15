import { Activity, Cable, Check, ChevronDown, CircleHelp, Gauge, Laptop, LayoutDashboard, PanelLeftClose, PanelLeftOpen, Server, Settings, SlidersHorizontal, Upload, UserRoundCog, X } from "lucide-react";
import { getCurrentWebview } from "@tauri-apps/api/webview";
import { useCallback, useEffect, useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import type { PageId, RelayMode } from "../api/types";
import { OverviewPage } from "../pages/overview/OverviewPage";
import { ConnectionsPage, ImportDialog } from "../pages/connections/ConnectionsPage";
import { PoolPage } from "../pages/pool/PoolPage";
import { GatewayPage } from "../pages/gateway/GatewayPage";
import { UsagePage } from "../pages/usage/UsagePage";
import { ProfilesPage } from "../pages/profiles/ProfilesPage";
import { SettingsPage } from "../pages/settings/SettingsPage";
import { useRelayState } from "../state/RelayStateProvider";
import { IconButton } from "../components/Ui";

const pages: Array<{ id: PageId; icon: typeof LayoutDashboard }> = [
  { id: "overview", icon: LayoutDashboard },
  { id: "connections", icon: Cable },
  { id: "pool", icon: SlidersHorizontal },
  { id: "gateway", icon: Gauge },
  { id: "usage", icon: Activity },
  { id: "profiles", icon: UserRoundCog },
  { id: "settings", icon: Settings },
];

export function RelayShell() {
  const { t } = useTranslation();
  const { mode, setMode, page, setPage, feedback, clearFeedback, loading, resetOnboarding } = useRelayState();
  const [collapsed, setCollapsed] = useState(() => window.matchMedia?.("(max-width: 1023px)").matches ?? false);
  const [modeOpen, setModeOpen] = useState(false);
  const [importRequest, setImportRequest] = useState<{ id: number; paths?: string[] } | null>(null);
  const [importDragActive, setImportDragActive] = useState(false);
  const nextImportRequest = useRef(0);
  const modePickerRef = useRef<HTMLDivElement>(null);
  const contentRef = useRef<HTMLDivElement>(null);
  const visiblePages = pages.filter((item) => !(item.id === "pool" && mode === "zenith"));
  const openImport = useCallback((paths?: string[]) => {
    if (mode === "zenith") setMode("local");
    setPage("connections");
    setImportRequest({ id: ++nextImportRequest.current, paths });
  }, [mode, setMode, setPage]);

  useEffect(() => {
    if (contentRef.current) {
      contentRef.current.scrollTop = 0;
      contentRef.current.scrollLeft = 0;
    }
  }, [mode, page]);

  useEffect(() => {
    const closePopovers = (event: PointerEvent) => {
      const target = event.target as Node;
      if (!modePickerRef.current?.contains(target)) setModeOpen(false);
      document.querySelectorAll<HTMLDetailsElement>(".relay-action-menu[open]").forEach((menu) => {
        if (!menu.contains(target)) menu.open = false;
      });
    };
    const closeWithEscape = (event: KeyboardEvent) => {
      if (event.key !== "Escape") return;
      setModeOpen(false);
      document.querySelectorAll<HTMLDetailsElement>(".relay-action-menu[open]").forEach((menu) => { menu.open = false; });
    };
    document.addEventListener("pointerdown", closePopovers);
    document.addEventListener("keydown", closeWithEscape);
    return () => {
      document.removeEventListener("pointerdown", closePopovers);
      document.removeEventListener("keydown", closeWithEscape);
    };
  }, []);

  useEffect(() => {
    let disposed = false;
    let unlisten: (() => void) | undefined;
    try {
      void getCurrentWebview().onDragDropEvent((event) => {
        if (event.payload.type === "enter" || event.payload.type === "over") {
          setImportDragActive(true);
          return;
        }
        setImportDragActive(false);
        if (event.payload.type === "drop" && event.payload.paths.length) {
          openImport(event.payload.paths);
        }
      }).then((stop) => {
        if (disposed) stop();
        else unlisten = stop;
      }).catch(() => undefined);
    } catch {
      // Browser previews do not expose Tauri webview metadata.
    }
    return () => {
      disposed = true;
      unlisten?.();
    };
  }, [openImport]);

  return (
    <div className={`relay-shell ${collapsed ? "sidebar-collapsed" : ""}`} data-mode={mode} data-page={page}>
      <aside className="relay-sidebar">
        <div className="mode-picker" ref={modePickerRef}>
          <button
            type="button"
            aria-label={`${t("common.mode")}: ${t(`modes.${mode}`)}`}
            title={t(`modes.${mode}`)}
            aria-haspopup="menu"
            aria-expanded={modeOpen}
            onClick={() => setModeOpen((value) => !value)}
          >
            <ModeIcon mode={mode} />
            <span>{t(`modes.${mode}`)}</span>
            <ChevronDown aria-hidden />
          </button>
          {modeOpen ? (
            <div className="mode-menu" role="menu">
              {(["local", "zenith", "remote"] as RelayMode[]).map((value) => (
                <button
                  role="menuitemradio"
                  aria-checked={mode === value}
                  key={value}
                  type="button"
                  onClick={() => {
                    setMode(value);
                    setModeOpen(false);
                  }}
                >
                  <ModeIcon mode={value} />
                  <span>{t(`modes.${value}`)}</span>
                  {mode === value ? <Check className="mode-check" aria-hidden /> : null}
                </button>
              ))}
            </div>
          ) : null}
        </div>
        <nav aria-label={t("nav.label")}>
          {visiblePages.map(({ id, icon: Icon }) => (
            <button
              key={id}
              type="button"
              className={page === id ? "active" : ""}
              aria-label={t(`nav.${id}`)}
              aria-current={page === id ? "page" : undefined}
              title={t(`nav.${id}`)}
              onClick={() => setPage(id)}
            >
              <Icon aria-hidden />
              <span>{t(`nav.${id}`)}</span>
            </button>
          ))}
        </nav>
        <div className="sidebar-footer">
          <button className="sidebar-help" type="button" aria-label={t("common.help")} title={t("common.help")} onClick={resetOnboarding}>
            <CircleHelp aria-hidden />
            <span className="sidebar-help-copy"><span>{t("common.help")}</span><small>v1.0.5</small></span>
          </button>
          <IconButton
            label={collapsed ? t("shell.expand") : t("shell.collapse")}
            icon={collapsed ? <PanelLeftOpen aria-hidden /> : <PanelLeftClose aria-hidden />}
            onClick={() => setCollapsed((value) => !value)}
          />
        </div>
      </aside>
      <div className="relay-content" ref={contentRef}>
        {feedback ? (
          <div className={`global-feedback ${feedback.kind}`} role="status">
            <span>{t(feedback.key)}</span>
            <IconButton label={t("common.close")} icon={<X aria-hidden />} onClick={clearFeedback} />
          </div>
        ) : null}
        {loading ? <div className="relay-loading">{t("common.loading")}</div> : <Page page={page} onImport={() => openImport()} />}
      </div>
      {importDragActive ? <div className="import-drop-overlay" role="status"><Upload aria-hidden /><strong>{t("accounts.dropImportFiles")}</strong></div> : null}
      {importRequest ? <ImportDialog key={importRequest.id} initialPaths={importRequest.paths} onClose={() => setImportRequest(null)} /> : null}
    </div>
  );
}

function ModeIcon({ mode }: { mode: RelayMode }) {
  return mode === "local" ? <Laptop aria-hidden /> : mode === "remote" ? <Server aria-hidden /> : <Gauge aria-hidden />;
}

function Page({ page, onImport }: { page: PageId; onImport: () => void }) {
  if (page === "overview") return <OverviewPage />;
  if (page === "connections") return <ConnectionsPage onImport={onImport} />;
  if (page === "pool") return <PoolPage />;
  if (page === "gateway") return <GatewayPage />;
  if (page === "usage") return <UsagePage />;
  if (page === "profiles") return <ProfilesPage />;
  return <SettingsPage />;
}
