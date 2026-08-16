import { Activity, ArchiveRestore, Cable, Check, CheckCircle2, ChevronDown, ChevronUp, CircleAlert, CircleHelp, Copy, Download, Gauge, Laptop, LayoutDashboard, PanelLeftClose, PanelLeftOpen, Server, Settings, SlidersHorizontal, Upload, X } from "lucide-react";
import { getCurrentWebview } from "@tauri-apps/api/webview";
import { lazy, Suspense, useCallback, useEffect, useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import { APP_VERSION, checkForUpdate, installUpdate, type AppUpdate } from "../../../platform/desktop";
import type { PageId, RelayMode } from "../api/types";
import { OverviewPage } from "../pages/overview/OverviewPage";
import { useRelayState, type Feedback } from "../state/RelayStateProvider";
import { ActionMenu, ActionMenuItem, Button, copyText, Dialog, IconButton } from "../components/Ui";

const SKIPPED_UPDATE_KEY = "relay.skippedUpdate";
type UpdateCheckState = "idle" | "checking" | "current" | "available" | "error" | "skipped";
type UpdateInstallError = "write" | "install" | null;

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
  const [availableUpdate, setAvailableUpdate] = useState<AppUpdate | null>(null);
  const [updateDialogOpen, setUpdateDialogOpen] = useState(false);
  const [updateCheckState, setUpdateCheckState] = useState<UpdateCheckState>("idle");
  const [installingUpdate, setInstallingUpdate] = useState(false);
  const [updateProgress, setUpdateProgress] = useState<{ downloaded: number; total?: number } | null>(null);
  const [updateInstallError, setUpdateInstallError] = useState<UpdateInstallError>(null);
  const nextImportRequest = useRef(0);
  const initialUpdateCheck = useRef(false);
  const modePickerRef = useRef<HTMLDivElement>(null);
  const contentRef = useRef<HTMLDivElement>(null);
  const visiblePages = pages.filter((item) => mode !== "zenith" || !(["pool", "gateway", "usage"] as PageId[]).includes(item.id));
  const openImport = useCallback((paths?: string[]) => {
    if (mode === "zenith") setMode("local");
    setPage("connections");
    setImportRequest({ id: ++nextImportRequest.current, paths });
  }, [mode, setMode, setPage]);
  const checkUpdates = useCallback(async (openWhenAvailable = false, includeSkipped = false): Promise<UpdateCheckState> => {
    setUpdateCheckState("checking");
    try {
      const update = await checkForUpdate();
      if (!update) {
        setAvailableUpdate(null);
        setUpdateCheckState("current");
        return "current";
      }
      if (!includeSkipped && localStorage.getItem(SKIPPED_UPDATE_KEY) === update.version) {
        setAvailableUpdate(null);
        setUpdateCheckState("skipped");
        return "skipped";
      }
      setAvailableUpdate(update);
      setUpdateCheckState("available");
      if (openWhenAvailable) setUpdateDialogOpen(true);
      return "available";
    } catch {
      setUpdateCheckState("error");
      return "error";
    }
  }, []);

  const applyUpdate = useCallback(async () => {
    if (!availableUpdate) return;
    setInstallingUpdate(true);
    setUpdateInstallError(null);
    setUpdateProgress({ downloaded: 0 });
    try {
      const result = await installUpdate(availableUpdate, (downloaded, total) => setUpdateProgress({ downloaded, total }));
      if (result === "unavailable") {
        setAvailableUpdate(null);
        setUpdateCheckState("current");
        setUpdateInstallError("install");
      }
    } catch (error) {
      setUpdateInstallError(String(error).includes("portable_not_writable") ? "write" : "install");
    } finally {
      setInstallingUpdate(false);
    }
  }, [availableUpdate]);

  const skipUpdate = useCallback(() => {
    if (availableUpdate) localStorage.setItem(SKIPPED_UPDATE_KEY, availableUpdate.version);
    setAvailableUpdate(null);
    setUpdateDialogOpen(false);
    setUpdateCheckState("skipped");
  }, [availableUpdate]);

  useEffect(() => {
    const timeout = window.setTimeout(() => {
      if (initialUpdateCheck.current) return;
      initialUpdateCheck.current = true;
      void checkUpdates();
    }, 1_500);
    return () => window.clearTimeout(timeout);
  }, [checkUpdates]);

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
          {feedback ? <GlobalFeedback feedback={feedback} clearFeedback={clearFeedback} /> : null}
          {availableUpdate ? <button className="sidebar-update" type="button" aria-label={t("updates.open", { version: availableUpdate.version })} title={t("updates.open", { version: availableUpdate.version })} onClick={() => setUpdateDialogOpen(true)}><Download aria-hidden /><span><strong>{t("updates.available")}</strong><small>v{availableUpdate.version}</small></span></button> : null}
          <div className="sidebar-footer-row">
            <button className={`sidebar-help ${page === "help" ? "active" : ""}`} type="button" aria-label={t("common.help")} title={t("common.help")} aria-current={page === "help" ? "page" : undefined} onClick={() => setPage("help")}>
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
      </aside>
      <div className="relay-content" ref={contentRef}>
        {loading ? <div className="relay-loading">{t("common.loading")}</div> : <Suspense key={page} fallback={<div className="relay-loading">{t("common.loading")}</div>}><Page page={page} onImport={() => openImport()} updateCheckState={updateCheckState} updateVersion={availableUpdate?.version ?? null} onCheckUpdates={() => checkUpdates(true, true)} /></Suspense>}
      </div>
      {importDragActive ? <div className="import-drop-overlay" role="status"><span className="import-drop-visual"><Upload aria-hidden /></span><strong>{t("accounts.dropImportFiles")}</strong></div> : null}
      {importRequest ? <Suspense fallback={null}><ImportDialog key={importRequest.id} initialPaths={importRequest.paths} onClose={() => setImportRequest(null)} /></Suspense> : null}
      {updateDialogOpen && availableUpdate ? <UpdateDialog update={availableUpdate} installing={installingUpdate} progress={updateProgress} installError={updateInstallError} onInstall={() => void applyUpdate()} onSkip={skipUpdate} onClose={() => { if (!installingUpdate) setUpdateDialogOpen(false); }} /> : null}
    </div>
  );
}

