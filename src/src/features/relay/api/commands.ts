import { invoke as tauriInvoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import type {
  TrashedAccountSummary,
  AccountExportInput,
  AccountExportResult,
  AccountLoginDetails,
  AccountLoginUpdate,
  AccountTotpPreview,
  AccountImportProgress,
  AccountTransferProgress,
  CandidateRuntimeSnapshot,
  ConfirmAccountImportResponse,
  ConfigurationPresetApplyResult,
  ConfigurationPresetPreview,
  ConsumeResetCreditResponse,
  DefaultServiceTier,
  RequestTimezoneUpdate,
  DiagnosticSettings,
  ImportSession,
  CacheSessionRecord,
  LocalUsagePage,
  OpenCodeConfigStatus,
  ProfileSnapshot,
  ProfileSnapshotList,
  MoveAccountsToRemoteResult,
  OAuthFlow,
  OAuthClientKind,
  OAuthFlowEvent,
  OAuthCompletion,
  ProfileActivation,
  ProfileBinding,
  ProxyPoolImportResult,
  ProxyCheckResult,
  ProxyPoolSummary,
  RelayStorageInfo,
  RevealedAccountIdentity,
  RemoteTarget,
  RemoteUsagePage,
  RemoteUsageQuery,
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
import type { ApplicationContextPolicy } from "./types/runtime";

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

export type UiState = {
  providerActive: boolean;
  codexRunning: boolean;
  hasSavedApiKey: boolean;
};

export const relayCommands = {
  setChatgptContextPolicy: (policy: ApplicationContextPolicy) =>
    invoke<RuntimeSnapshot>("set_local_chatgpt_context_policy", { input: policy }),
  readyState: () => invoke<UiState>("get_state"),
  onStateChanged: (callback: () => void) => listen("zenith-state-changed", callback),
  onRuntimeActivity: (callback: (activity: RuntimeActivitySnapshot) => void) => listen<RuntimeActivitySnapshot>("zenith-runtime-activity", (event) => callback(event.payload)),
  onUsageRecorded: (callback: () => void) => listen("zenith-usage-recorded", callback),
  localState: () => invoke<RuntimeSnapshot>("get_local_runtime_state"),
  localRuntimeOrder: () => invoke<CandidateRuntimeSnapshot[]>("get_local_runtime_order"),
  remoteState: () => invoke<RuntimeSnapshot | null>("get_remote_server_state"),
  remoteRuntimeOrder: () => invoke<CandidateRuntimeSnapshot[] | null>("get_remote_runtime_order"),
  remoteUsage: (usageQuery: RemoteUsageQuery = {}) => invoke<RemoteUsagePage | null>("get_remote_server_usage", { input: usageQuery }),
  revealRemoteGatewayApiKey: () => invoke<string>("reveal_remote_gateway_api_key"),
  rotateRemoteGatewayApiKey: () => invoke<string>("rotate_remote_gateway_api_key"),
  localUsagePage: (usageQuery: RemoteUsageQuery = {}) => invoke<LocalUsagePage>("get_local_usage_page", { input: usageQuery }),
  localCacheSessions: (usageQuery: RemoteUsageQuery = {}) => invoke<CacheSessionRecord[]>("get_local_cache_sessions", { input: usageQuery }),
  clearLocalUsage: () => invoke("clear_local_usage"),

  createSource: (sourceInput: Record<string, unknown>) => invoke("create_local_source", { input: sourceInput }),
  updateSource: (sourceInput: Record<string, unknown>) => invoke("update_local_source", { input: sourceInput }),
  rotateSourceKey: (sourceId: string, apiKey: string) => invoke("rotate_local_source_key", { sourceId, apiKey }),
  setSourceEnabled: (sourceId: string, enabled: boolean) => invoke("set_local_source_enabled", { sourceId, enabled }),
  deleteSource: (sourceId: string) => invoke("delete_local_source", { sourceId }),
  testSource: (sourceId: string) => invoke("test_local_source", { sourceId }),
  refreshSourceData: (sourceId: string) => invoke("refresh_local_source_data", { sourceId }),
  localSourceStats: (sourceId: string, force = false) => invoke<SourceStats>("get_local_source_stats", { sourceId, force }),
  remoteSourceStats: (sourceId: string, force = false) => invoke<SourceStats>("get_remote_source_stats", { sourceId, force }),

  startImport: (content: string) => invoke<ImportSession>("start_local_account_import", { input: { content } }),
  previewImportFiles: (paths?: string[]) => invoke<ImportSession | null>("preview_local_account_import_files", paths ? { paths } : {}),
  previewRemoteImportFiles: (paths?: string[]) => invoke<ImportSession | null>("preview_remote_account_import_files", paths ? { paths } : {}),
  prepareImport: (sessionId: string, probeQuota = false) => invoke<ImportSession>("prepare_local_account_import", { input: { sessionId, probeQuota } }),
  confirmImport: (sessionId: string, selectedItemIds: string[], addToPool = false) => invoke<ConfirmAccountImportResponse>("confirm_local_account_import", { input: { sessionId, selectedItemIds, discoverModels: false, probeQuota: false, models: [], addToPool } }),
  onImportProgress: (callback: (event: AccountImportProgress) => void) => listen<AccountImportProgress>("relay-account-import-progress", (event) => callback(event.payload)),
  cancelImport: (sessionId: string) => invoke("cancel_local_account_import", { sessionId }),
  refreshAccountQuota: (accountId: string) => invoke("refresh_local_account_quota", { accountId }),
  refreshAllAccountQuotas: () => invoke<Array<{ accountId: string; status: "succeeded" | "failed" }>>("refresh_all_local_account_quotas"),
  consumeResetCredit: (accountId: string) => invoke<ConsumeResetCreditResponse>("consume_local_reset_credit", { accountId }),
  updateAccount: (accountUpdate: Record<string, unknown>) => invoke("update_local_account", { input: accountUpdate }),
  setAccountEnabled: (accountId: string, enabled: boolean) => invoke("set_local_account_enabled", { accountId, enabled }),
  deleteAccount: (accountId: string) => invoke("delete_local_account", { accountId }),
  deleteAccounts: (accountIds: string[]) => invoke("delete_local_accounts", { accountIds }),
  accountTrash: () => invoke<TrashedAccountSummary[]>("list_local_account_trash"),
  trashAccounts: (accountIds: string[]) => invoke<void>("trash_local_accounts", { accountIds }),
  restoreAccountTrash: (accountIds: string[]) => invoke<void>("restore_local_account_trash", { accountIds }),
  purgeAccountTrash: (accountIds: string[]) => invoke<void>("purge_local_account_trash", { accountIds }),
  remoteAccountTrash: () => invoke<TrashedAccountSummary[]>("execute_remote_server_action", { input: { action: { type: "list_account_trash" }, payload: null } }),
  setAccountProxy: (accountId: string, proxyUrl: string | null, bypassCommonProxy = false) => invoke("set_local_account_proxy", { input: { accountId, proxyUrl, bypassCommonProxy } }),
  getProxyPool: () => invoke<ProxyPoolSummary>("get_local_proxy_pool"),
  checkStoredProxy: (proxyId: string) => invoke<ProxyCheckResult>("check_local_stored_proxy", { proxyId }),
  importProxyPool: (proxyUrls: string[]) => invoke<ProxyPoolImportResult>("import_local_proxy_pool", { input: { proxyUrls } }),
  deleteStoredProxy: (proxyId: string) => invoke<ProxyPoolSummary>("delete_local_stored_proxy", { proxyId }),
  deleteStoredProxies: (proxyIds: string[]) => invoke<ProxyPoolSummary>("delete_local_stored_proxies", { input: { proxyIds } }),
  assignStoredProxy: (accountId: string, proxyId: string) => invoke<StoredProxyAssignmentResult>("assign_local_stored_proxy", { input: { accountId, proxyId } }),
  setStoredProxyAccounts: (proxyId: string, accountIds: string[]) => invoke<StoredProxyAssignmentResult>("set_local_stored_proxy_accounts", { input: { proxyId, accountIds } }),
  assignAutomaticProxies: (accountIds: string[]) => invoke<StoredProxyAssignmentResult>("assign_free_local_account_proxies", { input: { accountIds } }),
  exportLocalAccounts: (exportInput: AccountExportInput) => invoke<AccountExportResult>("export_local_accounts", { input: exportInput }),
  exportRemoteAccounts: (exportInput: AccountExportInput) => invoke<AccountExportResult>("export_remote_accounts", { input: exportInput }),
  moveAccountsToRemote: (accountIds: string[]) => invoke<MoveAccountsToRemoteResult>("move_local_accounts_to_remote", { input: { accountIds } }),
  returnAccountToLocal: (localAccountId: string) => invoke<{ localAccountId: string }>("return_remote_account_to_local", { input: { localAccountId } }),
  forceActivateRemoteAccountLocally: (localAccountId: string) => invoke<{ localAccountId: string }>("force_activate_remote_account_locally", { input: { localAccountId, confirmRemoteMayStillBeRunning: true } }),
  onAccountTransferProgress: (callback: (event: AccountTransferProgress) => void) => listen<AccountTransferProgress>("relay-account-transfer-progress", (event) => callback(event.payload)),
  revealLocalAccountIdentity: (accountId: string) => invoke<RevealedAccountIdentity>("reveal_local_account_identity", { accountId }),
  revealLocalAccountLogin: (accountId: string) => invoke<AccountLoginDetails>("reveal_local_account_login", { accountId }),
  updateAccountLogin: (loginUpdate: AccountLoginUpdate) => invoke<AccountLoginDetails>("update_local_account_login", { input: loginUpdate }),
  previewTotpCode: (secret: string) => invoke<AccountTotpPreview>("preview_totp_code", { secret }),
  revealRemoteAccountIdentity: (accountId: string) => invoke<RevealedAccountIdentity>("reveal_remote_account_identity", { accountId }),

  startOAuth: (openBrowser = true, accountId?: string, proxyId?: string, clientKind?: OAuthClientKind) => invoke<OAuthFlow>("start_codex_oauth", { openBrowser, ...(accountId ? { accountId } : {}), ...(proxyId ? { proxyId } : {}), ...(clientKind ? { clientKind } : {}) }),
  resumeOAuth: (loginId: string) => invoke<OAuthFlow>("resume_codex_oauth", { loginId }),
  onOAuthStatus: (callback: (event: OAuthFlowEvent) => void) => listen<OAuthFlowEvent>("relay-oauth-status", (event) => callback(event.payload)),
  completeOAuth: (loginId: string) => invoke<OAuthCompletion>("complete_codex_oauth", { loginId }),
  cancelOAuth: (loginId: string) => invoke("cancel_codex_oauth", { loginId }),

  setPoolMembership: (accountIds: string[], sourceIds: string[], inPool: boolean) => invoke("set_local_pool_membership", { input: { accountIds, sourceIds, inPool } }),
  setModelEnabled: (modelId: string, enabled: boolean) => invoke("set_local_model_enabled", { input: { modelId, enabled } }),
  setModelReasoning: (modelId: string, allowedLevels: string[]) => invoke("set_local_model_reasoning", { input: { modelId, allowedLevels } }),
  setModelServiceTier: (modelId: string, serviceTier: DefaultServiceTier) => invoke("set_local_model_service_tier", { input: { modelId, serviceTier } }),
  /** An empty list clears manual positions and restores provider-block order. */
  setModelDisplayOrder: (modelIds: string[]) => invoke("set_local_model_display_order", { input: { modelIds } }),
  exportLocalConfigurationPreset: () => invoke<string | null>("export_local_configuration_preset"),
  previewLocalConfigurationPreset: () => invoke<ConfigurationPresetPreview | null>("preview_local_configuration_preset"),
  applyLocalConfigurationPreset: (preview: ConfigurationPresetPreview) => invoke<ConfigurationPresetApplyResult>("apply_local_configuration_preset", { input: { baseRevision: preview.baseRevision, preset: preview.preset } }),
  updateRouting: (maxRetryCandidates: number, defaultServiceTier: DefaultServiceTier, poolRouting?: PoolRoutingPolicy, expectedPoolRouting?: PoolRoutingSnapshot) => invoke("update_local_routing", { input: { maxRetryCandidates, defaultServiceTier, poolRouting, expectedPoolRouting } }),

  syncCodexDefaultServiceTier: (defaultServiceTier: DefaultServiceTier) => invoke<void>("sync_codex_default_service_tier", { defaultServiceTier }),
  startGateway: () => invoke("start_local_gateway"),
  stopGateway: () => invoke("stop_local_gateway"),
  restartGateway: () => invoke("restart_local_gateway"),
  updateGatewayPort: (port: number) => invoke("update_local_gateway_port", { port }),
  setRequestTimezone: (input: RequestTimezoneUpdate) => invoke("set_local_request_timezone", { input }),
  requestTimezoneCatalog: () => invoke<string[]>("get_request_timezone_catalog"),
  revealLocalGatewayApiKey: () => invoke<string>("reveal_local_gateway_api_key"),
  rotateLocalGatewayApiKey: () => invoke<string>("rotate_local_gateway_api_key"),
  updateChatgptQuotaReserve: (reserveBasisPoints: number) => invoke("update_chatgpt_interface_quota_reserve", { input: { reserveBasisPoints } }),
  setCodexBackgroundTasks: (enabled: boolean) => invoke("set_local_codex_background_tasks", { input: { enabled } }),
  setRemoteCodexBackgroundTasks: (enabled: boolean) => invoke("execute_remote_server_action", { input: { action: { type: "set_codex_background_tasks" }, payload: { enabled } } }),
  setChatgptRetryUntilAvailable: (enabled: boolean) => invoke("set_local_chatgpt_retry_until_available", { input: { enabled } }),
  setRemoteChatgptRetryUntilAvailable: (enabled: boolean) => invoke("execute_remote_server_action", { input: { action: { type: "set_chatgpt_retry_until_available" }, payload: { enabled } } }),
  setBlockDegradedRoutes: (enabled: boolean) => invoke("set_local_block_degraded_routes", { input: { enabled } }),
  setRemoteBlockDegradedRoutes: (enabled: boolean) => invoke("execute_remote_server_action", { input: { action: { type: "set_block_degraded_routes" }, payload: { enabled } } }),
  setCodexWebsockets: (enabled: boolean) => invoke("set_local_codex_websockets", { input: { enabled } }),
  setCodexProfileWebsockets: (enabled: boolean) => invoke("set_codex_profile_websockets", { input: { enabled } }),
  setRemoteCodexWebsockets: (enabled: boolean) => invoke("execute_remote_server_action", { input: { action: { type: "set_codex_websockets" }, payload: { enabled } } }),

  createAutomation: (automationInput: Record<string, unknown>) => invoke("create_quota_wake_automation", { input: automationInput }),
  updateAutomation: (taskId: string, automationInput: Record<string, unknown>) => invoke("update_quota_wake_automation", { taskId, input: automationInput }),
  setAutomationEnabled: (taskId: string, enabled: boolean) => invoke("set_quota_wake_automation_enabled", { taskId, enabled }),
  deleteAutomation: (taskId: string) => invoke("delete_quota_wake_automation", { taskId }),

  attachCodexGateway: (boundOauthAccountId: string | null = null, disableOauthBinding = false) => invoke<ProfileActivation>("attach_codex_to_local_gateway", { boundOauthAccountId, ...(disableOauthBinding ? { disableOauthBinding: true } : {}) }),
  attachCodexRemoteGateway: () => invoke<ProfileActivation>("attach_codex_to_remote_gateway"),
  stopManagedCodex: () => invoke<boolean>("stop_managed_codex_profile"),
  launchManagedCodex: () => invoke("launch_managed_codex_profile"),
  getOpenCodeConfigStatus: () => invoke<OpenCodeConfigStatus>("get_opencode_config_status"),
  createOpenCodeSnapshot: (snapshotName: string) => invoke<boolean>("create_opencode_snapshot", { name: snapshotName }),
  connectOpenCode: () => invoke<{ path: string; modelCount: number; backupCreated: boolean }>("connect_opencode_to_local_gateway"),
  launchOpenCodeSource: (sourceId: string) => invoke<{ path: string; modelCount: number; backupCreated: boolean }>("launch_opencode_source", { sourceId }),
  restartOpenCode: () => invoke<void>("restart_opencode_app"),
  restoreOpenCodeConfig: () => invoke<boolean>("restore_opencode_config"),
  profileSnapshots: () => invoke<ProfileSnapshotList>("list_codex_profile_snapshots"),
  createProfileSnapshot: (snapshotName: string) => invoke<ProfileSnapshot>("create_codex_profile_snapshot", { name: snapshotName }),
  restoreProfileSnapshot: (snapshotId: string) => invoke<void>("restore_full_codex_profile_snapshot", { snapshotId }),
  deleteProfileSnapshot: (snapshotId: string) => invoke("delete_codex_profile_snapshot", { snapshotId }),
  launchCodexAccount: (accountId: string) => invoke<ProfileActivation>("launch_codex_account", { accountId }),
  launchCodexSource: (sourceId: string) => invoke<ProfileActivation>("launch_codex_source", { sourceId }),
  profileBindings: () => invoke<ProfileBinding[]>("list_codex_account_bindings"),
  restoreDefaultAccountProfile: () => invoke("restore_codex_account_profile", { profileDir: null }),
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
  openFolder: (folder: "data" | "logs" | "error_logs" | "crash_logs" | "operation_logs" | "profile_backups" | "opencode_backups") =>
    invoke("open_relay_folder", { folder }),
  resetLocalData: () => invoke("reset_local_pool_data"),
  exportUsage: (rows: UsageExportRow[]) => invoke<string | null>("export_usage", { rows }),

  connectRemote: (connectionInput: Record<string, unknown>) => invoke<{ target: RemoteTarget }>("connect_remote_server", { input: connectionInput }),
  remoteLinkedAccountCount: () => invoke<number>("get_remote_linked_account_count"),
  disconnectRemote: () => invoke("disconnect_remote_server"),
  refreshRemoteCapabilities: () => invoke("refresh_remote_server_capabilities"),
  exportRemoteConfigurationPreset: () => invoke<string | null>("export_remote_configuration_preset"),
  previewRemoteConfigurationPreset: () => invoke<ConfigurationPresetPreview | null>("preview_remote_configuration_preset"),
  applyRemoteConfigurationPreset: (preview: ConfigurationPresetPreview) => invoke<ConfigurationPresetApplyResult>("apply_remote_configuration_preset", { input: { baseRevision: preview.baseRevision, preset: preview.preset } }),
  prepareRemoteDeployment: (publicBaseUrl: string) => invoke<{ directory: string; publicBaseUrl: string; managementToken: string; vaultKey: string; composeCommand: string }>("prepare_remote_server_deployment", { input: { publicBaseUrl } }),
  remoteAction: (action: Record<string, unknown>, payload?: unknown) => invoke("execute_remote_server_action", { input: { action, payload: payload ?? null } }),
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
