import { FormEvent, useEffect, useMemo, useState } from "react";
import { CirclePause, Copy, Download, Eye, EyeOff, Network, Pencil, Play, Plus, Power, RefreshCw, Trash2 } from "lucide-react";
import { useTranslation } from "react-i18next";
import { createSavedTopUpIntentAndOpen, prepareTopUpAmount, resetKey, saveKey } from "../../../../tauri";
import { defaultWakeInput, relayCommands } from "../../api/commands";
import type { AccountSummary, ImportSession, OAuthFlow, ProxyAssignmentResult, SourceSummary, WakeTask } from "../../api/types";
import {
  Button,
  Dialog,
  EmptyState,
  IconButton,
  PageHeader,
  QuotaMeter,
  SecretField,
  StatusBadge,
  Tabs,
  copyText,
} from "../../components/Ui";
import { useRelayState } from "../../state/RelayStateProvider";

type View = "sources" | "accounts" | "automations" | "remote" | "api";
type DialogKind = "source" | "oauth" | "import" | "automation" | "remote" | "deploy" | "ready" | "topup" | "accountProxy" | "bulkProxies" | null;

export function ConnectionsPage() {
  const { t } = useTranslation();
  const { mode, runtime, readyState, busy, perform, refresh } = useRelayState();
  const [view, setView] = useState<View>(mode === "zenith" ? "api" : "accounts");
  const [dialog, setDialog] = useState<DialogKind>(null);
  const [query, setQuery] = useState("");
  const [editingSource, setEditingSource] = useState<SourceSummary | null>(null);
  const [editingAutomation, setEditingAutomation] = useState<WakeTask | null>(null);
  const [proxyAccount, setProxyAccount] = useState<AccountSummary | null>(null);
  const remoteFeatures = new Set(runtime?.capabilities.features ?? []);
  const supports = (feature: string) => mode !== "remote" || remoteFeatures.has(feature);
  const canImportAccounts = mode !== "remote" || supports("account_batch_import");
  const canManageProxies = supports("account_proxies");

  useEffect(() => {
    setView(mode === "zenith" ? "api" : "accounts");
    setDialog(null);
    setEditingSource(null);
    setEditingAutomation(null);
    setProxyAccount(null);
  }, [mode]);

  useEffect(() => setQuery(""), [mode, view]);

  useEffect(() => {
    if (mode !== "remote") return;
    if (!runtime) { if (view !== "remote") setView("remote"); return; }
    if ((view === "accounts" && !supports("accounts")) || (view === "sources" && !supports("sources")) || (view === "automations" && !supports("wake_tasks"))) setView("remote");
  }, [mode, runtime, view]);

  const tabs = mode === "zenith"
    ? [{ id: "api", label: t("connections.api") }]
    : [
        ...(supports("accounts") ? [{ id: "accounts", label: t("connections.accounts") }] : []),
        ...(supports("sources") ? [{ id: "sources", label: t("connections.sources") }] : []),
        ...(supports("wake_tasks") ? [{ id: "automations", label: t("connections.automations") }] : []),
        ...(mode === "remote" ? [{ id: "remote", label: t("connections.remoteServer") }] : []),
      ];

  const primaryLabel = view === "accounts"
    ? mode === "local" ? t("accounts.signIn") : t("accounts.import")
    : view === "sources"
      ? t("sources.add")
      : view === "automations"
        ? t("automations.add")
        : view === "remote"
          ? runtime ? t("remote.refresh") : t("remote.connect")
          : readyState?.providerActive ? t("readyApi.topUp") : t("readyApi.connect");

  const primaryAction = () => {
    if (view === "accounts" && !canImportAccounts) return;
    if (view === "remote" && runtime) {
      void perform("remote-refresh", relayCommands.refreshRemoteCapabilities, "feedback.refreshed");
      return;
    }
    if (view === "api" && readyState?.providerActive) {
      setDialog("topup");
      return;
    }
    setEditingSource(null);
    setEditingAutomation(null);
    setDialog(
      view === "accounts" ? (mode === "local" ? "oauth" : "import")
        : view === "sources" ? "source"
          : view === "automations" ? "automation"
            : view === "remote" ? "remote"
              : "ready",
    );
  };

  return (
    <section className="relay-page" data-view={view}>
      <PageHeader
        title={t("nav.connections")}
        subtitle={t(`connections.subtitles.${mode}`)}
        actions={
          <>
            {view === "accounts" && mode !== "zenith" ? (
              <Button variant="secondary" icon={<Download aria-hidden />} disabled={!canImportAccounts} title={!canImportAccounts ? t("remote.capabilityUnavailable") : undefined} onClick={() => setDialog("import")}>
                {t("connections.import")}
              </Button>
            ) : null}
            <Button variant="primary" icon={view === "remote" && runtime ? <RefreshCw aria-hidden /> : <Plus aria-hidden />} disabled={view === "accounts" && !canImportAccounts} title={view === "accounts" && !canImportAccounts ? t("remote.capabilityUnavailable") : undefined} onClick={primaryAction}>
              {primaryLabel}
            </Button>
          </>
        }
      />
      <Tabs value={view} items={tabs} onChange={(id) => setView(id as View)} label={t("connections.views")} />
      {view === "sources" || view === "accounts" || view === "automations" ? <div className="table-toolbar">
        <label className="search-field">
          <span className="sr-only">{t("common.search")}</span>
          <input value={query} onChange={(event) => setQuery(event.target.value)} placeholder={t("common.search")} />
        </label>
        <Button variant="ghost" icon={<RefreshCw aria-hidden />} onClick={refresh}>{t("common.refresh")}</Button>
      </div> : null}

      {view === "sources" ? <SourcesTable query={query} onAdd={() => setDialog("source")} onEdit={(source) => { setEditingSource(source); setDialog("source"); }} /> : null}
      {view === "accounts" ? <AccountsTable query={query} canImport={canImportAccounts} canManageProxies={canManageProxies} onImport={() => setDialog("import")} onSignIn={() => setDialog("oauth")} onProxy={(account) => { setProxyAccount(account); setDialog("accountProxy"); }} onBulkProxies={() => setDialog("bulkProxies")} /> : null}
      {view === "automations" ? <AutomationsTable query={query} onAdd={() => { setEditingAutomation(null); setDialog("automation"); }} onEdit={(task) => { setEditingAutomation(task); setDialog("automation"); }} /> : null}
      {view === "remote" ? <RemoteView onConnect={() => setDialog("remote")} onDeploy={() => setDialog("deploy")} /> : null}
      {view === "api" ? <ReadyApiView connected={Boolean(readyState?.providerActive)} onConnect={() => setDialog("ready")} onTopUp={() => setDialog("topup")} /> : null}

      {dialog === "source" ? <SourceDialog source={editingSource} onClose={() => { setDialog(null); setEditingSource(null); }} /> : null}
      {dialog === "oauth" ? <OAuthDialog onClose={() => setDialog(null)} /> : null}
      {dialog === "import" ? <ImportDialog onClose={() => setDialog(null)} /> : null}
      {dialog === "automation" ? <AutomationDialog task={editingAutomation} onClose={() => { setDialog(null); setEditingAutomation(null); }} /> : null}
      {dialog === "remote" ? <RemoteDialog onClose={() => setDialog(null)} /> : null}
      {dialog === "deploy" ? <DeployDialog onClose={() => setDialog(null)} /> : null}
      {dialog === "ready" ? <ReadyApiDialog onClose={() => setDialog(null)} /> : null}
      {dialog === "topup" ? <TopUpDialog onClose={() => setDialog(null)} /> : null}
      {dialog === "accountProxy" && proxyAccount ? <AccountProxyDialog account={proxyAccount} onClose={() => { setDialog(null); setProxyAccount(null); }} /> : null}
      {dialog === "bulkProxies" ? <BulkProxyDialog onClose={() => setDialog(null)} /> : null}
      {busy ? <span className="sr-only" aria-live="polite">{t("common.working")}</span> : null}
    </section>
  );
}