function ModeIcon({ mode }: { mode: RelayMode }) {
  return mode === "local" ? <Laptop aria-hidden /> : mode === "remote" ? <Server aria-hidden /> : <Gauge aria-hidden />;
}

function GlobalFeedback({ feedback, clearFeedback }: { feedback: Exclude<Feedback, null>; clearFeedback: () => void }) {
  const { t } = useTranslation();
  const [detailsOpen, setDetailsOpen] = useState(false);
  const [copied, setCopied] = useState(false);
  const copyTimer = useRef<number | null>(null);
  const details = feedback.error ? JSON.stringify(feedback.error, null, 2) : null;
  const message = t(feedback.key);
  const accessibleLabel = feedback.error ? `${message} (${feedback.error.code})` : message;

  useEffect(() => {
    setDetailsOpen(false);
    setCopied(false);
    if (copyTimer.current !== null) window.clearTimeout(copyTimer.current);
    copyTimer.current = null;
    return () => {
      if (copyTimer.current !== null) window.clearTimeout(copyTimer.current);
    };
  }, [feedback]);

  const copyError = async () => {
    if (!details) return;
    try {
      await copyText(details);
      setCopied(true);
      if (copyTimer.current !== null) window.clearTimeout(copyTimer.current);
      copyTimer.current = window.setTimeout(() => {
        setCopied(false);
        copyTimer.current = null;
      }, 2_500);
    } catch {
      setCopied(false);
    }
  };

  return <div className={`global-feedback ${feedback.kind}${detailsOpen ? " details-open" : ""}`} role="status" aria-label={accessibleLabel}>
    <div className="global-feedback-copy">
      {feedback.error ? <button
        className="global-feedback-status-icon global-feedback-status-icon-button"
        type="button"
        aria-label={detailsOpen ? t("feedback.hideDetails") : t("feedback.showDetails")}
        aria-expanded={detailsOpen}
        title={detailsOpen ? t("feedback.hideDetails") : t("feedback.showDetails")}
        onClick={() => setDetailsOpen((open) => !open)}
      ><CircleAlert aria-hidden /></button> : <span className="global-feedback-status-icon" aria-hidden="true" title={message}><CheckCircle2 /></span>}
      <span className="global-feedback-message"><span>{message}</span>{feedback.error ? <code>{feedback.error.code}</code> : null}</span>
    </div>
    <div className="global-feedback-actions">
      {feedback.error ? <>
        <span className="global-feedback-copy-state" role="status" aria-live="polite">{copied ? t("feedback.copied") : ""}</span>
        <ActionMenu className="global-feedback-menu" label={copied ? t("feedback.copied") : t("feedback.errorActions")}>
          <ActionMenuItem icon={detailsOpen ? <ChevronUp aria-hidden /> : <CircleAlert aria-hidden />} onClick={() => setDetailsOpen((open) => !open)}>{detailsOpen ? t("feedback.hideDetails") : t("feedback.showDetails")}</ActionMenuItem>
          <ActionMenuItem icon={copied ? <Check aria-hidden /> : <Copy aria-hidden />} onClick={() => void copyError()}>{copied ? t("feedback.copied") : t("feedback.copyError")}</ActionMenuItem>
        </ActionMenu>
      </> : null}
      <IconButton label={t("common.close")} icon={<X aria-hidden />} onClick={clearFeedback} />
    </div>
    {detailsOpen && details ? <div className="global-feedback-details" role="region" aria-label={t("feedback.errorDetails")}><pre><code>{details}</code></pre><p>{t("feedback.detailsHint")}</p></div> : null}
  </div>;
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

function UpdateDialog({ update, installing, progress, installError, onInstall, onSkip, onClose }: { update: AppUpdate; installing: boolean; progress: { downloaded: number; total?: number } | null; installError: UpdateInstallError; onInstall: () => void; onSkip: () => void; onClose: () => void }) {
  const { i18n, t } = useTranslation();
  const percent = progress?.total ? Math.min(100, Math.round(progress.downloaded / progress.total * 100)) : null;
  const date = update.date ? new Intl.DateTimeFormat(i18n.language, { dateStyle: "long" }).format(new Date(update.date)) : null;
  const notes = localizeReleaseNotes(update.body, i18n.language);
  return <Dialog title={t("updates.title", { version: update.version })} onClose={onClose} footer={<div className="update-actions"><Button variant="secondary" disabled={installing} onClick={onSkip}>{t("updates.skipVersion", { version: update.version })}</Button><Button variant="primary" icon={<Download aria-hidden />} busy={installing} onClick={onInstall}>{t("updates.install")}</Button></div>}>
    <div className="update-release"><div><span>{t("updates.versionChange", { current: update.currentVersion, next: update.version })}</span>{date ? <small>{date}</small> : null}</div></div>
    <section className="update-notes"><h3>{t("updates.changelog")}</h3><p>{notes || t("updates.noChangelog")}</p></section>
    {installing ? <div className="update-progress" role="status"><div><strong>{t("updates.downloading")}</strong><span>{percent === null ? t("updates.preparing") : `${percent}%`}</span></div><progress max={100} value={percent ?? undefined} /></div> : null}
    {installError ? <p className="warning-box" role="alert">{t(installError === "write" ? "updates.portableWriteFailed" : "updates.installFailed")}</p> : null}
  </Dialog>;
}

function localizeReleaseNotes(body: string | undefined, language: string) {
  if (!body?.trim()) return "";
  const markers = [...body.matchAll(/<!--\s*relay-notes:([a-z0-9-]+)\s*-->/gi)];
  if (!markers.length) return body.trim();
  const sections = new Map(markers.map((marker, index) => [
    marker[1].toLowerCase(),
    body.slice((marker.index ?? 0) + marker[0].length, markers[index + 1]?.index).trim(),
  ]));
  const locale = language.toLowerCase();
  return sections.get(locale) ?? sections.get(locale.split("-")[0]) ?? sections.get("en") ?? sections.values().next().value ?? "";
}
