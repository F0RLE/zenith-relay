import { Channel, invoke as tauriInvoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import type {
  TrashedAccountSummary,
  AccountExportInput,
  AccountExportResult,
  AccountLoginDetails,
  AccountLoginUpdate,
  AccountTotpPreview,
  AccountImportProgress,
  CandidateRuntimeSnapshot,
  ConfirmAccountImportResponse,
  ConfigurationPresetApplyResult,
  ConfigurationPresetPreview,
  ConsumeResetCreditResponse,
  DefaultServiceTier,
  CodexMemoryStage,
  ModelContextPolicy,
  DiagnosticSettings,
  ImportSession,
  CacheSessionRecord,
  LocalUsagePage,
  ClaudeDesktopStatus,
  ClaudeDesktopConnection,
  LaunchApplication,
  LaunchPathSetting,
  OpenCodeConfigStatus,
  ProfileSnapshot,
  ProfileSnapshotList,
  OAuthFlow,
  OAuthClientKind,
  OAuthFlowEvent,
  OAuthCompletion,
  ProfileActivation,
  ProxyPoolImportResult,
  ProxyCheckResult,
  ProxySpeedTestResult,
  ProxyPoolSummary,
  RelayStorageInfo,
  RevealedAccountIdentity,
  UsageQuery,
  PoolRoutingPolicy,
  PoolRoutingSnapshot,
  RuntimeActivitySnapshot,
  RuntimeSnapshot,
  SourceStats,
  StoredProxyAssignmentResult,
  UsageExportRow,
  WakeTask,
} from "./types";
import { sanitizeFeedbackError } from "../state/feedback";
import { createUsagePageReader } from "./usagePage";

export type LocalNetworkAccess = {
  enabled: boolean;
  addresses: Array<{ interface: string; baseUrl: string }>;
};

/**
 * Keep the command surface typed while making every rejected IPC call
 * observable in the native error log.  The reporter itself uses the raw
 * invoke function to avoid an error-reporting loop.
 */
function invoke<T>(command: string, commandArgs?: Record<string, unknown>): Promise<T> {
  return tauriInvoke<T>(command, commandArgs).catch((cause) => {
    if (command !== "record_frontend_diagnostic") {
      const error = sanitizeFeedbackError(cause, "ipc_failed", "Relay command failed");
      void tauriInvoke<void>("record_frontend_diagnostic", {
        input: {
          source: "tauri-command",
          operation: command,
          code: error.code,
          message: error.message,
        },
      }).catch(() => undefined);
    }
    throw cause;
  });
}

const readLocalUsagePage = createUsagePageReader((query) => {
  const { includeReport, ...input } = query;
  return invoke<LocalUsagePage>("get_local_usage_page", {
    input,
    ...(includeReport !== undefined ? { includeReport } : {}),
  });
});

export const relayCommands = {
  localNetworkAccess: () => invoke<LocalNetworkAccess>("get_local_network_access"),
  setLocalNetworkAccess: (enabled: boolean, expectedEnabled: boolean) => invoke<LocalNetworkAccess>("update_local_network_access", { enabled, expectedEnabled }),
  launchApplication: (application: LaunchApplication) => invoke<void>("launch_application", { application }),
  openHostedPreview: () => invoke<void>("open_hosted_preview"),
  getApplicationLaunchPath: (application: LaunchApplication) => invoke<LaunchPathSetting>("get_application_launch_path", { application }),
  setApplicationLaunchPath: (application: LaunchApplication, path: string | null) => invoke<LaunchPathSetting>("set_application_launch_path", { application, path }),
  pickApplicationLaunchPath: (application: LaunchApplication, path: string | null) => invoke<string | null>("pick_application_launch_path", { application, path }),
  onStateChanged: (callback: () => void) => listen("zenith-state-changed", callback),
  onProxyPoolChanged: (callback: () => void) => listen("zenith-proxy-pool-changed", callback),
  onRuntimeActivity: (callback: (activity: RuntimeActivitySnapshot) => void) => listen<RuntimeActivitySnapshot>("zenith-runtime-activity", (event) => callback(event.payload)),
  onUsageRecorded: (callback: () => void) => listen("zenith-usage-recorded", callback),
  localState: () => invoke<RuntimeSnapshot>("get_local_runtime_state"),
  localRuntimeOrder: () => invoke<CandidateRuntimeSnapshot[]>("get_local_runtime_order"),
  localUsagePage: readLocalUsagePage,
  localCacheSessions: (usageQuery: UsageQuery = {}, scope?: Pick<CacheSessionRecord, "clientContextId" | "candidateKind" | "candidateId" | "model">) => invoke<CacheSessionRecord[]>("get_local_cache_sessions", {
    input: usageQuery, ...(scope ? { scope } : {}),
  }),
  clearLocalUsage: () => invoke("clear_local_usage"),

  createSource: (sourceInput: Record<string, unknown>) => invoke("create_local_source", { input: sourceInput }),
  updateSource: (sourceInput: Record<string, unknown>) => invoke("update_local_source", { input: sourceInput }),
  rotateSourceKey: (sourceId: string, apiKey: string) => invoke("rotate_local_source_key", { sourceId, apiKey }),
  setSourceEnabled: (sourceId: string, enabled: boolean) => invoke("set_local_source_enabled", { sourceId, enabled }),
  deleteSource: (sourceId: string) => invoke("delete_local_source", { sourceId }),
  testSource: (sourceId: string) => invoke("test_local_source", { sourceId }),
  refreshSourceData: (sourceId: string) => invoke("refresh_local_source_data", { sourceId }),
  localSourceStats: (sourceId: string, force = false) => invoke<SourceStats>("get_local_source_stats", { sourceId, force }),

  startImport: (content: string) => invoke<ImportSession>("start_local_account_import", { input: { content } }),
  previewImportFiles: (paths?: string[]) => invoke<ImportSession | null>("preview_local_account_import_files", paths ? { paths } : {}),
  prepareImport: (sessionId: string, probeQuota = false) => invoke<ImportSession>("prepare_local_account_import", { input: { sessionId, probeQuota } }),
  confirmImport: (sessionId: string, selectedItemIds: string[], addToPool = false) => invoke<ConfirmAccountImportResponse>("confirm_local_account_import", { input: { sessionId, selectedItemIds, discoverModels: false, probeQuota: false, models: [], addToPool } }),
  onImportProgress: (callback: (event: AccountImportProgress) => void) => listen<AccountImportProgress>("relay-account-import-progress", (event) => callback(event.payload)),
  cancelImport: (sessionId: string) => invoke("cancel_local_account_import", { sessionId }),
  refreshAccountQuota: (accountId: string) => invoke("refresh_local_account_quota", { accountId }),
  refreshAllAccountQuotas: () => invoke<Array<{ accountId: string; status: "succeeded" | "failed" }>>("refresh_all_local_account_quotas"),
  consumeResetCredit: (accountId: string) => invoke<ConsumeResetCreditResponse>("consume_local_reset_credit", { accountId }),
  updateAccount: (accountUpdate: Record<string, unknown>) => invoke("update_local_account", { input: accountUpdate }),
  setAccountEnabled: (accountId: string, enabled: boolean) => invoke("set_local_account_enabled", { accountId, enabled }),
  accountRouteCandidates: (accountId: string) => invoke<string[]>("local_account_route_candidates", { accountId }),
  selectAccountRoute: (accountId: string, connectionId: string, attach = false) => invoke<void>("select_local_account_route", { accountId, connectionId, attach }),
  detachAccountRoute: (accountId: string, connectionId: string) => invoke<void>("detach_local_account_route", { accountId, connectionId }),
  deleteAccount: (accountId: string) => invoke("delete_local_account", { accountId }),
  deleteAccounts: (accountIds: string[]) => invoke("delete_local_accounts", { accountIds }),
  accountTrash: () => invoke<TrashedAccountSummary[]>("list_local_account_trash"),
  trashAccounts: (accountIds: string[]) => invoke<void>("trash_local_accounts", { accountIds }),
  restoreAccountTrash: (accountIds: string[]) => invoke<void>("restore_local_account_trash", { accountIds }),
  purgeAccountTrash: (accountIds: string[]) => invoke<void>("purge_local_account_trash", { accountIds }),
  setAccountProxy: (accountId: string, proxyUrl: string | null, bypassCommonProxy = false) => invoke("set_local_account_proxy", { input: { accountId, proxyUrl, bypassCommonProxy } }),
  getProxyPool: () => invoke<ProxyPoolSummary>("get_local_proxy_pool"),
  clearCommonProxy: () => invoke("set_local_common_proxy", { input: { proxyUrl: null } }),
  checkStoredProxy: (proxyId: string) => invoke<ProxyCheckResult>("check_local_stored_proxy", { proxyId }),
  testStoredProxySpeed: (proxyId: string, onStarted?: () => void) => invoke<ProxySpeedTestResult>("test_local_stored_proxy_speed", {
    proxyId,
    onStarted: new Channel<null>(onStarted),
  }),
  renameStoredProxy: (proxyId: string, label: string) => invoke<ProxyPoolSummary>("rename_local_stored_proxy", { proxyId, label }),
  importProxyPool: (proxyUrls: string[]) => invoke<ProxyPoolImportResult>("import_local_proxy_pool", { input: { proxyUrls } }),
  deleteStoredProxy: (proxyId: string) => invoke<ProxyPoolSummary>("delete_local_stored_proxy", { proxyId }),
  deleteStoredProxies: (proxyIds: string[]) => invoke<ProxyPoolSummary>("delete_local_stored_proxies", { input: { proxyIds } }),
  assignStoredProxy: (accountId: string, proxyId: string) => invoke<StoredProxyAssignmentResult>("assign_local_stored_proxy", { input: { accountId, proxyId } }),
  setStoredProxyAccounts: (proxyId: string, accountIds: string[]) => invoke<StoredProxyAssignmentResult>("set_local_stored_proxy_accounts", { input: { proxyId, accountIds } }),
  assignAutomaticProxies: (accountIds: string[]) => invoke<StoredProxyAssignmentResult>("assign_free_local_account_proxies", { input: { accountIds } }),
  exportLocalAccounts: (exportInput: AccountExportInput) => invoke<AccountExportResult>("export_local_accounts", { input: exportInput }),
  recoverLocalAccount: (accountId: string) => invoke<void>("recover_local_account", { accountId, confirmExternalOwnerStopped: true }),
  revealLocalAccountIdentity: (accountId: string) => invoke<RevealedAccountIdentity>("reveal_local_account_identity", { accountId }),
  revealLocalAccountLogin: (accountId: string) => invoke<AccountLoginDetails>("reveal_local_account_login", { accountId }),
  updateAccountLogin: (loginUpdate: AccountLoginUpdate) => invoke<AccountLoginDetails>("update_local_account_login", { input: loginUpdate }),
  previewTotpCode: (secret: string) => invoke<AccountTotpPreview>("preview_totp_code", { secret }),

  startOAuth: (openBrowser = true, accountId?: string, proxyId?: string, clientKind?: OAuthClientKind, automaticLogin = false, notesAccountId?: string) => invoke<OAuthFlow>("start_codex_oauth", { openBrowser, ...(accountId ? { accountId } : {}), ...(proxyId ? { proxyId } : {}), ...(clientKind ? { clientKind } : {}), ...(automaticLogin ? { automaticLogin: notesAccountId ? { notesAccountId } : true } : {}) }),
  resumeOAuth: (loginId: string) => invoke<OAuthFlow>("resume_codex_oauth", { loginId }),
  onOAuthStatus: (callback: (event: OAuthFlowEvent) => void) => listen<OAuthFlowEvent>("relay-oauth-status", (event) => callback(event.payload)),
  completeOAuth: (loginId: string) => invoke<OAuthCompletion>("complete_codex_oauth", { loginId }),
  cancelOAuth: (loginId: string) => invoke("cancel_codex_oauth", { loginId }),

  setPoolMembership: (accountIds: string[], sourceIds: string[], inPool: boolean) => invoke("set_local_pool_membership", { input: { accountIds, sourceIds, inPool } }),
  setModelEnabled: (modelId: string, enabled: boolean) => invoke("set_local_model_enabled", { input: { modelId, enabled } }),
  setModelReasoning: (modelId: string, allowedLevels: string[]) => invoke("set_local_model_reasoning", { input: { modelId, allowedLevels } }),
  setModelServiceTier: (modelId: string, serviceTier: DefaultServiceTier) => invoke("set_local_model_service_tier", { input: { modelId, serviceTier } }),
  setModelContext: (modelId: string, contextPolicy: ModelContextPolicy) => invoke<void>("set_local_model_context", { input: { modelId, contextPolicy } }),
  /** An empty list clears manual positions and restores provider-block order. */
  setModelDisplayOrder: (modelIds: string[]) => invoke("set_local_model_display_order", { input: { modelIds } }),
  exportLocalConfigurationPreset: () => invoke<string | null>("export_local_configuration_preset"),
  previewLocalConfigurationPreset: () => invoke<ConfigurationPresetPreview | null>("preview_local_configuration_preset"),
  applyLocalConfigurationPreset: (preview: ConfigurationPresetPreview) => invoke<ConfigurationPresetApplyResult>("apply_local_configuration_preset", { input: { baseRevision: preview.baseRevision, preset: preview.preset } }),
  updateRouting: (maxRetryCandidates: number, defaultServiceTier: DefaultServiceTier, poolRouting?: PoolRoutingPolicy, expectedPoolRouting?: PoolRoutingSnapshot) => invoke("update_local_routing", { input: { maxRetryCandidates, defaultServiceTier, poolRouting, expectedPoolRouting } }),

  startGateway: () => invoke("start_local_gateway"),
  stopGateway: () => invoke("stop_local_gateway"),
  restartGateway: () => invoke("restart_local_gateway"),
  updateGatewayPort: (port: number) => invoke("update_local_gateway_port", { port }),
  revealLocalGatewayApiKey: (network = false) => invoke<string>("reveal_local_gateway_api_key", network ? { network: true } : undefined),
  rotateLocalGatewayApiKey: (network = false) => invoke<string>("rotate_local_gateway_api_key", network ? { network: true } : undefined),
  updateChatgptQuotaReserve: (reserveBasisPoints: number) => invoke("update_chatgpt_interface_quota_reserve", { input: { reserveBasisPoints } }),
  setCodexBackgroundTasks: (enabled: boolean) => invoke("set_local_codex_background_tasks", { input: { enabled } }),
  getCodexMemoryModel: (stage: CodexMemoryStage) => invoke<string | null>("get_local_codex_memory_model", { stage }),
  setCodexMemoryModel: (stage: CodexMemoryStage, model: string | null, expectedModel: string | null) => invoke<string | null>("set_local_codex_memory_model", { input: { stage, model, expectedModel } }),
  setCodexMemoryEnabled: (stage: CodexMemoryStage, enabled: boolean) => invoke<RuntimeSnapshot>("set_local_codex_memory_enabled", { input: { stage, enabled } }),
  setChatgptRetryUntilAvailable: (enabled: boolean) => invoke("set_local_chatgpt_retry_until_available", { input: { enabled } }),
  setBlockDegradedRoutes: (enabled: boolean) => invoke("set_local_block_degraded_routes", { input: { enabled } }),
  setCodexWebsockets: (enabled: boolean) => invoke("set_local_codex_websockets", { input: { enabled } }),

  createAutomation: (automationInput: Record<string, unknown>) => invoke("create_quota_wake_automation", { input: automationInput }),
  updateAutomation: (taskId: string, automationInput: Record<string, unknown>) => invoke("update_quota_wake_automation", { taskId, input: automationInput }),
  setAutomationEnabled: (taskId: string, enabled: boolean) => invoke("set_quota_wake_automation_enabled", { taskId, enabled }),
  deleteAutomation: (taskId: string) => invoke("delete_quota_wake_automation", { taskId }),

  attachCodexGateway: (boundOauthAccountId: string | null = null, disableOauthBinding = false) => invoke<ProfileActivation>("attach_codex_to_local_gateway", { boundOauthAccountId, ...(disableOauthBinding ? { disableOauthBinding: true } : {}) }),
  stopManagedCodex: () => invoke<boolean>("stop_managed_codex_profile"),
  launchManagedCodex: () => invoke("launch_managed_codex_profile"),
  getOpenCodeConfigStatus: () => invoke<OpenCodeConfigStatus>("get_opencode_config_status"),
  createOpenCodeSnapshot: (snapshotName: string) => invoke<boolean>("create_opencode_snapshot", { name: snapshotName }),
  connectOpenCode: () => invoke<{ path: string; modelCount: number; backupCreated: boolean }>("connect_opencode_to_local_gateway"),
  launchOpenCodeSource: (sourceId: string) => invoke<{ path: string; modelCount: number; backupCreated: boolean }>("launch_opencode_source", { sourceId }),
  restartOpenCode: () => invoke<void>("restart_opencode_app"),
  restoreOpenCodeConfig: () => invoke<boolean>("restore_opencode_config"),
  getClaudeDesktopStatus: () => invoke<ClaudeDesktopStatus>("get_claude_desktop_status"),
  createClaudeDesktopSnapshot: (snapshotName: string) => invoke<boolean>("create_claude_desktop_snapshot", { name: snapshotName }),
  connectClaudeDesktop: () => invoke<ClaudeDesktopConnection>("connect_claude_desktop"),
  launchClaudeDesktop: () => invoke<void>("launch_claude_desktop"),
  restoreClaudeDesktop: () => invoke<boolean>("restore_claude_desktop"),
  profileSnapshots: () => invoke<ProfileSnapshotList>("list_codex_profile_snapshots"),
  createProfileSnapshot: (snapshotName: string) => invoke<ProfileSnapshot>("create_codex_profile_snapshot", { name: snapshotName }),
  restoreProfileSnapshot: (snapshotId: string) => invoke<void>("restore_full_codex_profile_snapshot", { snapshotId }),
  deleteProfileSnapshot: (snapshotId: string) => invoke("delete_codex_profile_snapshot", { snapshotId }),
  launchCodexAccount: (accountId: string) => invoke<ProfileActivation>("launch_codex_account", { accountId }),
  launchCodexSource: (sourceId: string) => invoke<ProfileActivation>("launch_codex_source", { sourceId }),
  storageInfo: () => invoke<RelayStorageInfo>("get_relay_storage_info"),
  recordFrontendDiagnostic: (diagnosticInput: {
    source: string;
    message: string;
    operation?: string;
    code?: string;
    stack?: string;
    fatal?: boolean;
  }) => invoke<void>("record_frontend_diagnostic", { input: diagnosticInput }),
  diagnosticSettings: () => invoke<DiagnosticSettings>("get_diagnostic_settings"),
  setDiagnosticDebugMode: (enabled: boolean) => invoke<DiagnosticSettings>("set_diagnostic_debug_mode", { enabled }),
  openFolder: (folder: "data" | "logs" | "error_logs" | "crash_logs" | "operation_logs" | "profile_backups" | "claude_desktop_backups" | "opencode_backups") =>
    invoke("open_relay_folder", { folder }),
  resetLocalData: () => invoke("reset_local_pool_data"),
  exportUsage: (rows: UsageExportRow[]) => invoke<string | null>("export_usage", { rows }),
  exportRequestDiagnostics: (requestId: string) => invoke<string | null>("export_request_diagnostics", { requestId }),

};

export function defaultWakeInput(taskName: string): Omit<WakeTask, "id" | "createdAtMs" | "updatedAtMs" | "trigger"> {
  return {
    name: taskName,
    enabled: true,
    accountSelector: { kind: "all_eligible" },
    windowKinds: ["primary"],
    modelPolicy: { kind: "lightest_supported" },
    executionPolicy: "automatic",
    jitterSeconds: 0,
    maxAttemptsPerCycle: 1,
  };
}
