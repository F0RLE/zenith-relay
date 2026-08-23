import { useEffect, useState } from "react";
import { LogIn, Play, Plus, RefreshCw, Upload } from "lucide-react";
import { useTranslation } from "react-i18next";
import { relayCommands } from "../../api/commands";
import type { AccountSummary, SourceSummary, WakeTask } from "../../api/types";
import { Button, PageHeader, Tabs } from "../../components/Ui";
import { useOAuthSignIn } from "../../hooks/useOAuthSignIn";
import { useRelayState } from "../../state/RelayStateProvider";
import { AccountProxyDialog, BulkProxyDialog, ProxyImportDialog, ProxyStorageView } from "./ProxyDialogs";
import { connectionInitialView, connectionViews, reconcileRemoteConnectionView, type ConnectionView } from "./connectionViewState";
import { AccountsTable } from "./AccountsTable";
import { SourceDialog } from "./SourceDialog";
import { AccountExportDialog } from "./AccountExportDialog";
import { AutomationDialog, AutomationsTable } from "./AutomationsView";
import { OAuthAccountSetupDialog, OAuthDialog } from "./OAuthDialogs";
import { DeployDialog, RemoteDialog, RemoteView } from "./RemoteViews";
import { SourcesTable } from "./SourcesTable";
type DialogKind = "source" | "automation" | "remote" | "deploy" | "accountProxy" | "bulkProxies" | "proxyImport" | "oauthSetup" | "accountExport" | null;
const CONNECTIONS_VIEW_REQUEST = "relay.connections.requestedView";
export function ConnectionsPage({ onImport }: { onImport: () => void }) {
  const { t } = useTranslation();
  const { mode, runtime, busy, perform, refresh } = useRelayState();
  const [view, setView] = useState<ConnectionView>(() => connectionInitialView(
    mode,
    mode === "zenith" ? "sources" : "accounts",
    null,
    runtime?.capabilities.features ?? [],
  ));
  const [dialog, setDialog] = useState<DialogKind>(null);
  const [query, setQuery] = useState("");
  const [editingSource, setEditingSource] = useState<SourceSummary | null>(null);
  const [editingAutomation, setEditingAutomation] = useState<WakeTask | null>(null);
  const [proxyAccount, setProxyAccount] = useState<AccountSummary | null>(null);
  const [bulkProxyAccountIds, setBulkProxyAccountIds] = useState<string[]>([]);
  const [exportAccountIds, setExportAccountIds] = useState<string[]>([]);
  const [oauthAccountId, setOauthAccountId] = useState<string | null>(null);
  const [proxyRevision, setProxyRevision] = useState(0);
  const oauth = useOAuthSignIn((result) => {
    setOauthAccountId(result.account.id);
    setDialog("oauthSetup");
  });
  const startOAuth = () => void oauth.start(false);
  const remoteFeatures = new Set(runtime?.capabilities.features ?? []);
  const supports = (feature: string) => mode !== "remote" || remoteFeatures.has(feature);
  const availableViews = connectionViews(mode, runtime?.capabilities.features ?? []);
  const canImportAccounts = mode !== "remote" || supports("account_batch_import");
  const canManageProxies = supports("account_proxies");
  const canExportAccounts = supports("account_export");
  const showTableToolbar = view === "sources"
    ? Boolean(runtime?.sources.length)
    : view === "automations" && Boolean(runtime?.automations.length);

  useEffect(() => {
    const requested = mode === "zenith" ? null : sessionStorage.getItem(CONNECTIONS_VIEW_REQUEST);
    setView((current) => connectionInitialView(mode, current, requested, runtime?.capabilities.features ?? []));
    setDialog(null);
    setEditingSource(null);
    setEditingAutomation(null);
    setProxyAccount(null);
    setBulkProxyAccountIds([]);
    setExportAccountIds([]);
    setOauthAccountId(null);
  }, [mode]);

  useEffect(() => setQuery(""), [mode, view]);

  useEffect(() => {
    if (mode !== "remote") return;
    if (!runtime) {
      setView((current) => reconcileRemoteConnectionView(mode, false, current));
      return;
    }
    if (!availableViews.includes(view)) setView("remote");
  }, [mode, runtime, view]);

  const tabLabels: Record<ConnectionView, string> = {
    accounts: t("connections.accounts"),
    sources: t("connections.sources"),
    proxies: t("proxies.storage"),
    automations: t("connections.automations"),
    remote: t("connections.remoteServer"),
  };
  const tabs = availableViews.map((id) => ({ id, label: tabLabels[id] }));

  const primaryLabel = view === "accounts"
    ? mode === "local" ? t("accounts.signIn") : t("connections.import")
    : view === "proxies"
      ? t("proxies.import")
    : view === "sources"
      ? t("sources.add")
      : view === "automations"
        ? t("automations.add")
        : runtime ? t("remote.refresh") : t("remote.connect");

  const primaryAction = () => {
    if (view === "accounts" && !canImportAccounts) return;
    if (view === "accounts" && mode === "local") {
      startOAuth();
      return;
    }
    if (view === "accounts" && mode === "remote") {
      onImport();
      return;
    }
    if (view === "proxies") {
      setDialog("proxyImport");
      return;
    }
    if (view === "remote" && runtime) {
      void perform("remote-refresh", relayCommands.refreshRemoteCapabilities, "feedback.refreshed");
      return;
    }
    setEditingSource(null);
    setEditingAutomation(null);
    setDialog(
      view === "sources" ? "source"
          : view === "automations" ? "automation"
            : "remote",
    );
  };

  return (
    <section className="relay-page" data-view={view}>
      <PageHeader
        title={t("nav.connections")}
        subtitle={t(`connections.subtitles.${mode}`)}
        actions={
          <>
            {view === "accounts" && mode === "local" ? (
              <Button variant="secondary" icon={<Upload aria-hidden />} disabled={!canImportAccounts} title={!canImportAccounts ? t("remote.capabilityUnavailable") : undefined} onClick={onImport}>
                {t("connections.import")}
              </Button>
            ) : null}
            <Button variant="primary" icon={view === "accounts" ? mode === "local" ? <LogIn aria-hidden /> : <Upload aria-hidden /> : view === "proxies" ? <Upload aria-hidden /> : view === "remote" && runtime ? <RefreshCw aria-hidden /> : <Plus aria-hidden />} busy={view === "accounts" && mode === "local" && busy === "oauth-start"} disabled={view === "accounts" && !canImportAccounts} title={view === "accounts" && !canImportAccounts ? t("remote.capabilityUnavailable") : undefined} onClick={primaryAction}>
              {primaryLabel}
            </Button>
          </>
        }
      />
      <Tabs value={view} items={tabs} onChange={(id) => { if (id === "sources") sessionStorage.setItem(CONNECTIONS_VIEW_REQUEST, id); else sessionStorage.removeItem(CONNECTIONS_VIEW_REQUEST); setView(id as ConnectionView); }} label={t("connections.views")} />
      {showTableToolbar ? <div className={`table-toolbar${view === "sources" ? " relay-compact-content" : ""}`}>
        <label className="search-field">
          <span className="sr-only">{t("common.search")}</span>
          <input value={query} onChange={(event) => setQuery(event.target.value)} placeholder={t("common.search")} />
        </label>
        {view === "automations" && mode === "local" ? <Button variant="secondary" icon={<Play aria-hidden />} busy={busy === "wake-due"} onClick={() => perform("wake-due", relayCommands.runWakeConfirmations, "feedback.checked")}>{t("automations.runDue")}</Button> : null}
        <Button variant="secondary" icon={<RefreshCw aria-hidden />} onClick={() => void refresh()}>{t("common.refresh")}</Button>
      </div> : null}

      {view === "sources" ? <SourcesTable query={query} onEdit={(source) => { setEditingSource(source); setDialog("source"); }} /> : null}
      {view === "accounts" ? <AccountsTable query={query} onQuery={setQuery} canImport={canImportAccounts} canManageProxies={canManageProxies} canExport={canExportAccounts} onImport={onImport} onSignIn={startOAuth} onProxy={(account) => { setProxyAccount(account); setDialog("accountProxy"); }} onBulkProxies={(accountIds) => { setBulkProxyAccountIds(accountIds); setDialog("bulkProxies"); }} onExport={(accountIds) => { setExportAccountIds(accountIds); setDialog("accountExport"); }} /> : null}
      {view === "proxies" ? <ProxyStorageView revision={proxyRevision} onImport={() => setDialog("proxyImport")} /> : null}
      {view === "automations" ? <AutomationsTable query={query} onEdit={(task) => { setEditingAutomation(task); setDialog("automation"); }} /> : null}
      {view === "remote" ? <RemoteView onConnect={() => setDialog("remote")} onDeploy={() => setDialog("deploy")} /> : null}

      {dialog === "source" ? <SourceDialog source={editingSource} onClose={() => { setDialog(null); setEditingSource(null); }} /> : null}
      {oauth.flow ? <OAuthDialog flow={oauth.flow} onCancel={oauth.cancel} /> : null}
      {dialog === "automation" ? <AutomationDialog task={editingAutomation} onClose={() => { setDialog(null); setEditingAutomation(null); }} /> : null}
      {dialog === "remote" ? <RemoteDialog onClose={() => setDialog(null)} /> : null}
      {dialog === "deploy" ? <DeployDialog onClose={() => setDialog(null)} /> : null}
      {dialog === "accountProxy" && proxyAccount ? <AccountProxyDialog account={proxyAccount} onClose={() => { setDialog(null); setProxyAccount(null); }} /> : null}
      {dialog === "bulkProxies" ? <BulkProxyDialog accountIds={bulkProxyAccountIds} onClose={() => setDialog(null)} /> : null}
      {dialog === "proxyImport" ? <ProxyImportDialog onImported={() => setProxyRevision((value) => value + 1)} onClose={() => setDialog(null)} /> : null}
      {dialog === "oauthSetup" && oauthAccountId ? <OAuthAccountSetupDialog accountId={oauthAccountId} onClose={() => { setDialog(null); setOauthAccountId(null); }} /> : null}
      {dialog === "accountExport" ? <AccountExportDialog accountIds={exportAccountIds} onClose={() => { setDialog(null); setExportAccountIds([]); }} /> : null}
      {busy ? <span className="sr-only" aria-live="polite">{t("common.working")}</span> : null}
    </section>
  );
}