function SourcesTable({ query, onAdd, onEdit }: { query: string; onAdd: () => void; onEdit: (source: SourceSummary) => void }) {
  const { t } = useTranslation();
  const { mode, runtime, perform, busy } = useRelayState();
  const canTest = mode !== "remote" || runtime?.capabilities.features.includes("diagnostics");
  if (!runtime?.sources.length) {
    return <EmptyState title={t("sources.emptyTitle")} description={t("sources.emptyDescription")} action={<Button variant="primary" onClick={onAdd}>{t("sources.add")}</Button>} />;
  }
  const sources = runtime.sources.filter((source) => matchesQuery(query, source.name, source.baseUrl, source.wireApi, source.models));
  if (!sources.length) return <NoResults />;
  return (
    <div className="relay-table-wrap">
      <table className="relay-table">
        <thead><tr><th>{t("common.status")}</th><th>{t("common.name")}</th><th>{t("sources.host")}</th><th>{t("sources.protocol")}</th><th>{t("common.models")}</th><th>{t("pool.priority")}</th><th><span className="sr-only">{t("common.actions")}</span></th></tr></thead>
        <tbody>{sources.map((source) => (
          <tr key={source.id}>
            <td><StatusBadge status={source.enabled && source.secretAvailable ? "ready" : "disabled"} label={source.enabled ? t("common.enabled") : t("common.disabled")} /></td>
            <td><strong>{source.name}</strong></td>
            <td><code>{safeHost(source.baseUrl)}</code></td>
            <td>{source.wireApi}</td>
            <td>{source.models.length}</td>
            <td>{source.priority}</td>
            <td className="row-actions">
              <IconButton label={t("common.test")} icon={<Play aria-hidden />} disabled={!canTest || busy === `test-${source.id}`} title={!canTest ? t("remote.capabilityUnavailable") : t("common.test")} onClick={() => perform(`test-${source.id}`, () => mode === "local" ? relayCommands.testSource(source.id) : relayCommands.remoteAction({ type: "test_source", id: source.id }), "feedback.checked")} />
              <IconButton label={t("common.edit")} icon={<Pencil aria-hidden />} onClick={() => onEdit(source)} />
              <IconButton label={source.enabled ? t("common.disable") : t("common.enable")} icon={<Power aria-hidden />} onClick={() => perform(`toggle-${source.id}`, () => mode === "local" ? relayCommands.setSourceEnabled(source.id, !source.enabled) : relayCommands.remoteAction({ type: "update_source", id: source.id }, { enabled: !source.enabled }), "feedback.saved")} />
              <IconButton label={t("common.delete")} icon={<Trash2 aria-hidden />} onClick={() => { if (window.confirm(t("sources.deleteConfirm"))) void perform(`delete-${source.id}`, () => mode === "local" ? relayCommands.deleteSource(source.id) : relayCommands.remoteAction({ type: "delete_source", id: source.id }), "feedback.deleted"); }} />
            </td>
          </tr>
        ))}</tbody>
      </table>
    </div>
  );
}

function AccountsTable({ query, canImport, canManageProxies, onImport, onSignIn, onProxy, onBulkProxies }: { query: string; canImport: boolean; canManageProxies: boolean; onImport: () => void; onSignIn: () => void; onProxy: (account: AccountSummary) => void; onBulkProxies: () => void }) {
  const { t } = useTranslation();
  const { mode, runtime, perform, busy } = useRelayState();
  if (!runtime?.accounts.length) {
    return <EmptyState title={t("accounts.emptyTitle")} description={t("accounts.emptyDescription")} action={<div className="inline-actions">{mode === "local" ? <Button variant="primary" onClick={onSignIn}>{t("accounts.signIn")}</Button> : null}<Button variant={mode === "local" ? "secondary" : "primary"} disabled={!canImport} title={!canImport ? t("remote.capabilityUnavailable") : undefined} onClick={onImport}>{t("accounts.import")}</Button></div>} />;
  }
  const accounts = runtime.accounts.filter((account) => matchesQuery(query, account.label, account.identityHint, account.subscription.planType, account.models));
  if (!accounts.length) return <NoResults />;
  return (
    <>
    <div className="table-toolbar"><Button variant="secondary" icon={<Network aria-hidden />} disabled={!canManageProxies} title={!canManageProxies ? t("remote.capabilityUnavailable") : undefined} onClick={onBulkProxies}>{t("proxies.assignBulk")}</Button>{mode === "local" ? <Button variant="secondary" busy={busy === "quota-all"} onClick={() => perform("quota-all", relayCommands.refreshAllAccountQuotas, "feedback.refreshed")}>{t("accounts.refreshAll")}</Button> : null}</div>
    <div className="relay-table-wrap">
      <table className="relay-table">
        <thead><tr><th>{t("common.health")}</th><th>{t("common.name")}</th><th>{t("accounts.plan")}</th><th>{t("common.quota")}</th><th>{t("proxies.proxy")}</th><th>{t("common.models")}</th><th><span className="sr-only">{t("common.actions")}</span></th></tr></thead>
        <tbody>{accounts.map((account) => (
          <tr key={account.id}>
            <td><StatusBadge status={account.health === "healthy" ? "ready" : account.health === "blocked" ? "error" : "warning"} label={t(`health.${account.health}`, { defaultValue: account.health })} /></td>
            <td><strong>{account.label}</strong><small>{account.identityHint}</small></td>
            <td>{account.subscription.planType ?? t("common.unknown")}</td>
            <td><QuotaMeter window={account.quota.primary} label={t("quota.primary")} /></td>
            <td><button type="button" className="proxy-status-button" disabled={!canManageProxies} title={!canManageProxies ? t("remote.capabilityUnavailable") : t("proxies.changeAccount")} onClick={() => onProxy(account)}><StatusBadge status={account.proxyAvailable === false ? "error" : account.proxyMode === "account" ? "info" : "ready"} label={t(`proxies.modes.${account.proxyMode ?? "direct"}`)} /></button></td>
            <td>{account.models.length}</td>
            <td className="row-actions">
              <IconButton label={t("accounts.refreshQuota")} icon={<RefreshCw aria-hidden />} disabled={mode === "remote" || busy === `quota-${account.id}`} title={mode === "remote" ? t("accounts.serverRefreshHint") : t("accounts.refreshQuota")} onClick={() => perform(`quota-${account.id}`, () => relayCommands.refreshAccountQuota(account.id), "feedback.refreshed")} />
              <IconButton label={account.draining ? t("accounts.resume") : t("accounts.drain")} icon={account.draining ? <Play aria-hidden /> : <CirclePause aria-hidden />} title={account.draining ? t("accounts.resume") : t("accounts.drain")} onClick={() => perform(`drain-${account.id}`, () => mode === "local" ? relayCommands.setAccountDraining(account.id, !account.draining) : relayCommands.remoteAction({ type: "update_account", id: account.id }, { draining: !account.draining }), "feedback.saved")} />
              <IconButton label={account.enabled ? t("common.disable") : t("common.enable")} icon={<Power aria-hidden />} title={account.enabled ? t("common.disable") : t("common.enable")} onClick={() => perform(`enable-${account.id}`, () => mode === "local" ? relayCommands.setAccountEnabled(account.id, !account.enabled) : relayCommands.remoteAction({ type: "update_account", id: account.id }, { enabled: !account.enabled }), "feedback.saved")} />
              <IconButton label={t("common.delete")} icon={<Trash2 aria-hidden />} title={t("common.delete")} onClick={() => { if (window.confirm(t("accounts.deleteConfirm"))) void perform(`delete-${account.id}`, () => mode === "local" ? relayCommands.deleteAccount(account.id) : relayCommands.remoteAction({ type: "delete_account", id: account.id }), "feedback.deleted"); }} />
            </td>
          </tr>
        ))}</tbody>
      </table>
    </div>
    </>
  );
}

