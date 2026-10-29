import { Activity, ArchiveRestore, Cable, Check, CheckCircle2, ChevronDown, CircleAlert, CircleHelp, Cloud, Download, Laptop, LayoutDashboard, PanelLeftClose, PanelLeftOpen, Plug, Server, Settings, SlidersHorizontal, Upload, X } from "lucide-react";
import { getCurrentWebview } from "@tauri-apps/api/webview";
import { lazy, Suspense, useCallback, useEffect, useRef, useState, type ComponentType } from "react";
import { useTranslation } from "react-i18next";
import { APP_VERSION } from "../../../platform/desktop";
import { revealStartupShell } from "../../../platform/startup";
import type { PageId, RelayMode } from "../api/types";
import { OverviewPage } from "../pages/overview/OverviewPage";
import { useRelayState, type Feedback } from "../state/RelayStateProvider";
import { Button, EmptyState, ErrorDetailsDialog, IconButton } from "../components/Ui";
import { useAppUpdates, type UpdateCheckState } from "../hooks/useAppUpdates";
import { UpdateDialog } from "./UpdateDialog";
import { HostedDevelopmentPage } from "../pages/hosted/HostedDevelopmentPage";

function lazyNamed<P extends object>(load: () => Promise<ComponentType<P>>) {
  let pending: Promise<{ default: ComponentType<P> }> | undefined;
  let ready: ComponentType<P> | undefined;
  const run = () => (pending ??= load().then((component) => {
    ready = component;
    return { default: component };
  }).catch((error: unknown) => { pending = undefined; throw error; }));
  const Lazy = lazy(run);
  return {
    // React.lazy suspends even on its first read of an already-resolved promise.
    // Render a prepared module directly so the first click has no empty frame.
    Component: (props: P) => { const Component = ready ?? Lazy; return <Component {...props} />; },
    prepare: run,
    preload: () => {
      void run().catch(() => undefined);
    },
  };
}

const connectionsPage = lazyNamed(async () => (await import("../pages/connections/ConnectionsPage")).ConnectionsPage);
const importDialog = lazyNamed(async () => (await import("../pages/connections/ImportDialog")).ImportDialog);
const poolPage = lazyNamed(async () => (await import("../pages/pool/PoolPage")).PoolPage);
const integrationsPage = lazyNamed(async () => (await import("../pages/integrations/IntegrationsPage")).IntegrationsPage);
const usagePage = lazyNamed(async () => (await import("../pages/usage/UsagePage")).UsagePage);
const profilesPage = lazyNamed(async () => (await import("../pages/profiles/ProfilesPage")).ProfilesPage);
const settingsPage = lazyNamed(async () => (await import("../pages/settings/SettingsPage")).SettingsPage);
const helpCenter = lazyNamed(async () => (await import("../help/HelpCenter")).HelpCenter);
const pageModules = { connections: connectionsPage, pool: poolPage, usage: usagePage, integrations: integrationsPage, profiles: profilesPage, settings: settingsPage, help: helpCenter };

const pages: Array<{ id: PageId; icon: typeof LayoutDashboard; preload?: () => void }> = [
  { id: "overview", icon: LayoutDashboard },
  { id: "connections", icon: Cable, preload: connectionsPage.preload },
  { id: "pool", icon: SlidersHorizontal, preload: poolPage.preload },
  { id: "integrations", icon: Plug, preload: integrationsPage.preload },
  { id: "usage", icon: Activity, preload: usagePage.preload },
  { id: "profiles", icon: ArchiveRestore, preload: profilesPage.preload },
  { id: "settings", icon: Settings, preload: settingsPage.preload },
];

