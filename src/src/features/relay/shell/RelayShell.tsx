import { Activity, ArchiveRestore, Cable, Check, CheckCircle2, ChevronDown, CircleAlert, CircleHelp, Download, Gauge, Laptop, LayoutDashboard, PanelLeftClose, PanelLeftOpen, Server, Settings, SlidersHorizontal, Upload, X } from "lucide-react";
import { getCurrentWebview } from "@tauri-apps/api/webview";
import { lazy, Suspense, useCallback, useEffect, useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import { APP_VERSION } from "../../../platform/desktop";
import type { PageId, RelayMode } from "../api/types";
import { OverviewPage } from "../pages/overview/OverviewPage";
import { useRelayState, type Feedback } from "../state/RelayStateProvider";
import { ErrorDetailsDialog, IconButton } from "../components/Ui";
import { useAppUpdates, type UpdateCheckState } from "../hooks/useAppUpdates";
import { UpdateDialog } from "./UpdateDialog";

const ConnectionsPage = lazy(async () => ({ default: (await import("../pages/connections/ConnectionsPage")).ConnectionsPage }));
const ImportDialog = lazy(async () => ({ default: (await import("../pages/connections/ImportDialog")).ImportDialog }));
const PoolPage = lazy(async () => ({ default: (await import("../pages/pool/PoolPage")).PoolPage }));
const GatewayPage = lazy(async () => ({ default: (await import("../pages/gateway/GatewayPage")).GatewayPage }));
const UsagePage = lazy(async () => ({ default: (await import("../pages/usage/UsagePage")).UsagePage }));
const ProfilesPage = lazy(async () => ({ default: (await import("../pages/profiles/ProfilesPage")).ProfilesPage }));
const SettingsPage = lazy(async () => ({ default: (await import("../pages/settings/SettingsPage")).SettingsPage }));
const HelpCenter = lazy(async () => ({ default: (await import("../help/HelpCenter")).HelpCenter }));

const pages: Array<{ id: PageId; icon: typeof LayoutDashboard }> = [
  { id: "overview", icon: LayoutDashboard },
  { id: "connections", icon: Cable },
  { id: "pool", icon: SlidersHorizontal },
  { id: "gateway", icon: Gauge },
  { id: "usage", icon: Activity },
  { id: "profiles", icon: ArchiveRestore },
  { id: "settings", icon: Settings },
];

export function RelayShell() {
  const { t } = useTranslation();
  const { mode, setMode, page, setPage, feedback, clearFeedback, loading } = useRelayState();
  const [collapsed, setCollapsed] = useState(() => window.matchMedia?.("(max-width: 1023px)").matches ?? false);
  const [modeOpen, setModeOpen] = useState(false);
  const [importRequest, setImportRequest] = useState<{ id: number; paths?: string[] } | null>(null);
  const [importDragActive, setImportDragActive] = useState(false);
  const nextImportRequest = useRef(0);
  const modePickerRef = useRef<HTMLDivElement>(null);
  const contentRef = useRef<HTMLDivElement>(null);
  const {
    availableUpdate,
    updateDialogOpen,
    updateCheckState,
    installingUpdate,
    updateProgress,
    updateInstallError,
    checkUpdates,
    applyUpdate,
    skipUpdate,
    openUpdateDialog,
    closeUpdateDialog,
  } = useAppUpdates();
  const visiblePages = pages.filter((item) => mode !== "zenith" || !(["pool", "gateway", "usage"] as PageId[]).includes(item.id));
  const focusModePicker = useCallback(() => {
    modePickerRef.current?.querySelector<HTMLButtonElement>("button")?.focus({ preventScroll: true });
  }, []);
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
            className="mode-picker-trigger"
            aria-label={`${t("common.mode")}: ${t(`modes.${mode}`)}`}
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
              onClick={() => setPage(id)}
            >
              <Icon aria-hidden />
              <span>{t(`nav.${id}`)}</span>
            </button>
          ))}
        </nav>
        <div className="sidebar-bottom">
          {feedback ? <div className="sidebar-feedback"><GlobalFeedback feedback={feedback} clearFeedback={clearFeedback} focusAfterClose={focusModePicker} /></div> : null}
          <div className="sidebar-footer">
            {availableUpdate ? <button className="sidebar-update" type="button" aria-label={t("updates.open", { version: availableUpdate.version })} onClick={openUpdateDialog}><Download aria-hidden /><span><strong>{t("updates.available")}</strong><small>v{availableUpdate.version}</small></span></button> : null}
            <div className="sidebar-footer-row">
              <button className={`sidebar-help ${page === "help" ? "active" : ""}`} type="button" aria-label={t("common.help")} aria-current={page === "help" ? "page" : undefined} onClick={() => setPage("help")}>
                <CircleHelp aria-hidden />
                <span className="sidebar-help-copy"><span>{t("common.help")}</span><small>v{APP_VERSION}</small></span>
              </button>
              <IconButton
                label={collapsed ? t("shell.expand") : t("shell.collapse")}
                icon={collapsed ? <PanelLeftOpen aria-hidden /> : <PanelLeftClose aria-hidden />}
                onClick={() => setCollapsed((value) => !value)}
              />
            </div>
          </div>
        </div>
      </aside>
      <div className="relay-content" ref={contentRef}>
        {loading ? <div className="relay-loading">{t("common.loading")}</div> : <Suspense key={page} fallback={<div className="relay-loading">{t("common.loading")}</div>}><Page page={page} onImport={() => openImport()} updateCheckState={updateCheckState} updateVersion={availableUpdate?.version ?? null} onCheckUpdates={() => checkUpdates({ openWhenAvailable: true, includeSkipped: true })} /></Suspense>}
      </div>
      {importDragActive ? <div className="import-drop-overlay" role="status"><span className="import-drop-visual"><Upload aria-hidden /></span><strong>{t("accounts.dropImportFiles")}</strong></div> : null}
      {importRequest ? <Suspense fallback={null}><ImportDialog key={importRequest.id} initialPaths={importRequest.paths} onClose={() => setImportRequest(null)} /></Suspense> : null}
      {updateDialogOpen && availableUpdate ? <UpdateDialog update={availableUpdate} installing={installingUpdate} progress={updateProgress} installError={updateInstallError} onInstall={() => void applyUpdate()} onSkip={skipUpdate} onClose={closeUpdateDialog} /> : null}
    </div>
  );
}