function AccountProxyDialog({ account, onClose }: { account: AccountSummary; onClose: () => void }) {
  const { t } = useTranslation();
  const { mode, busy, perform } = useRelayState();
  const [proxyUrl, setProxyUrl] = useState("");
  const apply = async (value: string | null) => {
    const ok = await perform(`proxy-${account.id}`, () => mode === "local"
      ? relayCommands.setAccountProxy(account.id, value)
      : relayCommands.remoteAction({ type: "set_account_proxy", id: account.id }, { proxyUrl: value }), "feedback.saved");
    if (ok) onClose();
  };
  return <Dialog title={t("proxies.accountTitle", { name: account.label })} onClose={onClose} footer={<><Button variant="secondary" onClick={onClose}>{t("common.cancel")}</Button>{account.proxyMode === "account" ? <Button variant="secondary" busy={busy === `proxy-${account.id}`} onClick={() => apply(null)}>{t("proxies.useInherited")}</Button> : null}<Button variant="primary" busy={busy === `proxy-${account.id}`} disabled={!proxyUrl.trim()} onClick={() => apply(proxyUrl.trim())}>{t("common.save")}</Button></>}><div className="relay-form"><div className="proxy-current"><span>{t("proxies.currentMode")}</span><StatusBadge status={account.proxyAvailable === false ? "error" : "ready"} label={t(`proxies.modes.${account.proxyMode ?? "direct"}`)} /></div><SecretField label={t("proxies.proxyUrl")} value={proxyUrl} onChange={setProxyUrl} placeholder="user:password@us-proxy.example:8080" /><p className="form-note">{t("proxies.savedHidden")}</p></div></Dialog>;
}

function BulkProxyDialog({ onClose }: { onClose: () => void }) {
  const { t } = useTranslation();
  const { mode, runtime, busy, perform } = useRelayState();
  const accounts = runtime?.accounts ?? [];
  const [selected, setSelected] = useState(() => accounts.map((account) => account.id));
  const [content, setContent] = useState("");
  const [revealed, setRevealed] = useState(false);
  const [result, setResult] = useState<ProxyAssignmentResult | null>(null);
  const proxyUrls = content.split(/\r?\n/).map((value) => value.trim()).filter(Boolean);
  const selectedAccountIds = accounts.filter((account) => selected.includes(account.id)).map((account) => account.id);
  const valid = selectedAccountIds.length > 0 && proxyUrls.length >= selectedAccountIds.length;
  const toggle = (accountId: string) => setSelected((current) => current.includes(accountId) ? current.filter((id) => id !== accountId) : [...current, accountId]);
  const assign = async () => {
    let response: ProxyAssignmentResult | null = null;
    const ok = await perform("proxy-bulk", async () => {
      response = mode === "local"
        ? await relayCommands.assignAccountProxies(selectedAccountIds, proxyUrls)
        : await relayCommands.remoteAction({ type: "assign_account_proxies" }, { accountIds: selectedAccountIds, proxyUrls }) as ProxyAssignmentResult;
    }, "feedback.saved");
    if (ok) {
      setResult(response);
      setContent("");
    }
  };
  return <Dialog wide title={t("proxies.bulkTitle")} onClose={onClose} footer={<><Button variant="secondary" onClick={onClose}>{t("common.close")}</Button><Button variant="primary" busy={busy === "proxy-bulk"} disabled={!valid} onClick={assign}>{t("proxies.assign")}</Button></>}><div className="relay-form"><label className="toggle-row"><input type="checkbox" checked={selectedAccountIds.length === accounts.length && accounts.length > 0} onChange={(event) => setSelected(event.target.checked ? accounts.map((account) => account.id) : [])} /><span>{t("proxies.selectAll", { count: accounts.length })}</span></label><fieldset><legend>{t("connections.accounts")}</legend><div className="scope-grid proxy-account-grid">{accounts.map((account) => <label key={account.id}><input type="checkbox" checked={selected.includes(account.id)} onChange={() => toggle(account.id)} />{account.label}</label>)}</div></fieldset><label className="relay-field"><span>{t("proxies.proxyList")}</span><div className="proxy-list-field"><textarea className={revealed ? "" : "secret-textarea"} value={content} onChange={(event) => { setContent(event.target.value); setResult(null); }} placeholder={t("proxies.proxyListPlaceholder")} autoComplete="off" spellCheck={false} /><IconButton type="button" label={revealed ? t("common.hide") : t("common.reveal")} icon={revealed ? <EyeOff aria-hidden /> : <Eye aria-hidden />} onClick={() => setRevealed((value) => !value)} /></div></label><p className="form-note">{t("proxies.bulkHint", { selected: selectedAccountIds.length, provided: proxyUrls.length })}</p>{result ? <p role="status" className="form-note success-text">{t("proxies.bulkResult", result)}</p> : null}</div></Dialog>;
}