export function RelayShell() {
  const { t } = useTranslation();
  const { mode, setMode, page, setPage, feedback, clearFeedback, loading, runtime, runtimeReady } = useRelayState();
  const pageNeedsRuntime = mode === "local" && page !== "settings" && page !== "help";
  const navigationRevision = useRef(0);
  const [navigationFailure, setNavigationFailure] = useState<PageId | null>(null);
  const navigate = useCallback((target: PageId) => {
    const revision = ++navigationRevision.current;
    setNavigationFailure(null);
    if (target === "overview") { setPage(target); return; }
    // Keep the actual current screen until both the target JS and CSS are ready.
    void pageModules[target].prepare().then(() => {
      if (navigationRevision.current === revision) setPage(target);
    }).catch(() => {
      if (navigationRevision.current === revision) setNavigationFailure(target);
    });
  }, [setPage]);
  useEffect(() => () => { navigationRevision.current += 1; }, [mode, page]);
  useEffect(() => {
    let secondFrame = 0;
    const firstFrame = requestAnimationFrame(() => {
      secondFrame = requestAnimationFrame(revealStartupShell);
    });
    return () => {
      cancelAnimationFrame(firstFrame);
      cancelAnimationFrame(secondFrame);
    };
  }, []);
  const sidebarPreference = useRef<boolean | null>(null);
  const [collapsed, setCollapsed] = useState(() => window.matchMedia?.("(max-width: 1023px)").matches ?? false);
  useEffect(() => {
    const compact = window.matchMedia?.("(max-width: 1023px)");
    const syncSidebarWithViewport = () => setCollapsed(sidebarPreference.current ?? compact?.matches ?? false);
    compact?.addEventListener("change", syncSidebarWithViewport);
    return () => compact?.removeEventListener("change", syncSidebarWithViewport);
  }, []);
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
  const mainPages = mode === "hosted" ? [] : pages.filter(({ id }) => id !== "profiles" && id !== "settings");
  const utilityPages = mode === "hosted" ? [] : pages.filter(({ id }) => id === "profiles" || id === "settings");
  const navigationButton = ({ id: pageId, icon: Icon, preload }: (typeof pages)[number]) => (
    <button
      key={pageId}
      type="button"
      className={page === pageId ? "active" : ""}
      aria-label={t(`nav.${pageId}`)}
      aria-current={page === pageId ? "page" : undefined}
      data-relay-tooltip={collapsed ? t(`nav.${pageId}`) : undefined}
      onClick={() => navigate(pageId)}
      onPointerEnter={preload}
      onFocus={preload}
    >
      <Icon aria-hidden />
      <span>{t(`nav.${pageId}`)}</span>
    </button>
  );
  const focusModePicker = useCallback(() => {
    modePickerRef.current?.querySelector<HTMLButtonElement>("button")?.focus({ preventScroll: true });
  }, []);
  const openImport = useCallback((paths?: string[]) => {
    if (mode === "hosted") return;
    navigationRevision.current += 1;
    importDialog.preload();
    navigate("connections");
    setImportRequest({ id: ++nextImportRequest.current, ...(paths ? { paths } : {}) });
  }, [mode, navigate]);

  useEffect(() => {
    if (contentRef.current) {
      contentRef.current.scrollTop = 0;
      contentRef.current.scrollLeft = 0;
    }
  }, [mode, page]);

  useEffect(() => {
    if (!modeOpen) return;
    const closePopovers = (event: PointerEvent) => {
      const target = event.target as Node;
      if (!modePickerRef.current?.contains(target)) setModeOpen(false);
    };
    const closeWithEscape = (event: KeyboardEvent) => {
      if (event.key !== "Escape" || event.defaultPrevented) return;
      event.preventDefault();
      setModeOpen(false);
      focusModePicker();
    };
    document.addEventListener("pointerdown", closePopovers);
    document.addEventListener("keydown", closeWithEscape);
    return () => {
      document.removeEventListener("pointerdown", closePopovers);
      document.removeEventListener("keydown", closeWithEscape);
    };
  }, [modeOpen, focusModePicker]);

  useEffect(() => {
    if (mode === "hosted") return;
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
  }, [mode, openImport]);

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
            onClick={() => setModeOpen((isOpen) => !isOpen)}
          >
            <ModeIcon mode={mode} />
            <span className="mode-picker-label">{t(`modes.${mode}`)}</span>
            <ChevronDown aria-hidden />
          </button>
          {modeOpen ? (
            <div className="mode-menu relay-popover-panel" role="menu">
              {(["local", "hosted"] as RelayMode[]).map((modeOption) => (
                <button
                  className="relay-popover-item"
                  role="menuitemradio"
                  aria-checked={mode === modeOption}
                  key={modeOption}
                  type="button"
                  onClick={() => {
                    navigationRevision.current += 1;
                    setNavigationFailure(null);
                    setMode(modeOption);
                    setImportRequest(null);
                    setImportDragActive(false);
                    setModeOpen(false);
                  }}
                >
                  <ModeIcon mode={modeOption} />
                  <span>{t(`modes.${modeOption}`)}</span>
                  {mode === modeOption ? <Check className="mode-check" aria-hidden /> : null}
                </button>
              ))}
            </div>
          ) : null}
        </div>
        <div className="sidebar-navigation">
          <nav className="sidebar-main-nav" aria-label={t("nav.label")}>
            <div className="sidebar-nav-group">{mainPages.map(navigationButton)}</div>
          </nav>
        </div>
        <div className="sidebar-bottom">
          {feedback ? <div className="sidebar-feedback"><GlobalFeedback feedback={feedback} clearFeedback={clearFeedback} focusAfterClose={focusModePicker} /></div> : null}
          {availableUpdate ? (
            <div className="sidebar-update-row">
              <button
                className="sidebar-update"
                type="button"
                aria-label={t("updates.open", { version: availableUpdate.version })}
                onClick={openUpdateDialog}
              >
                <Download aria-hidden />
                <span>{t("updates.available")}</span>
              </button>
            </div>
          ) : null}
          <div className="sidebar-bottom-actions">
            <nav className="sidebar-nav-utilities" aria-label={t("nav.utilities")}>
              {utilityPages.map(navigationButton)}
            </nav>
            <button
              className={`sidebar-help ${page === "help" ? "active" : ""}`}
              type="button"
              aria-label={t("common.help")}
              aria-current={page === "help" ? "page" : undefined}
              data-relay-tooltip={collapsed ? t("common.help") : undefined}
              onClick={() => navigate("help")}
              onPointerEnter={helpCenter.preload}
              onFocus={helpCenter.preload}
            >
              <CircleHelp aria-hidden />
              <span className="sidebar-help-copy">{t("common.help")}</span>
            </button>
          </div>
          <div className="sidebar-footer">
            <small className="sidebar-version">v{APP_VERSION}</small>
            <IconButton
              label={collapsed ? t("shell.expand") : t("shell.collapse")}
              icon={collapsed ? <PanelLeftOpen aria-hidden /> : <PanelLeftClose aria-hidden />}
              onClick={() => {
                sidebarPreference.current = !collapsed;
                setCollapsed(!collapsed);
              }}
            />
          </div>
        </div>
      </aside>
      <div className="relay-content" ref={contentRef}>
        {navigationFailure ? <div className="form-note error-text" role="alert">
          {t("feedback.refreshFailed")} <Button variant="secondary" onClick={() => navigate(navigationFailure)}>{t("common.retry")}</Button>
        </div> : null}
        {/* Remount the boundary per page: a retained boundary keeps the
            previous page hidden in the DOM while the next lazy chunk loads,
            so automation and assistive tech would still resolve the old
            page's nodes after navigation. */}
        {pageNeedsRuntime && !runtime && page !== "overview" ? (
          loading ? null : <RuntimeUnavailable />
        ) : (
          <div inert={pageNeedsRuntime && !runtimeReady} style={{ display: "contents" }}>
          <Suspense key={page} fallback={null}>
            <Page
              page={page}
              onImport={() => openImport()}
              updateCheckState={updateCheckState}
              updateVersion={availableUpdate?.version ?? null}
              onCheckUpdates={() => checkUpdates({ openWhenAvailable: true, includeSkipped: true })}
            />
          </Suspense>
          </div>
        )}
        {pageNeedsRuntime && (runtime || page === "overview") && !runtimeReady && !loading ? <RuntimeUnavailable /> : null}
      </div>
      {importDragActive ? (
        <div className="import-drop-overlay" role="status">
          <span className="import-drop-visual"><Upload aria-hidden /></span>
          <strong>{t("accounts.dropImportFiles")}</strong>
        </div>
      ) : null}
      {importRequest ? (
        <Suspense fallback={null}>
          <importDialog.Component
            key={importRequest.id}
            {...(importRequest.paths ? { initialPaths: importRequest.paths } : {})}
            onClose={() => setImportRequest(null)}
          />
        </Suspense>
      ) : null}
      {updateDialogOpen && availableUpdate ? (
        <UpdateDialog
          update={availableUpdate}
          installing={installingUpdate}
          progress={updateProgress}
          installError={updateInstallError}
          onInstall={() => void applyUpdate()}
          onSkip={skipUpdate}
          onClose={closeUpdateDialog}
        />
      ) : null}
    </div>
  );
}

