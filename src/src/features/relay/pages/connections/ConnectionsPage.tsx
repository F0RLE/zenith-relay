import { useEffect, useState } from "react";
import { LogIn, Plus, Upload } from "lucide-react";
import { useTranslation } from "react-i18next";
import type { AccountSummary, SourceSummary, WakeTask } from "../../api/types";
import { Button, PageHeader, Tabs } from "../../components/Ui";
import { useAccountSignIn } from "../../hooks/useAccountSignIn";
import { useSourceRefresh } from "../../hooks/useSourceRefresh";
import { useSourceStats } from "../../hooks/useSourceStats";
import { useRelayState } from "../../state/RelayStateProvider";
import { AccountProxyDialog, BulkProxyDialog, ProxyImportDialog, ProxyStorageView } from "./ProxyDialogs";
import { connectionInitialView, connectionViews, type ConnectionView } from "./connectionViewState";
import { AccountsTable } from "./AccountsTable";
import { AccountTrashView } from "./AccountTrashView";
import { SourceDialog } from "./SourceDialog";
import { AccountExportDialog } from "./AccountExportDialog";
import { AutomationDialog, AutomationsList } from "./AutomationsView";
import { SourcesList } from "./SourcesList";
import { useProxyChecks } from "./useProxyChecks";
type DialogKind = "source" | "automation" | "accountProxy" | "bulkProxies" | "proxyImport" | "accountExport" | null;
const CONNECTIONS_VIEW_REQUEST = "relay.connections.requestedView";
const EMPTY_SOURCES: SourceSummary[] = [];
export function ConnectionsPage({ onImport }: { onImport: () => void }) {
  const { t } = useTranslation();
  const { mode, runtime, busy, perform } = useRelayState();
  const [view, setView] = useState<ConnectionView>(() => connectionInitialView(
    "accounts",
    null,
  ));
  const [dialog, setDialog] = useState<DialogKind>(null);
  const [query, setQuery] = useState("");
  const [editingSource, setEditingSource] = useState<SourceSummary | null>(null);
  const [editingAutomation, setEditingAutomation] = useState<WakeTask | null>(null);
  const [proxyAccount, setProxyAccount] = useState<AccountSummary | null>(null);
  const [bulkProxyAccountIds, setBulkProxyAccountIds] = useState<string[]>([]);
  const [exportAccountIds, setExportAccountIds] = useState<string[]>([]);
  const [proxyRevision, setProxyRevision] = useState(0);
  const proxyChecks = useProxyChecks(mode);
  const signIn = useAccountSignIn();
  const startOAuth = () => signIn.start();
  const availableViews = connectionViews();
  const { stats: sourceStats, refresh: refreshSourceStats } = useSourceStats(mode, view === "sources" ? runtime?.sources ?? EMPTY_SOURCES : EMPTY_SOURCES);

  useEffect(() => {
    const requested = sessionStorage.getItem(CONNECTIONS_VIEW_REQUEST);
    setView((previousView) => connectionInitialView(previousView, requested));
    setDialog(null);
    setEditingSource(null);
    setEditingAutomation(null);
    setProxyAccount(null);
    setBulkProxyAccountIds([]);
    setExportAccountIds([]);
  }, [mode]);

  useEffect(() => setQuery(""), [mode, view]);

  const tabLabels: Record<ConnectionView, string> = {
    accounts: t("connections.accounts"),
    sources: t("connections.sources"),
    proxies: t("proxies.storage"),
    automations: t("connections.automations"),
    trash: t("trash.title"),
  };
  const tabs = availableViews.map((viewId) => ({ id: viewId, label: tabLabels[viewId] }));

  const primaryLabel = view === "accounts"
    ? t("accounts.signIn")
    : view === "proxies"
      ? t("proxies.import")
    : view === "sources"
      ? t("sources.add")
      : t("automations.add");

  const primaryAction = () => {
    if (view === "accounts") {
      startOAuth();
      return;
    }
    if (view === "proxies") {
      setDialog("proxyImport");
      return;
    }
    setEditingSource(null);
    setEditingAutomation(null);
    setDialog(
      view === "sources" ? "source"
          : view === "automations" ? "automation" : null,
    );
  };
  const { report: sourceRefreshReport, refresh: refreshSourceData, refreshOne: refreshSingleSource } = useSourceRefresh({
    mode,
    sources: runtime?.sources ?? [],
    resetKey: `${mode}:${view}`,
    perform,
    refreshStats: refreshSourceStats,
  });

  return (
    <section className="relay-page relay-workspace-page" data-view={view}>
      <PageHeader
        title={t("nav.connections")}
        navigation={<Tabs value={view} items={tabs} onChange={(nextView) => { if (nextView === "sources") sessionStorage.setItem(CONNECTIONS_VIEW_REQUEST, nextView); else sessionStorage.removeItem(CONNECTIONS_VIEW_REQUEST); setView(nextView as ConnectionView); }} label={t("connections.views")} />}
        actions={view === "trash" ? undefined :
          <>
            {view === "accounts" ? (
              <Button variant="secondary" icon={<Upload aria-hidden />} onClick={onImport}>
                {t("connections.import")}
              </Button>
            ) : null}
            <Button
              variant="primary"
              icon={view === "accounts"
                ? <LogIn aria-hidden />
                : view === "proxies"
                  ? <Upload aria-hidden />
                  : <Plus aria-hidden />}
              busy={view === "accounts" && busy === "oauth-start"}
              disabled={view === "sources" && Boolean(busy)}
              onClick={primaryAction}
            >
              {primaryLabel}
            </Button>
          </>
        }
      />
      {view === "sources" ? <SourcesList query={query} onQuery={setQuery} sourceStats={sourceStats} refreshReport={sourceRefreshReport ? t("sources.refreshResult", sourceRefreshReport) : null} onEdit={(source) => { setEditingSource(source); setDialog("source"); }} onRefresh={refreshSingleSource} onRefreshSelected={refreshSourceData} onRefreshAll={() => refreshSourceData()} /> : null}
      {view === "accounts" ? (
        <AccountsTable
          query={query}
          onQuery={setQuery}
          onImport={onImport}
          onSignIn={startOAuth}
          onReauthenticate={signIn.reauthenticate}
          onProxy={(account) => { setProxyAccount(account); setDialog("accountProxy"); }}
          onBulkProxies={(accountIds) => { setBulkProxyAccountIds(accountIds); setDialog("bulkProxies"); }}
          onExport={(accountIds) => { setExportAccountIds(accountIds); setDialog("accountExport"); }}
        />
      ) : null}
      {view === "trash" ? <AccountTrashView /> : null}
      {view === "proxies" ? <ProxyStorageView revision={proxyRevision} diagnostics={proxyChecks} onImport={() => setDialog("proxyImport")} /> : null}
      {view === "automations" ? <AutomationsList onEdit={(task) => { setEditingAutomation(task); setDialog("automation"); }} /> : null}

      {dialog === "source" ? <SourceDialog source={editingSource} onClose={() => { setDialog(null); setEditingSource(null); }} /> : null}
      {signIn.dialogs}
      {dialog === "automation" ? <AutomationDialog task={editingAutomation} onClose={() => { setDialog(null); setEditingAutomation(null); }} /> : null}
      {dialog === "accountProxy" && proxyAccount ? <AccountProxyDialog account={proxyAccount} onClose={() => { setDialog(null); setProxyAccount(null); }} /> : null}
      {dialog === "bulkProxies" ? <BulkProxyDialog accountIds={bulkProxyAccountIds} onClose={() => setDialog(null)} /> : null}
      {dialog === "proxyImport" ? <ProxyImportDialog diagnostics={proxyChecks} onImported={() => setProxyRevision((revision) => revision + 1)} onClose={() => setDialog(null)} /> : null}
      {dialog === "accountExport" ? <AccountExportDialog accountIds={exportAccountIds} onClose={() => { setDialog(null); setExportAccountIds([]); }} /> : null}
      {busy ? <span className="sr-only" aria-live="polite">{t("common.working")}</span> : null}
    </section>
  );
}