function AutomationsTable({ query, onAdd, onEdit }: { query: string; onAdd: () => void; onEdit: (task: WakeTask) => void }) {
  const { t } = useTranslation();
  const { mode, runtime, perform, busy } = useRelayState();
  if (!runtime?.automations.length) {
    return <EmptyState title={t("automations.emptyTitle")} description={t("automations.emptyDescription")} action={<Button variant="primary" onClick={onAdd}>{t("automations.add")}</Button>} />;
  }
  const automations = runtime.automations.filter((task) => matchesQuery(query, task.name, task.accountSelector.kind === "all_eligible" ? "" : task.accountSelector.values, task.modelPolicy.kind === "explicit" ? task.modelPolicy.value : ""));
  if (!automations.length) return <NoResults />;
  return (
    <>
      {mode === "local" ? <div className="table-toolbar"><Button variant="secondary" busy={busy === "wake-due"} onClick={() => perform("wake-due", relayCommands.runWakeConfirmations, "feedback.checked")}>{t("automations.runDue")}</Button></div> : null}
      <div className="relay-table-wrap">
        <table className="relay-table">
          <thead><tr><th>{t("common.status")}</th><th>{t("common.name")}</th><th>{t("connections.accounts")}</th><th>{t("common.quota")}</th><th>{t("common.model")}</th><th>{t("automations.lastResult")}</th><th><span className="sr-only">{t("common.actions")}</span></th></tr></thead>
          <tbody>{automations.map((task) => {
            const history = runtime.wakeHistory.filter((item) => item.taskId === task.id);
            const last = history[history.length - 1];
            return (
              <tr key={task.id}>
                <td><input type="checkbox" checked={task.enabled} aria-label={t("common.enabled")} onChange={() => perform(`automation-${task.id}`, () => mode === "local" ? relayCommands.setAutomationEnabled(task.id, !task.enabled) : relayCommands.remoteAction({ type: "update_wake_task", id: task.id }, { ...task, enabled: !task.enabled }), "feedback.saved")} /></td>
                <td><strong>{task.name}</strong></td>
                <td>{task.accountSelector.kind === "all_eligible" ? t("automations.allEligible") : task.accountSelector.kind === "account_ids" ? task.accountSelector.values.map((id) => runtime.accounts.find((account) => account.id === id)?.label ?? id).join(", ") : task.accountSelector.values.join(", ")}</td>
                <td>{task.windowKinds.map((item) => t(`quota.${item}`)).join(", ")}</td>
                <td>{task.modelPolicy.kind === "explicit" ? task.modelPolicy.value : t("automations.lightest")}</td>
                <td>{last ? t(`wake.${last.outcome}`, { defaultValue: last.outcome }) : t("common.never")}</td>
                <td className="row-actions"><IconButton label={t("common.edit")} icon={<Pencil aria-hidden />} onClick={() => onEdit(task)} /><IconButton label={t("common.test")} icon={<Play aria-hidden />} disabled={busy === `test-${task.id}`} onClick={() => perform(`test-${task.id}`, () => mode === "local" ? relayCommands.testAutomation(task.id) : relayCommands.remoteAction({ type: "test_wake_task", id: task.id }), "feedback.checked")} /><IconButton label={t("common.delete")} icon={<Trash2 aria-hidden />} onClick={() => { if (window.confirm(t("automations.deleteConfirm"))) void perform(`delete-${task.id}`, () => mode === "local" ? relayCommands.deleteAutomation(task.id) : relayCommands.remoteAction({ type: "delete_wake_task", id: task.id }), "feedback.deleted"); }} /></td>
              </tr>
            );
          })}</tbody>
        </table>
      </div>
    </>
  );
}

function RemoteView({ onConnect, onDeploy }: { onConnect: () => void; onDeploy: () => void }) {
  const { t } = useTranslation();
  const { runtime, perform, busy } = useRelayState();
  if (!runtime) return <EmptyState title={t("remote.emptyTitle")} description={t("remote.emptyDescription")} action={<div className="inline-actions"><Button variant="primary" onClick={onConnect}>{t("remote.connectExisting")}</Button><Button variant="secondary" onClick={onDeploy}>{t("remote.deployNew")}</Button></div>} />;
  return <section className="remote-summary"><div className="remote-status"><StatusBadge status={runtime.gateway.running ? "ready" : "warning"} label={runtime.gateway.running ? t("common.online") : t("common.offline")} /><div><strong>{runtime.runtimeTarget.origin}</strong><small>{runtime.runtimeTarget.serverId}</small></div></div><dl className="detail-list"><div><dt>{t("remote.version")}</dt><dd>{runtime.runtimeTarget.version}</dd></div><div><dt>{t("gateway.endpoint")}</dt><dd><code>{runtime.gateway.baseUrl}</code></dd></div><div><dt>{t("remote.capabilities")}</dt><dd>{runtime.capabilities.features.length}</dd></div></dl><div className="inline-actions"><Button variant="secondary" busy={busy === "remote-refresh"} onClick={() => perform("remote-refresh", relayCommands.refreshRemoteCapabilities, "feedback.refreshed")}>{t("remote.refresh")}</Button><Button variant="danger" onClick={() => { if (window.confirm(t("remote.disconnectConfirm"))) void perform("remote-disconnect", relayCommands.disconnectRemote, "feedback.disconnected"); }}>{t("remote.disconnect")}</Button></div></section>;
}

function ReadyApiView({ connected, onConnect, onTopUp }: { connected: boolean; onConnect: () => void; onTopUp: () => void }) {
  const { t } = useTranslation();
  const { readyStats, perform } = useRelayState();
  return <section className="ready-api-connection"><div className="recommended-line"><div><strong>Zenith API</strong><small>https://api.zenithmarket.dev/v1</small></div><span>{t("common.recommended")}</span></div><StatusBadge status={connected ? "ready" : "warning"} label={connected ? t("common.connected") : t("common.notConfigured")} /><p>{t("readyApi.connectionHint")}</p>{connected ? <><dl className="detail-list"><div><dt>{t("readyApi.balance")}</dt><dd>{readyStats?.balance ?? "-"}</dd></div><div><dt>{t("usage.requests")}</dt><dd>{readyStats?.requestsDisplay ?? readyStats?.requests ?? "-"}</dd></div></dl><div className="inline-actions"><Button variant="secondary" onClick={onTopUp}>{t("readyApi.topUp")}</Button><Button variant="secondary" onClick={onConnect}>{t("readyApi.updateKey")}</Button><Button variant="danger" onClick={() => { if (window.confirm(t("readyApi.disconnectConfirm"))) void perform("ready-disconnect", resetKey, "feedback.disconnected"); }}>{t("remote.disconnect")}</Button></div></> : <Button variant="primary" onClick={onConnect}>{t("readyApi.connect")}</Button>}</section>;
}