function ModeIcon({ mode }: { mode: RelayMode }) {
  return mode === "local" ? <Laptop aria-hidden /> : mode === "remote" ? <Server aria-hidden /> : <Gauge aria-hidden />;
}

function GlobalFeedback({ feedback, clearFeedback, focusAfterClose }: { feedback: Exclude<Feedback, null>; clearFeedback: () => void; focusAfterClose: () => void }) {
  const { t } = useTranslation();
  const [detailsOpen, setDetailsOpen] = useState(false);
  const error = feedback.error;
  const message = t(feedback.key);
  const toastMessage = error ? t("feedback.errorPrompt") : message;
  const accessibleLabel = error ? toastMessage : message;

  useEffect(() => {
    setDetailsOpen(false);
  }, [feedback]);
  const closeDetails = () => {
    setDetailsOpen(false);
    window.requestAnimationFrame(() => {
      focusAfterClose();
      clearFeedback();
    });
  };

  return <>
    {!detailsOpen ? <div className={`global-feedback ${feedback.kind}`} role="status" aria-label={accessibleLabel}>
      {error ? <button
        className="global-feedback-copy global-feedback-error-trigger"
        type="button"
        aria-label={t("feedback.showDetails")}
        aria-haspopup="dialog"
        onClick={() => setDetailsOpen(true)}
      >
        <span className="global-feedback-status-icon" aria-hidden="true"><CircleAlert /></span>
        <span className="global-feedback-message"><span>{toastMessage}</span></span>
      </button> : <div className="global-feedback-copy">
        <span className="global-feedback-status-icon" aria-hidden="true"><CheckCircle2 /></span>
        <span className="global-feedback-message"><span>{message}</span></span>
      </div>}
      <div className="global-feedback-actions">
        {!error ? <IconButton label={t("common.close")} icon={<X aria-hidden />} onClick={clearFeedback} /> : null}
      </div>
    </div> : null}
    {detailsOpen && error ? <ErrorDetailsDialog error={error} message={message} onClose={closeDetails} /> : null}
  </>;
}

function Page({ page, onImport, updateCheckState, updateVersion, onCheckUpdates }: { page: PageId; onImport: () => void; updateCheckState: UpdateCheckState; updateVersion: string | null; onCheckUpdates: () => Promise<UpdateCheckState> }) {
  if (page === "overview") return <OverviewPage />;
  if (page === "connections") return <ConnectionsPage onImport={onImport} />;
  if (page === "pool") return <PoolPage />;
  if (page === "gateway") return <GatewayPage />;
  if (page === "usage") return <UsagePage />;
  if (page === "profiles") return <ProfilesPage />;
  if (page === "help") return <HelpCenter />;
  return <SettingsPage updateCheckState={updateCheckState} updateVersion={updateVersion} onCheckUpdates={onCheckUpdates} />;
}
