import { ChangeEvent, useEffect, useRef, useState } from "react";
import { Check, Clock3, Copy, Download, ExternalLink, Eye, ListMinus, ListPlus, Loader2, LogIn, Pencil, Play, Plus, Power, RefreshCw, Trash2, Upload } from "lucide-react";
import { useTranslation } from "react-i18next";
import { defaultWakeInput, relayCommands } from "../../api/commands";
import type { AccountExportFormat, AccountSummary, OAuthFlow, SourceSummary, WakeTask } from "../../api/types";
import { SourceProtocolBindingsSummary } from "../../components/SourceProtocolRoutingDisclosure";
import {
  effectiveSourceProtocolBindings,
  sourceSupportsNativeResponses,
  sourceSupportsWireApi,
} from "../../sourceProtocolBindings";
import { sortModelIdsForLauncher } from "../../modelGroups";
import {
  Button,
  ActionMenu,
  ActionMenuItem,
  Dialog,
  EmptyState,
  IconButton,
  OptionMenu,
  PageHeader,
  SecretField,
  SettingToggle,
  StatusBadge,
  StatusIcon,
  Tabs,
  copyText,
  operationalStatusTone,
  transientCandidateTone,
  useConfirm,
} from "../../components/Ui";
import { useOAuthSignIn } from "../../hooks/useOAuthSignIn";
import { useRelayState } from "../../state/RelayStateProvider";
import { AccountProxyDialog, BulkProxyDialog, ProxyImportDialog, ProxyStorageView, useProxyPool } from "./ProxyDialogs";
import { NoResults, matchesQuery } from "./connectionHelpers";
import { AccountsTable } from "./AccountsTable";
import { MarkdownPreview } from "./MarkdownPreview";
import { SourceDialog } from "./SourceDialog";

type View = "sources" | "accounts" | "proxies" | "automations" | "remote";
type DialogKind = "source" | "automation" | "remote" | "deploy" | "accountProxy" | "bulkProxies" | "proxyImport" | "oauthSetup" | "accountExport" | null;
const CONNECTIONS_VIEW_REQUEST = "relay.connections.requestedView";
const MAX_ZENITH_EXPORT_DESCRIPTION_LENGTH = 2_000;

const accountExportFormats: Array<{ value: AccountExportFormat; label: string; multiple: boolean }> = [
  { value: "zenith", label: "Zenith", multiple: true },
  { value: "sub2api", label: "sub2api", multiple: true },
  { value: "cpa", label: "CPA", multiple: false },
  { value: "cockpit", label: "Cockpit Tools", multiple: true },
  { value: "9router", label: "9router", multiple: true },
  { value: "codex", label: "ChatGPT", multiple: false },
  { value: "axon_hub", label: "AxonHub", multiple: false },
  { value: "codex_manager", label: "Codex-Manager", multiple: true },
];