function SourceDialog({ source, onClose }: { source: SourceSummary | null; onClose: () => void }) {
  const { t } = useTranslation();
  const { mode, perform, busy } = useRelayState();
  const [name, setName] = useState(source?.name ?? "");
  const [baseUrl, setBaseUrl] = useState(source?.baseUrl ?? "");
  const [apiKey, setApiKey] = useState("");
  const [wireApi, setWireApi] = useState<SourceSummary["wireApi"]>(source?.wireApi ?? "responses");
  const [models, setModels] = useState(source?.models.join(", ") ?? "");
  const [allowed, setAllowed] = useState(source?.allowedModels.join(", ") ?? "");
  const [excluded, setExcluded] = useState(source?.excludedModels.join(", ") ?? "");
  const [priority, setPriority] = useState(source?.priority ?? 0);
  const [weight, setWeight] = useState(source?.weight ?? 100);
  const submit = async (event: FormEvent) => {
    event.preventDefault();
    const base = { name, baseUrl, wireApi, models: parseList(models), allowedModels: parseList(allowed), excludedModels: parseList(excluded), draining: source?.draining ?? false, priority, weight };
    const ok = await perform("source-save", async () => {
      if (!source) {
        const payload = { ...base, apiKey };
        if (mode === "local") await relayCommands.createSource(payload);
        else await relayCommands.remoteAction({ type: "create_source" }, payload);
        return;
      }
      if (mode === "local") {
        await relayCommands.updateSource({ sourceId: source.id, ...base });
        if (apiKey) await relayCommands.rotateSourceKey(source.id, apiKey);
      } else {
        await relayCommands.remoteAction({ type: "update_source", id: source.id }, { ...base, ...(apiKey ? { apiKey } : {}) });
      }
    }, source ? "feedback.saved" : "feedback.sourceAdded");
    if (ok) onClose();
  };
  return <Dialog wide title={source ? t("sources.edit") : t("sources.add")} onClose={onClose} footer={<><Button variant="secondary" onClick={onClose}>{t("common.cancel")}</Button><Button variant="primary" busy={busy === "source-save"} disabled={!source && !apiKey.trim()} onClick={() => document.querySelector<HTMLFormElement>("#source-form")?.requestSubmit()}>{t("common.save")}</Button></>}><form id="source-form" className="relay-form" onSubmit={submit}><label className="relay-field"><span>{t("common.name")}</span><input value={name} onChange={(event) => setName(event.target.value)} required /></label><label className="relay-field"><span>{t("sources.address")}</span><input type="url" value={baseUrl} onChange={(event) => setBaseUrl(event.target.value)} placeholder="https://api.example.com/v1" required /></label><label className="relay-field"><span>{t("sources.protocol")}</span><select value={wireApi} onChange={(event) => setWireApi(event.target.value as SourceSummary["wireApi"])}><option value="responses">Responses API</option><option value="chat_completions">Chat Completions</option></select></label><SecretField label={source ? t("sources.replaceKey") : t("sources.apiKey")} value={apiKey} onChange={setApiKey} /><label className="relay-field"><span>{t("common.models")}</span><input value={models} onChange={(event) => setModels(event.target.value)} placeholder="gpt-5.4, gpt-5.4-mini" /></label><div className="settings-row"><label><span>{t("pool.allowedModels")}</span><input value={allowed} onChange={(event) => setAllowed(event.target.value)} /></label><label><span>{t("pool.excludedModels")}</span><input value={excluded} onChange={(event) => setExcluded(event.target.value)} /></label></div><div className="settings-row"><label><span>{t("pool.priority")}</span><input type="number" value={priority} onChange={(event) => setPriority(Number(event.target.value))} /></label><label><span>{t("pool.weight")}</span><input type="number" min="1" value={weight} onChange={(event) => setWeight(Number(event.target.value))} /></label></div></form></Dialog>;
}

function OAuthDialog({ onClose }: { onClose: () => void }) {
  const { t } = useTranslation();
  const { perform, busy } = useRelayState();
  const [flow, setFlow] = useState<OAuthFlow | null>(null);
  const [callbackUrl, setCallbackUrl] = useState("");
  const [loginId, setLoginId] = useState("");
  const start = async () => {
    const result: { current: OAuthFlow | null } = { current: null };
    const ok = await perform("oauth-start", async () => { result.current = await relayCommands.startOAuth(); });
    if (ok) setFlow(result.current);
  };
  const resume = async () => {
    const result: { current: OAuthFlow | null } = { current: null };
    const ok = await perform("oauth-resume", async () => { result.current = await relayCommands.resumeOAuth(loginId.trim()); });
    if (ok) setFlow(result.current);
  };
  const finish = async () => {
    if (!flow) return;
    const ok = await perform("oauth-complete", async () => {
      if (callbackUrl.trim()) await relayCommands.submitOAuthCallback(flow.loginId, callbackUrl.trim());
      else await relayCommands.oauthStatus(flow.loginId);
      await relayCommands.completeOAuth(flow.loginId);
    }, "feedback.accountAdded");
    if (ok) onClose();
  };
  const cancel = async () => {
    if (flow) await perform("oauth-cancel", () => relayCommands.cancelOAuth(flow.loginId));
    onClose();
  };
  return <Dialog title={t("accounts.signIn")} onClose={cancel} footer={<><Button variant="secondary" onClick={cancel}>{t("common.cancel")}</Button>{flow ? <Button variant="primary" busy={busy === "oauth-complete"} onClick={finish}>{t("accounts.finishSignIn")}</Button> : <Button variant="primary" busy={busy === "oauth-start"} onClick={start}>{t("accounts.openSignIn")}</Button>}</>}>{flow ? <div className="relay-form"><p>{t("accounts.browserOpened")}</p><label className="relay-field"><span>{t("accounts.callbackUrl")}</span><input value={callbackUrl} onChange={(event) => setCallbackUrl(event.target.value)} placeholder={flow.redirectUri} /></label><a href={flow.authorizationUrl} target="_blank" rel="noreferrer">{t("accounts.reopenSignIn")}</a><small>{t("accounts.oauthExpires", { value: new Date(flow.expiresAtMs).toLocaleTimeString() })}</small></div> : <div className="relay-form"><p>{t("accounts.oauthDescription")}</p><label className="relay-field"><span>{t("accounts.resumeLoginId")}</span><div className="inline-actions"><input value={loginId} onChange={(event) => setLoginId(event.target.value)} /><Button variant="secondary" busy={busy === "oauth-resume"} disabled={!loginId.trim()} onClick={resume}>{t("common.resume")}</Button></div></label></div>}</Dialog>;
}