function RuntimeUnavailable() {
  const { t } = useTranslation();
  const { refresh } = useRelayState();
  const [retrying, setRetrying] = useState(false);
  const retry = async () => {
    if (retrying) return;
    setRetrying(true);
    try {
      await refresh();
    } catch {
      // Keep the failed state visible; an unsuccessful retry is not empty data.
    } finally {
      setRetrying(false);
    }
  };
  return <div className="relay-page"><EmptyState
    title={t("feedback.refreshFailed")}
    action={<Button variant="secondary" busy={retrying} onClick={() => void retry()}>{t("common.retry")}</Button>}
  /></div>;
}

function ModeIcon({ mode }: { mode: RelayMode }) {
  return mode === "local" ? <Laptop aria-hidden /> : mode === "hosted" ? <Cloud aria-hidden /> : <Server aria-hidden />;
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
  const { mode } = useRelayState();
  if (mode === "hosted" && page !== "help") return <HostedDevelopmentPage />;
  if (page === "overview") return <OverviewPage />;
  if (page === "connections") return <connectionsPage.Component onImport={onImport} />;
  if (page === "pool") return <poolPage.Component />;
  if (page === "integrations") return <integrationsPage.Component />;
  if (page === "usage") return <usagePage.Component />;
  if (page === "profiles") return <profilesPage.Component />;
  if (page === "help") return <helpCenter.Component />;
  return <settingsPage.Component updateCheckState={updateCheckState} updateVersion={updateVersion} onCheckUpdates={onCheckUpdates} />;
}