export function ConnectionsPage({ onImport }: { onImport: () => void }) {
  const { t } = useTranslation();
  const { mode, runtime, busy, perform, refresh } = useRelayState();
  const [view, setView] = useState<View>(mode === "zenith" ? "sources" : "accounts");
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
  const canImportAccounts = mode !== "remote" || supports("account_batch_import");
  const canManageProxies = supports("account_proxies");
  const canExportAccounts = supports("account_export");
  const showTableToolbar = view === "sources"
    ? Boolean(runtime?.sources.length)
    : view === "automations" && Boolean(runtime?.automations.length);

  useEffect(() => {
    const requested = mode === "zenith" ? null : sessionStorage.getItem(CONNECTIONS_VIEW_REQUEST);
    setView((current) => mode === "zenith" ? "sources" : requested === "sources" || current === "sources" ? "sources" : "accounts");
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
    if (!runtime) { if (view !== "remote") setView("remote"); return; }
    if ((view === "accounts" && !supports("accounts")) || (view === "sources" && !supports("sources")) || (view === "automations" && !supports("wake_tasks"))) setView("remote");
  }, [mode, runtime, view]);

  const tabs = mode === "zenith"
      ? [{ id: "sources", label: t("connections.sources") }]
    : [
        ...(supports("accounts") ? [{ id: "accounts", label: t("connections.accounts") }] : []),
        ...(supports("sources") ? [{ id: "sources", label: t("connections.sources") }] : []),
        ...(mode === "local" ? [{ id: "proxies", label: t("proxies.storage") }] : []),
        ...(supports("wake_tasks") ? [{ id: "automations", label: t("connections.automations") }] : []),
        ...(mode === "remote" ? [{ id: "remote", label: t("connections.remoteServer") }] : []),
      ];

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
      <Tabs value={view} items={tabs} onChange={(id) => { if (id === "sources") sessionStorage.setItem(CONNECTIONS_VIEW_REQUEST, id); else sessionStorage.removeItem(CONNECTIONS_VIEW_REQUEST); setView(id as View); }} label={t("connections.views")} />
      {showTableToolbar ? <div className={`table-toolbar${view === "sources" ? " relay-compact-content" : ""}`}>
        <label className="search-field">
          <span className="sr-only">{t("common.search")}</span>
          <input value={query} onChange={(event) => setQuery(event.target.value)} placeholder={t("common.search")} />
        </label>
        {view === "automations" && mode === "local" ? <Button variant="secondary" icon={<Play aria-hidden />} busy={busy === "wake-due"} onClick={() => perform("wake-due", relayCommands.runWakeConfirmations, "feedback.checked")}>{t("automations.runDue")}</Button> : null}
        <Button variant="secondary" icon={<RefreshCw aria-hidden />} onClick={refresh}>{t("common.refresh")}</Button>
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

function SourcesTable({ query, onEdit }: { query: string; onEdit: (source: SourceSummary) => void }) {
  const { t, i18n } = useTranslation();
  const { mode, runtime, perform, activateCodexProfile, busy } = useRelayState();
  const confirm = useConfirm();
  const [nowMs, setNowMs] = useState(Date.now());
  const sourceCooldownDeadline = Math.min(...(runtime?.gateway.routingOrder ?? [])
    .flatMap((candidate) => candidate.kind === "api_source" && candidate.nextRetryAtMs != null && candidate.nextRetryAtMs > nowMs ? [candidate.nextRetryAtMs] : []));
  useEffect(() => {
    if (!Number.isFinite(sourceCooldownDeadline)) return;
    const timer = window.setTimeout(() => setNowMs(Date.now()), sourceCooldownDeadline - nowMs < 60 * 60_000 ? 1_000 : 60_000);
    return () => window.clearTimeout(timer);
  }, [nowMs, sourceCooldownDeadline]);
  if (!runtime?.sources.length) {
    return <EmptyState title={t("sources.emptyTitle")} description={t("sources.emptyDescription")} />;
  }
  const sources = runtime.sources.filter((source) => matchesQuery(
    query,
    source.name,
    source.baseUrl,
    effectiveSourceProtocolBindings(source).map((binding) => binding.wireApi),
    source.models,
  ));
  if (!sources.length) return <NoResults />;
  const localSource = mode !== "remote";
  const runtimeBySource = new Map((runtime.gateway.routingOrder ?? []).map((candidate) => [candidate.candidateId, candidate]));
  const updateParticipation = (source: SourceSummary, inPool: boolean) => perform(
    `source-pool-${source.id}`,
    () => localSource
      ? relayCommands.setPoolMembership([], [source.id], inPool)
      : relayCommands.remoteAction({ type: "set_pool_membership" }, { accountIds: [], sourceIds: [source.id], inPool }),
    "feedback.saved",
  );
  const refreshModels = (source: SourceSummary) => perform(
    `source-models-${source.id}`,
    () => localSource
      ? relayCommands.testSource(source.id)
      : relayCommands.remoteAction({ type: "test_source", id: source.id }),
    "feedback.refreshed",
  );
  return (
    <div className="relay-table-wrap relay-compact-content">
      <table className="relay-table source-table">
        <thead><tr><th>{t("common.status")}</th><th>{t("common.name")}</th><th>{t("sources.host")}</th><th>{t("sources.route")}</th><th>{t("common.models")}</th><th><span className="sr-only">{t("common.actions")}</span></th></tr></thead>
        <tbody>{sources.map((source) => {
          const launchBusy = busy === `launch-source-${source.id}`;
          const supportsResponses = sourceSupportsWireApi(source, "responses");
          const supportsNativeResponses = sourceSupportsNativeResponses(source);
          const launchDisabled = !localSource || !supportsNativeResponses || !source.enabled || !source.secretAvailable || launchBusy;
          const launchTitle = !localSource
            ? t("sources.launchLocalOnly")
            : !supportsNativeResponses
              ? t("sources.launchResponsesOnly")
              : !source.enabled || !source.secretAvailable
                ? t("sources.launchUnavailable")
                : t("sources.launch");
          const runtimeState = source.inPool ? runtimeBySource.get(source.id) : undefined;
          const runtimeTone = source.operationalStatus === "rotation" ? transientCandidateTone(runtimeState, nowMs, true) : null;
          const runtimeHint = runtimeState?.halfOpen
            ? t("pool.recoveryProbe")
            : runtimeState?.nextRetryAtMs != null && runtimeState.nextRetryAtMs > nowMs
              ? t("pool.retryAt", { time: new Date(runtimeState.nextRetryAtMs).toLocaleString(i18n.language) })
              : null;
          const statusLabel = t(`connections.status.${source.operationalStatus}`);
          const indicatorLabel = runtimeHint ? `${statusLabel} · ${runtimeHint}` : statusLabel;
          const indicatorTone = source.operationalStatus === "unavailable" || source.operationalStatus === "disabled"
            ? operationalStatusTone(source.operationalStatus)
            : runtimeTone ?? operationalStatusTone(source.operationalStatus);
          return <tr key={source.id}>
            <td><StatusIcon status={indicatorTone} label={indicatorLabel} /></td>
            <td><strong>{source.name}</strong></td>
            <td><code>{safeHost(source.baseUrl)}</code></td>
            <td><SourceProtocolBindingsSummary source={source} /></td>
            <td>{source.models.length}</td>
            <td className="row-actions-cell"><div className="row-actions">
              <ActionMenu>
                <ActionMenuItem icon={busy === `source-models-${source.id}` ? <Loader2 className="spin" aria-hidden /> : <RefreshCw aria-hidden />} disabled={busy === `source-models-${source.id}`} onClick={() => void refreshModels(source)}>{t("sources.refreshModels")}</ActionMenuItem>
                {mode !== "zenith" ? <ActionMenuItem icon={source.inPool ? <ListMinus aria-hidden /> : <ListPlus aria-hidden />} disabled={busy === `source-pool-${source.id}` || (!source.inPool && !supportsResponses)} title={!source.inPool && !supportsResponses ? t("sources.poolResponsesOnly") : undefined} onClick={() => void updateParticipation(source, !source.inPool)}>{t(source.inPool ? "sources.removeFromPoolAction" : "sources.addToPoolAction")}</ActionMenuItem> : null}
                <ActionMenuItem icon={<Power aria-hidden />} onClick={() => perform(`toggle-${source.id}`, () => localSource ? relayCommands.setSourceEnabled(source.id, !source.enabled) : relayCommands.remoteAction({ type: "update_source", id: source.id }, { enabled: !source.enabled }), "feedback.saved")}>{source.enabled ? t("common.disable") : t("common.enable")}</ActionMenuItem>
                <ActionMenuItem danger icon={<Trash2 aria-hidden />} onClick={() => void confirm(t("sources.deleteConfirm"), { danger: true }).then((accepted) => accepted && perform(`delete-${source.id}`, () => localSource ? relayCommands.deleteSource(source.id) : relayCommands.remoteAction({ type: "delete_source", id: source.id }), "feedback.deleted"))}>{t("common.delete")}</ActionMenuItem>
              </ActionMenu>
              <IconButton label={t("common.edit")} icon={<Pencil aria-hidden />} onClick={() => onEdit(source)} />
              <IconButton label={t("sources.launch")} icon={launchBusy ? <Loader2 className="spin" aria-hidden /> : <Play aria-hidden />} disabled={launchDisabled} title={launchTitle} onClick={() => {
                void activateCodexProfile(`launch-source-${source.id}`, () => relayCommands.launchCodexSource(source.id), true)
                  .then((activated) => { if (activated) localStorage.setItem("relay.directSourceId", source.id); });
              }} />
            </div></td>
          </tr>;
        })}</tbody>
      </table>
    </div>
  );
}

function AccountExportDialog({ accountIds, onClose }: { accountIds: string[]; onClose: () => void }) {
  const { t } = useTranslation();
  const { mode, perform, busy } = useRelayState();
  const [format, setFormat] = useState<AccountExportFormat>("zenith");
  const [description, setDescription] = useState("");
  const [descriptionMode, setDescriptionMode] = useState<"edit" | "preview">("edit");
  const [descriptionError, setDescriptionError] = useState<string | null>(null);
  const markdownFileInput = useRef<HTMLInputElement>(null);
  const formats = accountExportFormats.filter((option) => accountIds.length === 1 || option.multiple);
  const selectedFormat = formats.find((option) => option.value === format) ?? formats[0];
  const loadMarkdown = async (event: ChangeEvent<HTMLInputElement>) => {
    const input = event.currentTarget;
    const file = input.files?.[0];
    input.value = "";
    if (!file) return;
    try {
      const content = (await file.text()).replace(/\r\n?/g, "\n");
      if (content.length > MAX_ZENITH_EXPORT_DESCRIPTION_LENGTH) {
        setDescriptionError(t("accounts.exportDescriptionTooLong", { max: MAX_ZENITH_EXPORT_DESCRIPTION_LENGTH }));
        return;
      }
      setDescription(content);
      setDescriptionError(null);
      setDescriptionMode("preview");
    } catch {
      setDescriptionError(t("accounts.exportDescriptionReadFailed"));
    }
  };
  const run = async (destination: "copy" | "download") => {
    const ok = await perform(`account-export-${destination}`, async () => {
      const input = {
        accountIds,
        format,
        destination,
        ...(format === "zenith" && description.trim() ? { description } : {}),
      } as const;
      const result = mode === "local"
        ? await relayCommands.exportLocalAccounts(input)
        : await relayCommands.exportRemoteAccounts(input);
      if (destination === "copy") {
        if (!result.content) throw new Error("account export content is missing");
        await copyText(result.content);
      }
    }, destination === "copy" ? "feedback.accountExportCopied" : "feedback.accountExportDownloaded");
    if (ok) onClose();
  };
  return <Dialog title={t("accounts.exportTitle")} onClose={onClose} footer={<><Button variant="secondary" onClick={onClose}>{t("common.cancel")}</Button><Button variant="secondary" icon={<Copy aria-hidden />} busy={busy === "account-export-copy"} onClick={() => run("copy")}>{t("accounts.copyExport")}</Button><Button variant="primary" icon={<Download aria-hidden />} busy={busy === "account-export-download"} onClick={() => run("download")}>{t("accounts.downloadExport")}</Button></>}>
    <div className="relay-form account-export-form">
      <div className="account-export-heading"><span>{t("accounts.exportFormat")}</span><strong>{t("accounts.exportCount", { count: accountIds.length })}</strong></div>
      <div className="account-export-formats" data-count={formats.length} role="radiogroup" aria-label={t("accounts.exportFormat")}>{formats.map((option) => <button type="button" role="radio" data-value={option.value} aria-checked={format === option.value} key={option.value} onClick={() => setFormat(option.value)}><span>{option.label}</span>{format === option.value ? <Check aria-hidden /> : null}</button>)}</div>
      <p className="account-export-description">{t(`accounts.exportFormats.${selectedFormat.value}`)}</p>
      {format === "zenith" ? <div className="relay-field account-export-description-field">
        <div className="account-export-description-toolbar">
          <label htmlFor="zenith-export-description">{t("accounts.exportDescription")}</label>
          <div className="account-export-description-controls">
            <input ref={markdownFileInput} type="file" accept=".md,text/markdown" onChange={(event) => void loadMarkdown(event)} />
            <IconButton label={t("accounts.loadMarkdown")} icon={<Upload aria-hidden />} onClick={() => markdownFileInput.current?.click()} />
            <div className="markdown-mode-switch" role="group" aria-label={t("accounts.descriptionMode")}>
              <button type="button" aria-pressed={descriptionMode === "edit"} onClick={() => setDescriptionMode("edit")}><Pencil aria-hidden />{t("accounts.descriptionEdit")}</button>
              <button type="button" aria-pressed={descriptionMode === "preview"} onClick={() => setDescriptionMode("preview")}><Eye aria-hidden />{t("accounts.descriptionPreview")}</button>
            </div>
          </div>
        </div>
        {descriptionMode === "edit"
          ? <textarea id="zenith-export-description" value={description} maxLength={MAX_ZENITH_EXPORT_DESCRIPTION_LENGTH} placeholder={t("accounts.exportDescriptionPlaceholder")} onChange={(event) => { setDescription(event.target.value); setDescriptionError(null); }} />
          : <div className="account-export-markdown-preview" role="region" aria-label={t("accounts.descriptionPreview")}>{description.trim() ? <MarkdownPreview content={description} /> : <p>{t("accounts.descriptionPreviewEmpty")}</p>}</div>}
        {descriptionError ? <p className="form-note error-text" role="alert">{descriptionError}</p> : null}
        <small>{t("accounts.exportDescriptionHint", { count: description.length, max: MAX_ZENITH_EXPORT_DESCRIPTION_LENGTH })}</small>
      </div> : null}
    </div>
  </Dialog>;
}

function AutomationsTable({ query, onEdit }: { query: string; onEdit: (task: WakeTask) => void }) {
  const { t } = useTranslation();
  const { mode, runtime, perform, busy } = useRelayState();
  const confirm = useConfirm();
  if (!runtime?.automations.length) {
    return <EmptyState title={t("automations.emptyTitle")} description={t("automations.emptyDescription")} />;
  }
  const automations = runtime.automations.filter((task) => matchesQuery(query, task.name, task.accountSelector.kind === "all_eligible" ? "" : task.accountSelector.values, task.modelPolicy.kind === "explicit" ? task.modelPolicy.value : ""));
  if (!automations.length) return <NoResults />;
  return (
    <div className="relay-table-wrap">
        <table className="relay-table">
          <thead><tr><th>{t("common.status")}</th><th>{t("common.name")}</th><th>{t("connections.accounts")}</th><th>{t("common.model")}</th><th>{t("automations.lastResult")}</th><th><span className="sr-only">{t("common.actions")}</span></th></tr></thead>
          <tbody>{automations.map((task) => {
            const history = runtime.wakeHistory.filter((item) => item.taskId === task.id);
            const last = history[history.length - 1];
            return (
              <tr key={task.id}>
                <td><input type="checkbox" checked={task.enabled} aria-label={t("common.enabled")} onChange={() => perform(`automation-${task.id}`, () => mode === "local" ? relayCommands.setAutomationEnabled(task.id, !task.enabled) : relayCommands.remoteAction({ type: "update_wake_task", id: task.id }, { ...task, enabled: !task.enabled }), "feedback.saved")} /></td>
                <td><strong>{task.name}</strong></td>
                <td>{task.accountSelector.kind === "all_eligible" ? t("automations.allEligible") : task.accountSelector.kind === "account_ids" ? task.accountSelector.values.map((id) => runtime.accounts.find((account) => account.id === id)?.label ?? id).join(", ") : task.accountSelector.values.join(", ")}</td>
                <td>{task.modelPolicy.kind === "explicit" ? task.modelPolicy.value : t("automations.lightest")}</td>
                <td>{last ? t(`wake.${last.outcome}`, { defaultValue: last.outcome }) : t("common.never")}</td>
                <td className="row-actions-cell"><div className="row-actions"><IconButton label={t("common.edit")} icon={<Pencil aria-hidden />} onClick={() => onEdit(task)} /><IconButton label={t("common.test")} icon={<Play aria-hidden />} disabled={busy === `test-${task.id}`} onClick={() => perform(`test-${task.id}`, () => mode === "local" ? relayCommands.testAutomation(task.id) : relayCommands.remoteAction({ type: "test_wake_task", id: task.id }), "feedback.checked")} /><ActionMenu><ActionMenuItem danger icon={<Trash2 aria-hidden />} onClick={() => void confirm(t("automations.deleteConfirm"), { danger: true }).then((accepted) => accepted && perform(`delete-${task.id}`, () => mode === "local" ? relayCommands.deleteAutomation(task.id) : relayCommands.remoteAction({ type: "delete_wake_task", id: task.id }), "feedback.deleted"))}>{t("common.delete")}</ActionMenuItem></ActionMenu></div></td>
              </tr>
            );
          })}</tbody>
        </table>
    </div>
  );
}

function RemoteView({ onConnect, onDeploy }: { onConnect: () => void; onDeploy: () => void }) {
  const { t } = useTranslation();
  const { runtime, perform, busy } = useRelayState();
  const confirm = useConfirm();
  if (!runtime) return <EmptyState title={t("remote.emptyTitle")} description={t("remote.emptyDescription")} action={<div className="inline-actions"><Button variant="primary" onClick={onConnect}>{t("remote.connectExisting")}</Button><Button variant="secondary" onClick={onDeploy}>{t("remote.deployNew")}</Button></div>} />;
  const disconnect = async () => {
    let linkedAccounts = 0;
    const counted = await perform("remote-disconnect-check", async () => { linkedAccounts = await relayCommands.remoteLinkedAccountCount(); });
    if (!counted) return;
    const message = linkedAccounts
      ? t("remote.disconnectLinkedConfirm", { count: linkedAccounts })
      : t("remote.disconnectConfirm");
    if (await confirm(message, { danger: true })) {
      await perform("remote-disconnect", relayCommands.disconnectRemote, "feedback.disconnected");
    }
  };
  return <section className="remote-summary"><div className="remote-status"><StatusBadge status={runtime.runtimeTarget.connected ? "ready" : "error"} label={runtime.runtimeTarget.connected ? t("common.connected") : t("common.offline")} /><div><strong>{runtime.runtimeTarget.origin}</strong><small>{runtime.runtimeTarget.serverId}</small></div></div><dl className="detail-list"><div><dt>{t("remote.version")}</dt><dd>{runtime.runtimeTarget.version}</dd></div><div><dt>{t("gateway.endpoint")}</dt><dd><code>{runtime.gateway.baseUrl}</code></dd></div><div><dt>{t("remote.capabilities")}</dt><dd>{runtime.capabilities.features.length}</dd></div></dl><div className="inline-actions"><Button variant="danger" busy={busy === "remote-disconnect-check" || busy === "remote-disconnect"} onClick={() => void disconnect()}>{t("remote.disconnect")}</Button></div></section>;
}

function OAuthDialog({ flow, onCancel }: { flow: OAuthFlow; onCancel: () => Promise<void> }) {
  const { t } = useTranslation();
  const { busy, perform } = useRelayState();
  const [now, setNow] = useState(Date.now);
  const [reopenAt, setReopenAt] = useState(0);
  const [linkCopied, setLinkCopied] = useState(false);
  useEffect(() => {
    const interval = window.setInterval(() => setNow(Date.now()), 250);
    return () => window.clearInterval(interval);
  }, []);
  const secondsRemaining = Math.max(0, Math.ceil((flow.expiresAtMs - now) / 1_000));
  const reopenIn = Math.max(0, Math.ceil((reopenAt - now) / 1_000));
  const callbackReceived = flow.status === "callback_received" || busy === "oauth-complete";
  const flowFailed = flow.status === "callback_rejected" || flow.status === "expired" || flow.status === "failed";
  const flowUnavailable = secondsRemaining === 0 || flow.status !== "pending";
  const reopen = async () => {
    const opened = await perform("oauth-reopen", () => relayCommands.resumeOAuth(flow.loginId));
    if (opened) setReopenAt(Date.now() + 3_000);
  };
  const copyLink = async () => {
    await copyText(flow.authorizationUrl);
    setLinkCopied(true);
    window.setTimeout(() => setLinkCopied(false), 1_500);
  };
  return <Dialog
    title={t("accounts.signIn")}
    onClose={() => void onCancel()}
    footer={<Button variant="secondary" busy={busy === "oauth-cancel"} onClick={() => void onCancel()}>{t("common.cancel")}</Button>}
  >
    <div className="relay-form oauth-waiting">
      <div className="oauth-waiting-status"><Loader2 className="spin" aria-hidden /><div><strong>{t(callbackReceived ? "accounts.completingSignIn" : "accounts.waitingForSignIn")}</strong><p>{t("accounts.waitingForSignInHint")}</p></div></div>
      {flowFailed ? <p role="alert" className="form-note error-text">{t(`accounts.oauthStatus.${flow.status}`)}</p> : null}
      <div className="oauth-expiry" role="timer"><Clock3 aria-hidden /><span>{t("accounts.oauthRemaining")}</span><strong>{formatCountdown(secondsRemaining)}</strong></div>
      <div className="oauth-link-actions">
        <Button variant="primary" icon={<ExternalLink aria-hidden />} busy={busy === "oauth-reopen"} disabled={flowUnavailable || reopenIn > 0} onClick={() => void reopen()}>{reopenIn > 0 ? t("accounts.reopenSignInCooldown", { count: reopenIn }) : t(reopenAt ? "accounts.reopenSignIn" : "accounts.openSignIn")}</Button>
        <Button variant="secondary" icon={linkCopied ? <Check aria-hidden /> : <Copy aria-hidden />} disabled={flowUnavailable} onClick={() => void copyLink()}>{t(linkCopied ? "accounts.signInLinkCopied" : "accounts.copySignInLink")}</Button>
      </div>
    </div>
  </Dialog>;
}

function OAuthAccountSetupDialog({ accountId, onClose }: { accountId: string; onClose: () => void }) {
  const { t } = useTranslation();
  const { runtime, busy, perform } = useRelayState();
  const { pool } = useProxyPool();
  const [addToPool, setAddToPool] = useState(true);
  const [assignProxy, setAssignProxy] = useState(false);
  const account = runtime?.accounts.find((item) => item.id === accountId);
  const apply = async () => {
    if (!addToPool && !assignProxy) {
      onClose();
      return;
    }
    const ok = await perform("oauth-setup", async () => {
      if (addToPool) await relayCommands.setPoolMembership([accountId], [], true);
      if (assignProxy) await relayCommands.assignAutomaticProxies([accountId]);
    }, "feedback.saved");
    if (ok) onClose();
  };
  return <Dialog title={t("accounts.accountAdded")} onClose={onClose} footer={<><Button variant="secondary" onClick={onClose}>{t("accounts.configureLater")}</Button><Button variant="primary" busy={busy === "oauth-setup"} onClick={() => void apply()}>{t("common.done")}</Button></>}><div className="relay-form oauth-account-setup"><div className="oauth-account-added"><Check aria-hidden /><div><strong>{account?.identityHint ?? t("accounts.accountReady")}</strong><p>{t("accounts.accountAddedHint")}</p></div></div><div className="post-import-options"><label><input type="checkbox" checked={addToPool} onChange={(event) => setAddToPool(event.target.checked)} /><span><strong>{t("accounts.addAccountToPool")}</strong><small>{t("accounts.addToPoolHint")}</small></span></label><label><input type="checkbox" checked={assignProxy} disabled={!pool || pool.total === 0} onChange={(event) => setAssignProxy(event.target.checked)} /><span><strong>{t("proxies.assignStoredAfterAdd")}</strong><small>{pool ? t(pool.total ? "proxies.storedAvailable" : "proxies.noStored", { count: pool.total }) : t("common.loading")}</small></span></label></div></div></Dialog>;
}

function formatCountdown(seconds: number) {
  const hours = Math.floor(seconds / 3_600);
  const minutes = Math.floor((seconds % 3_600) / 60);
  const remainder = seconds % 60;
  return hours > 0
    ? `${hours}:${String(minutes).padStart(2, "0")}:${String(remainder).padStart(2, "0")}`
    : `${minutes}:${String(remainder).padStart(2, "0")}`;
}

function AutomationDialog({ task, onClose }: { task: WakeTask | null; onClose: () => void }) {
  const { t } = useTranslation();
  const { mode, runtime, perform, busy } = useRelayState();
  const [name, setName] = useState(task?.name ?? t("automations.defaultName"));
  const [executionPolicy, setExecutionPolicy] = useState<WakeTask["executionPolicy"]>(mode === "local" ? task?.executionPolicy ?? "automatic" : "automatic");
  const [selectorKind, setSelectorKind] = useState<WakeTask["accountSelector"]["kind"]>(task?.accountSelector.kind ?? "all_eligible");
  const [accountIds, setAccountIds] = useState<string[]>(task?.accountSelector.kind === "account_ids" ? task.accountSelector.values : []);
  const [modelId, setModelId] = useState(task?.modelPolicy.kind === "explicit" ? task.modelPolicy.value : "");
  const accounts = runtime?.accounts ?? [];
  const poolAccounts = accounts.filter((account) => account.inPool && account.enabled && !account.draining);
  const selectedAccounts = poolAccounts.filter((account) => accountIds.includes(account.id));
  const targetAccounts = selectorKind === "account_ids" ? selectedAccounts : selectorKind === "all_eligible" ? poolAccounts : [];
  const rawPoolModels = runtime?.gateway.visibleModelIds.length
    ? runtime.gateway.visibleModelIds
    : (runtime?.gateway.models ?? []).filter((model) => model.enabled).map((model) => model.id);
  const poolModels = sortModelIdsForLauncher(rawPoolModels.filter((model, index) => rawPoolModels.findIndex((candidate) => candidate.toLowerCase() === model.toLowerCase()) === index));
  const modelSets = targetAccounts.map((account) => account.models.filter((model) => (account.allowedModels.length === 0 || account.allowedModels.some((allowed) => allowed.toLowerCase() === model.toLowerCase())) && !account.excludedModels.some((excluded) => excluded.toLowerCase() === model.toLowerCase())));
  const targetModels = selectorKind === "account_ids" && modelSets.length > 1
    ? modelSets[0].filter((model) => modelSets.slice(1).every((set) => set.some((candidate) => candidate.toLowerCase() === model.toLowerCase())))
    : modelSets.flat();
  const availableModels = poolModels.filter((model) => targetModels.some((candidate) => candidate.toLowerCase() === model.toLowerCase()));
  const toggleAccount = (id: string) => setAccountIds((current) => current.includes(id) ? current.filter((item) => item !== id) : [...current, id]);
  const accountSelectionValid = selectorKind === "all_eligible" ? poolAccounts.length > 0 : selectorKind === "account_ids" && accountIds.length > 0 && selectedAccounts.length === accountIds.length;
  const selectedModel = availableModels.find((model) => model.toLowerCase() === modelId.trim().toLowerCase()) ?? availableModels[0] ?? "";
  const valid = Boolean(name.trim() && accountSelectionValid && selectedModel);
  const selectorOptions = [
    { value: "all_eligible", label: t("automations.allEligible") },
    { value: "account_ids", label: t("automations.selectedAccounts") },
    ...(selectorKind === "tags" ? [{ value: "tags", label: t("automations.matchingTags") }] : []),
  ];
  const modelOptions = availableModels.length ? availableModels.map((model) => ({ value: model, label: model })) : [{ value: "", label: t("automations.noPoolModels") }];
  const save = async () => {
    if (!valid) return;
    const now = Date.now();
    const accountSelector = selectorKind === "account_ids" ? { kind: selectorKind, values: accountIds } : { kind: "all_eligible" as const };
    const modelPolicy = { kind: "explicit" as const, value: selectedModel };
    const base = { ...defaultWakeInput(name), enabled: task?.enabled ?? true, accountSelector, modelPolicy, executionPolicy, jitterSeconds: task?.jitterSeconds ?? 0, maxAttemptsPerCycle: task?.maxAttemptsPerCycle ?? 1 };
    const remoteInput = task ? { ...task, ...base, updatedAtMs: now } : { ...base, id: "", trigger: { kind: "quota_full" }, fallbackSchedule: null, createdAtMs: now, updatedAtMs: now };
    const id = task ? `automation-update-${task.id}` : "automation-create";
    const ok = await perform(id, () => mode === "local" ? (task ? relayCommands.updateAutomation(task.id, base) : relayCommands.createAutomation(base)) : relayCommands.remoteAction({ type: task ? "update_wake_task" : "create_wake_task", ...(task ? { id: task.id } : {}) }, remoteInput), task ? "feedback.saved" : "feedback.automationAdded");
    if (ok) onClose();
  };
  return <Dialog wide title={task ? t("automations.edit") : t("automations.add")} onClose={onClose} footer={<><Button variant="secondary" onClick={onClose}>{t("common.cancel")}</Button><Button variant="primary" busy={busy === (task ? `automation-update-${task.id}` : "automation-create")} disabled={!valid} onClick={save}>{t("common.save")}</Button></>}>
    <div className="relay-form automation-form">
      <label className="relay-field"><span>{t("common.name")}</span><input value={name} onChange={(event) => setName(event.target.value)} autoFocus /></label>
      <div className="automation-target-grid">
        <div className="relay-field"><span>{t("automations.accountSelection")}</span><OptionMenu className="field-option-menu" label={t("automations.accountSelection")} value={selectorKind} onChange={(value) => setSelectorKind(value as WakeTask["accountSelector"]["kind"])} options={selectorOptions} /></div>
        <div className="relay-field"><span>{t("common.model")}</span><OptionMenu className="field-option-menu" label={t("common.model")} value={selectedModel} onChange={setModelId} options={modelOptions} disabled={!availableModels.length} /></div>
      </div>
      {selectorKind === "account_ids" ? <fieldset className="automation-account-picker"><legend>{t("automations.selectedAccounts")}</legend><div className="scope-grid">{poolAccounts.map((account) => <label key={account.id}><input type="checkbox" checked={accountIds.includes(account.id)} onChange={() => toggleAccount(account.id)} /><span>{account.label}</span></label>)}</div></fieldset> : null}
      {selectorKind === "tags" ? <><label className="relay-field"><span>{t("automations.tags")}</span><input value={task?.accountSelector.kind === "tags" ? task.accountSelector.values.join(", ") : ""} readOnly /></label><p role="alert" className="automation-validation">{t("automations.legacyTags")}</p></> : null}
      {!accountSelectionValid ? <p role="alert" className="automation-validation">{t("automations.accountsRequired")}</p> : null}
      {accountSelectionValid && !selectedModel ? <p role="alert" className="automation-validation">{t("automations.modelUnavailable")}</p> : null}
      <div className="automation-rule">
        <span>{t("automations.condition")}</span>
        {mode === "local" ? <div className="segmented automation-execution" role="group" aria-label={t("automations.execution")}>
          <button type="button" className={executionPolicy === "automatic" ? "active" : ""} aria-pressed={executionPolicy === "automatic"} onClick={() => setExecutionPolicy("automatic")}>{t("automations.automatic")}</button>
          <button type="button" className={executionPolicy === "require_confirmation" ? "active" : ""} aria-pressed={executionPolicy === "require_confirmation"} onClick={() => setExecutionPolicy("require_confirmation")}>{t("automations.manual")}</button>
        </div> : <strong>{t("automations.automatic")}</strong>}
      </div>
      {mode !== "local" && task?.executionPolicy === "require_confirmation" ? <p role="status" className="automation-validation">{t("automations.remoteConfirmationMigration")}</p> : null}
    </div>
  </Dialog>;
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
  return <Dialog title={t("remote.connectExisting")} onClose={onClose} footer={<><Button variant="secondary" onClick={onClose}>{t("common.cancel")}</Button><Button variant="primary" busy={busy === "remote-connect"} disabled={!baseUrl || !token || (insecure && !allowInsecure)} onClick={connect}>{t("remote.testAndConnect")}</Button></>}><div className="relay-form"><label className="relay-field"><span>{t("remote.address")}</span><input type="url" value={baseUrl} onChange={(event) => setBaseUrl(event.target.value)} placeholder="https://relay.example.com" /></label><SecretField label={t("remote.token")} value={token} onChange={setToken} /><div className="remote-connect-options">{insecure ? <SettingToggle tone="warning" label={t("remote.allowInsecure")} description={t("remote.allowInsecureHint")} checked={allowInsecure} onChange={setAllowInsecure} /> : null}<SettingToggle label={t("remote.confirmIdentityChange")} description={t("remote.identityHint")} checked={confirmIdentityChange} onChange={setConfirmIdentityChange} /></div></div></Dialog>;
}

function DeployDialog({ onClose }: { onClose: () => void }) {
  const { t } = useTranslation();
  const { perform, busy } = useRelayState();
  const [url, setUrl] = useState("");
  const [plan, setPlan] = useState<{ directory: string; managementToken: string; vaultKey: string; composeCommand: string } | null>(null);
  const generate = async () => { const result: { current: typeof plan } = { current: null }; const ok = await perform("remote-deploy", async () => { result.current = await relayCommands.prepareRemoteDeployment(url); }, "feedback.deploymentPrepared"); if (ok) setPlan(result.current); };
  return <Dialog title={t("remote.deployNew")} onClose={onClose} footer={<><Button variant="secondary" onClick={onClose}>{t("common.close")}</Button>{!plan ? <Button variant="primary" busy={busy === "remote-deploy"} disabled={!url} onClick={generate}>{t("remote.generate")}</Button> : null}</>}>{plan ? <div className="deployment-result"><StatusBadge status="ready" label={t("common.ready")} /><label><span>{t("remote.bundlePath")}</span><code>{plan.directory}</code></label><div className="relay-field"><span>{t("remote.token")}</span><div className="endpoint-line"><input aria-label={t("remote.token")} type="password" value={plan.managementToken} readOnly /><Button variant="secondary" icon={<Copy aria-hidden />} onClick={() => copyText(plan.managementToken)}>{t("common.copy")}</Button></div></div><div className="relay-field"><span>{t("remote.vaultKey")}</span><div className="endpoint-line"><input aria-label={t("remote.vaultKey")} type="password" value={plan.vaultKey} readOnly /><Button variant="secondary" icon={<Copy aria-hidden />} onClick={() => copyText(plan.vaultKey)}>{t("common.copy")}</Button></div></div><label><span>{t("remote.command")}</span><code>{plan.composeCommand}</code></label><p>{t("remote.secretOnce")}</p><p>{t("remote.deployHint")}</p></div> : <label className="relay-field"><span>{t("remote.publicUrl")}</span><input type="url" value={url} onChange={(event) => setUrl(event.target.value)} placeholder="https://relay.example.com" /></label>}</Dialog>;
}

function safeHost(value: string) {
  try { return new URL(value).host; } catch { return value; }
}