function ImportDialog({ onClose }: { onClose: () => void }) {
  const { t } = useTranslation();
  const { mode, perform, busy } = useRelayState();
  const [content, setContent] = useState("");
  const [session, setSession] = useState<ImportSession | null>(null);
  const [resumeId, setResumeId] = useState("");
  const [ownedSessionId, setOwnedSessionId] = useState<string | null>(null);
  const [selected, setSelected] = useState<string[]>([]);
  const [commandFailed, setCommandFailed] = useState(false);
  const [completed, setCompleted] = useState<Array<{ itemId: string; code: string }> | null>(null);
  const acceptSession = (next: ImportSession) => {
    setSession(next);
    setOwnedSessionId(next.sessionId);
    setCommandFailed(false);
    setCompleted(null);
    setSelected(next.preview.rows
      .filter((row) => row.selectable && row.defaultSelected)
      .map((row) => row.itemId));
  };
  const cancel = async () => {
    const sessionId = session?.sessionId ?? ownedSessionId;
    if (mode === "local" && sessionId && !completed) await perform("import-cancel", () => relayCommands.cancelImport(sessionId));
    onClose();
  };
  const preview = async () => {
    if (mode === "local") {
      const result: { current: ImportSession | null } = { current: null };
      const ok = await perform("import-preview", async () => {
        const started = await relayCommands.startImport(content);
        setResumeId(started.sessionId);
        setOwnedSessionId(started.sessionId);
        result.current = await relayCommands.prepareImport(started.sessionId, true);
      });
      if (ok && result.current) acceptSession(result.current);
      else if (!ok) setCommandFailed(true);
      return;
    }
    const result: { current: ImportSession | null } = { current: null };
    const ok = await perform("import-preview", async () => {
      result.current = await relayCommands.remoteAction({ type: "preview_account_batch_import" }, { content }) as ImportSession;
    });
    if (ok && result.current) acceptSession(result.current);
    else if (!ok) setCommandFailed(true);
  };
  const resume = async () => {
    const result: { current: ImportSession | null } = { current: null };
    const ok = await perform("import-resume", async () => { result.current = await relayCommands.resumeImport(resumeId.trim()); });
    if (ok && result.current) acceptSession(result.current);
    else if (!ok) setCommandFailed(true);
  };
  const confirm = async () => {
    if (!session) return;
    if (mode === "local") {
      const result: { current: Awaited<ReturnType<typeof relayCommands.confirmImport>> | null } = { current: null };
      const ok = await perform("import-confirm", async () => { result.current = await relayCommands.confirmImport(session.sessionId, selected); });
      if (!ok) {
        setSession(null);
        setSelected([]);
        setCommandFailed(true);
        return;
      }
      const failures = (result.current?.results ?? [])
        .filter((item) => item.status === "failed")
        .map((item) => ({ itemId: item.itemId, code: item.error?.code ?? "unknown" }));
      if (failures.length) {
        setCompleted(failures);
        return;
      }
      onClose();
      return;
    }
    const result: { current: Awaited<ReturnType<typeof relayCommands.confirmImport>> | null } = { current: null };
    const ok = await perform("import-confirm", async () => {
      result.current = await relayCommands.remoteAction(
        { type: "confirm_account_batch_import" },
        { sessionId: session.sessionId, selectedItemIds: selected },
      ) as Awaited<ReturnType<typeof relayCommands.confirmImport>>;
    }, "feedback.accountAdded");
    if (!ok) {
      setCommandFailed(true);
      return;
    }
    const failures = (result.current?.results ?? [])
      .filter((item) => item.status === "failed")
      .map((item) => ({ itemId: item.itemId, code: item.error?.code ?? "unknown" }));
    if (failures.length) setCompleted(failures);
    else onClose();
  };
  const toggle = (itemId: string) => setSelected((current) => current.includes(itemId)
    ? current.filter((id) => id !== itemId)
    : [...current, itemId]);
  const footer = completed
    ? <Button variant="primary" onClick={cancel}>{t("common.close")}</Button>
    : <><Button variant="secondary" onClick={cancel}>{t("common.cancel")}</Button>{session ? <Button variant="primary" busy={busy === "import-confirm"} disabled={selected.length === 0} onClick={confirm}>{t("accounts.confirmImport", { count: selected.length })}</Button> : <Button variant="primary" busy={busy === "import-preview"} disabled={!content.trim()} onClick={preview}>{t("accounts.preview")}</Button>}</>;
  const body = completed ? <div role="alert" className="relay-form"><strong>{t("accounts.importIncomplete")}</strong><p>{t("accounts.importIncompleteHint", { count: completed.length })}</p><ul>{completed.map((failure) => <li key={failure.itemId}><code>{failure.code}</code></li>)}</ul></div> : session ? <div className="import-preview"><table className="relay-table"><thead><tr><th><span className="sr-only">{t("accounts.selectImport")}</span></th><th>{t("common.status")}</th><th>{t("common.name")}</th><th>{t("accounts.identity")}</th><th>{t("accounts.plan")}</th></tr></thead><tbody>{session.preview.rows.map((row) => {
    const badge = row.status === "invalid" ? "error" : row.status === "quota_failed" ? "warning" : row.status === "existing" ? "info" : "ready";
    return <tr key={row.itemId}><td><input type="checkbox" checked={selected.includes(row.itemId)} disabled={!row.selectable} aria-label={t("accounts.selectImportRow", { name: row.label })} onChange={() => toggle(row.itemId)} /></td><td><StatusBadge status={badge} label={t(`accounts.importStatus.${row.status}`, { defaultValue: row.status })} /></td><td>{row.label}{row.error ? <small className="error-text">{t("accounts.importIssue", { code: row.error.code })}</small> : row.warnings.length ? <small>{row.warnings.map((warning) => warning.code).join(", ")}</small> : null}</td><td><code>{row.identity}</code></td><td>{row.plan ?? "-"}</td></tr>;
  })}</tbody></table></div> : <div className="relay-form"><label className="relay-field"><span>{t("accounts.importData")}</span><textarea value={content} onChange={(event) => setContent(event.target.value)} placeholder={mode === "local" ? t("accounts.importPlaceholder") : t("accounts.remoteImportPlaceholder")} spellCheck={false} /></label>{mode === "local" ? <label className="relay-field"><span>{t("accounts.resumeImportId")}</span><div className="inline-actions"><input value={resumeId} onChange={(event) => setResumeId(event.target.value)} /><Button variant="secondary" busy={busy === "import-resume"} disabled={!resumeId.trim()} onClick={resume}>{t("common.resume")}</Button></div></label> : null}</div>;
  return <Dialog wide title={t("accounts.import")} onClose={cancel} footer={footer}>{commandFailed ? <p role="alert" className="form-note error-text">{t("accounts.importCommandFailed")}</p> : null}{body}</Dialog>;
}

function AutomationDialog({ task, onClose }: { task: WakeTask | null; onClose: () => void }) {
  const { t } = useTranslation();
  const { mode, runtime, perform, busy } = useRelayState();
  const [name, setName] = useState(task?.name ?? t("automations.defaultName"));
  const [automatic, setAutomatic] = useState(task?.executionPolicy !== "require_confirmation");
  const [windowKinds, setWindowKinds] = useState<WakeTask["windowKinds"]>(task?.windowKinds ?? ["primary", "secondary"]);
  const [selectorKind, setSelectorKind] = useState<WakeTask["accountSelector"]["kind"]>(task?.accountSelector.kind ?? "all_eligible");
  const [accountIds, setAccountIds] = useState<string[]>(task?.accountSelector.kind === "account_ids" ? task.accountSelector.values : []);
  const [tags, setTags] = useState(task?.accountSelector.kind === "tags" ? task.accountSelector.values.join(", ") : "");
  const [modelKind, setModelKind] = useState<WakeTask["modelPolicy"]["kind"]>(task?.modelPolicy.kind ?? "lightest_supported");
  const [modelId, setModelId] = useState(task?.modelPolicy.kind === "explicit" ? task.modelPolicy.value : "");
  const availableModels = useMemo(() => {
    const accounts = (runtime?.accounts ?? []).filter((account) => selectorKind !== "account_ids" || accountIds.includes(account.id));
    const modelSets = accounts.map((account) => account.models.filter((model) => (account.allowedModels.length === 0 || account.allowedModels.some((allowed) => allowed.toLowerCase() === model.toLowerCase())) && !account.excludedModels.some((excluded) => excluded.toLowerCase() === model.toLowerCase())));
    const models = selectorKind === "account_ids" && modelSets.length > 1
      ? modelSets[0].filter((model) => modelSets.slice(1).every((set) => set.some((candidate) => candidate.toLowerCase() === model.toLowerCase())))
      : modelSets.flat();
    return [...new Set([...(modelId ? [modelId] : []), ...models])].sort();
  }, [accountIds, modelId, runtime?.accounts, selectorKind]);
  const toggleWindow = (kind: WakeTask["windowKinds"][number]) => setWindowKinds((current) => current.includes(kind) ? current.filter((item) => item !== kind) : [...current, kind]);
  const toggleAccount = (id: string) => setAccountIds((current) => current.includes(id) ? current.filter((item) => item !== id) : [...current, id]);
  const parsedTags = tags.split(",").map((tag) => tag.trim()).filter(Boolean);
  const valid = Boolean(name.trim() && windowKinds.length && (selectorKind !== "account_ids" || accountIds.length) && (selectorKind !== "tags" || parsedTags.length) && (modelKind !== "explicit" || modelId));
  const save = async () => {
    const now = Date.now();
    const accountSelector = selectorKind === "account_ids" ? { kind: selectorKind, values: accountIds } : selectorKind === "tags" ? { kind: selectorKind, values: parsedTags } : { kind: selectorKind };
    const modelPolicy = modelKind === "explicit" ? { kind: modelKind, value: modelId } : { kind: modelKind };
    const base = { ...defaultWakeInput(name), enabled: task?.enabled ?? true, accountSelector, windowKinds, modelPolicy, executionPolicy: automatic ? "automatic" as const : "require_confirmation" as const, jitterSeconds: task?.jitterSeconds ?? 0, maxAttemptsPerCycle: task?.maxAttemptsPerCycle ?? 1 };
    const remoteInput = task ? { ...task, ...base, updatedAtMs: now } : { ...base, id: "", trigger: { kind: "quota_full" }, fallbackSchedule: null, createdAtMs: now, updatedAtMs: now };
    const id = task ? `automation-update-${task.id}` : "automation-create";
    const ok = await perform(id, () => mode === "local" ? (task ? relayCommands.updateAutomation(task.id, base) : relayCommands.createAutomation(base)) : relayCommands.remoteAction({ type: task ? "update_wake_task" : "create_wake_task", ...(task ? { id: task.id } : {}) }, remoteInput), task ? "feedback.saved" : "feedback.automationAdded");
    if (ok) onClose();
  };
  return <Dialog wide title={task ? t("automations.edit") : t("automations.add")} onClose={onClose} footer={<><Button variant="secondary" onClick={onClose}>{t("common.cancel")}</Button><Button variant="primary" busy={busy === (task ? `automation-update-${task.id}` : "automation-create")} disabled={!valid} onClick={save}>{t("common.save")}</Button></>}><div className="relay-form"><label className="relay-field"><span>{t("common.name")}</span><input value={name} onChange={(event) => setName(event.target.value)} /></label><div className="form-row"><span>{t("automations.windows")}</span><label><input type="checkbox" checked={windowKinds.includes("primary")} onChange={() => toggleWindow("primary")} />{t("quota.primary")}</label><label><input type="checkbox" checked={windowKinds.includes("secondary")} onChange={() => toggleWindow("secondary")} />{t("quota.secondary")}</label></div><label className="relay-field"><span>{t("automations.accountSelection")}</span><select value={selectorKind} onChange={(event) => setSelectorKind(event.target.value as WakeTask["accountSelector"]["kind"])}><option value="all_eligible">{t("automations.allEligible")}</option><option value="account_ids">{t("automations.selectedAccounts")}</option><option value="tags">{t("automations.matchingTags")}</option></select></label>{selectorKind === "account_ids" ? <fieldset><legend>{t("automations.selectedAccounts")}</legend><div className="scope-grid">{runtime?.accounts.map((account) => <label key={account.id}><input type="checkbox" checked={accountIds.includes(account.id)} onChange={() => toggleAccount(account.id)} />{account.label}</label>)}</div></fieldset> : null}{selectorKind === "tags" ? <label className="relay-field"><span>{t("automations.tags")}</span><input value={tags} onChange={(event) => setTags(event.target.value)} placeholder={t("automations.tagsPlaceholder")} /></label> : null}<label className="relay-field"><span>{t("automations.modelPolicy")}</span><select value={modelKind} onChange={(event) => setModelKind(event.target.value as WakeTask["modelPolicy"]["kind"])}><option value="lightest_supported">{t("automations.lightest")}</option><option value="explicit">{t("automations.explicitModel")}</option></select></label>{modelKind === "explicit" ? <label className="relay-field"><span>{t("common.model")}</span><select value={modelId} onChange={(event) => setModelId(event.target.value)}><option value="">{t("automations.selectModel")}</option>{availableModels.map((model) => <option key={model} value={model}>{model}</option>)}</select></label> : null}<label className="toggle-row"><input type="checkbox" checked={automatic} onChange={(event) => setAutomatic(event.target.checked)} /><span>{t("automations.automatic")}</span></label><p className="form-note">{t("automations.fixedPrompt")}</p></div></Dialog>;
}

function RemoteDialog({ onClose }: { onClose: () => void }) {
  const { t } = useTranslation();
  const { perform, busy } = useRelayState();
  const [baseUrl, setBaseUrl] = useState("");
  const [token, setToken] = useState("");
  const [allowInsecure, setAllowInsecure] = useState(false);
  const [confirmIdentityChange, setConfirmIdentityChange] = useState(false);
  const insecure = baseUrl.trim().toLowerCase().startsWith("http://");
  const connect = async () => { const ok = await perform("remote-connect", () => relayCommands.connectRemote({ baseUrl, managementToken: token, allowInsecureHttp: insecure && allowInsecure, confirmIdentityChange }), "feedback.connected"); if (ok) onClose(); };
  return <Dialog title={t("remote.connectExisting")} onClose={onClose} footer={<><Button variant="secondary" onClick={onClose}>{t("common.cancel")}</Button><Button variant="primary" busy={busy === "remote-connect"} disabled={!baseUrl || !token || (insecure && !allowInsecure)} onClick={connect}>{t("remote.testAndConnect")}</Button></>}><div className="relay-form"><label className="relay-field"><span>{t("remote.address")}</span><input type="url" value={baseUrl} onChange={(event) => setBaseUrl(event.target.value)} placeholder="https://relay.example.com" /></label><SecretField label={t("remote.token")} value={token} onChange={setToken} />{insecure ? <label className="check-line"><input type="checkbox" checked={allowInsecure} onChange={(event) => setAllowInsecure(event.target.checked)} /><span>{t("remote.allowInsecure")}</span></label> : null}<label className="check-line"><input type="checkbox" checked={confirmIdentityChange} onChange={(event) => setConfirmIdentityChange(event.target.checked)} /><span>{t("remote.confirmIdentityChange")}</span></label><p className="form-note">{t("remote.identityHint")}</p></div></Dialog>;
}

function DeployDialog({ onClose }: { onClose: () => void }) {
  const { t } = useTranslation();
  const { perform, busy } = useRelayState();
  const [url, setUrl] = useState("");
  const [plan, setPlan] = useState<{ directory: string; managementToken: string; vaultKey: string; composeCommand: string } | null>(null);
  const generate = async () => { const result: { current: typeof plan } = { current: null }; const ok = await perform("remote-deploy", async () => { result.current = await relayCommands.prepareRemoteDeployment(url); }, "feedback.deploymentPrepared"); if (ok) setPlan(result.current); };
  return <Dialog title={t("remote.deployNew")} onClose={onClose} footer={<><Button variant="secondary" onClick={onClose}>{t("common.close")}</Button>{!plan ? <Button variant="primary" busy={busy === "remote-deploy"} disabled={!url} onClick={generate}>{t("remote.generate")}</Button> : null}</>}>{plan ? <div className="deployment-result"><StatusBadge status="ready" label={t("common.ready")} /><label><span>{t("remote.bundlePath")}</span><code>{plan.directory}</code></label><div className="relay-field"><span>{t("remote.token")}</span><div className="endpoint-line"><input aria-label={t("remote.token")} type="password" value={plan.managementToken} readOnly /><Button variant="secondary" icon={<Copy aria-hidden />} onClick={() => copyText(plan.managementToken)}>{t("common.copy")}</Button></div></div><div className="relay-field"><span>{t("remote.vaultKey")}</span><div className="endpoint-line"><input aria-label={t("remote.vaultKey")} type="password" value={plan.vaultKey} readOnly /><Button variant="secondary" icon={<Copy aria-hidden />} onClick={() => copyText(plan.vaultKey)}>{t("common.copy")}</Button></div></div><label><span>{t("remote.command")}</span><code>{plan.composeCommand}</code></label><p>{t("remote.secretOnce")}</p><p>{t("remote.deployHint")}</p></div> : <label className="relay-field"><span>{t("remote.publicUrl")}</span><input type="url" value={url} onChange={(event) => setUrl(event.target.value)} placeholder="https://relay.example.com" /></label>}</Dialog>;
}

function ReadyApiDialog({ onClose }: { onClose: () => void }) {
  const { t } = useTranslation();
  const { perform, busy } = useRelayState();
  const [apiKey, setApiKey] = useState("");
  const save = async () => { const ok = await perform("ready-save", () => saveKey(apiKey), "feedback.connected"); if (ok) onClose(); };
  return <Dialog title={t("readyApi.connect")} onClose={onClose} footer={<><Button variant="secondary" onClick={onClose}>{t("common.cancel")}</Button><Button variant="primary" busy={busy === "ready-save"} disabled={!apiKey.trim()} onClick={save}>{t("common.save")}</Button></>}><div className="relay-form"><div className="recommended-line"><strong>Zenith API</strong><span>{t("common.recommended")}</span></div><SecretField label={t("readyApi.key")} value={apiKey} onChange={setApiKey} /></div></Dialog>;
}

function TopUpDialog({ onClose }: { onClose: () => void }) {
  const { t } = useTranslation();
  const { perform, busy } = useRelayState();
  const [amount, setAmount] = useState("10");
  const submit = async () => {
    const prepared = await prepareTopUpAmount(amount);
    if (!prepared.valid) return;
    const ok = await perform("ready-topup", () => createSavedTopUpIntentAndOpen(prepared.amountCents), "feedback.topUpOpened");
    if (ok) onClose();
  };
  return <Dialog title={t("readyApi.topUp")} onClose={onClose} footer={<><Button variant="secondary" onClick={onClose}>{t("common.cancel")}</Button><Button variant="primary" busy={busy === "ready-topup"} disabled={!amount.trim()} onClick={submit}>{t("readyApi.openTopUp")}</Button></>}><label className="relay-field"><span>{t("readyApi.amount")}</span><input inputMode="decimal" value={amount} onChange={(event) => setAmount(event.target.value)} /></label></Dialog>;
}

function safeHost(value: string) {
  try { return new URL(value).host; } catch { return value; }
}

function matchesQuery(query: string, ...values: Array<string | string[] | null | undefined>) {
  const normalized = query.trim().toLocaleLowerCase();
  return !normalized || values.flatMap((value) => Array.isArray(value) ? value : value ?? []).some((value) => value.toLocaleLowerCase().includes(normalized));
}

function parseList(value: string) {
  return [...new Set(value.split(/[\n,]/).map((item) => item.trim()).filter(Boolean))];
}

function NoResults() {
  const { t } = useTranslation();
  return <EmptyState title={t("common.noResults")} description={t("common.noResultsHint")} />;
}
