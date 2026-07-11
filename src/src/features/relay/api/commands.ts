import { invoke } from "@tauri-apps/api/core";
import type {
  ConfirmAccountImportResponse,
  ImportSession,
  LocalUsage,
  OAuthFlow,
  ProfileBinding,
  RemoteTarget,
  RemoteUsagePage,
  RuntimeSnapshot,
  SupportExportContext,
  UsageExportRow,
  WakeTask,
} from "./types";

export const relayCommands = {
  localState: () => invoke<RuntimeSnapshot>("get_local_runtime_state"),
  remoteState: () => invoke<RuntimeSnapshot | null>("get_remote_server_state"),
  remoteUsage: (page = 1, pageSize = 100) => invoke<RemoteUsagePage | null>("get_remote_server_usage", { page, pageSize }),
  localUsage: (limit = 100) => invoke<LocalUsage[]>("get_local_usage", { limit }),

  createSource: (input: Record<string, unknown>) => invoke("create_local_source", { input }),
  updateSource: (input: Record<string, unknown>) => invoke("update_local_source", { input }),
  rotateSourceKey: (sourceId: string, apiKey: string) => invoke("rotate_local_source_key", { sourceId, apiKey }),
  setSourceEnabled: (sourceId: string, enabled: boolean) => invoke("set_local_source_enabled", { sourceId, enabled }),
  deleteSource: (sourceId: string) => invoke("delete_local_source", { sourceId }),
  testSource: (sourceId: string) => invoke("test_local_source", { sourceId }),

  startImport: (content: string) => invoke<ImportSession>("start_local_account_import", { input: { content } }),
  resumeImport: (sessionId: string) => invoke<ImportSession>("resume_local_account_import", { sessionId }),
  prepareImport: (sessionId: string, probeQuota = true) => invoke<ImportSession>("prepare_local_account_import", { input: { sessionId, probeQuota } }),
  confirmImport: (sessionId: string, selectedItemIds: string[]) => invoke<ConfirmAccountImportResponse>("confirm_local_account_import", { input: { sessionId, selectedItemIds, discoverModels: true, probeQuota: true, models: [] } }),
  cancelImport: (sessionId: string) => invoke("cancel_local_account_import", { sessionId }),
  refreshAccountQuota: (accountId: string) => invoke("refresh_local_account_quota", { accountId }),
  refreshAllAccountQuotas: () => invoke("refresh_all_local_account_quotas"),
  updateAccount: (input: Record<string, unknown>) => invoke("update_local_account", { input }),
  setAccountEnabled: (accountId: string, enabled: boolean) => invoke("set_local_account_enabled", { accountId, enabled }),
  setAccountDraining: (accountId: string, draining: boolean) => invoke("set_local_account_draining", { accountId, draining }),
  deleteAccount: (accountId: string) => invoke("delete_local_account", { accountId }),

  startOAuth: () => invoke<OAuthFlow>("start_codex_oauth"),
  resumeOAuth: (loginId: string) => invoke<OAuthFlow>("resume_codex_oauth", { loginId }),
  oauthStatus: (loginId: string) => invoke<OAuthFlow>("get_codex_oauth_status", { loginId }),
  submitOAuthCallback: (loginId: string, callbackUrl: string) => invoke("submit_codex_oauth_callback", { loginId, callbackUrl }),
  completeOAuth: (loginId: string) => invoke("complete_codex_oauth", { loginId }),
  cancelOAuth: (loginId: string) => invoke("cancel_codex_oauth", { loginId }),

  createKey: (label: string) => invoke<{ key: unknown; secret: string }>("create_local_gateway_key", { label }),
  updateKey: (input: Record<string, unknown>) => invoke("update_local_gateway_key", { input }),
  rotateKey: (keyId: string) => invoke<{ key: unknown; secret: string }>("rotate_local_gateway_key", { keyId }),
  setKeyEnabled: (keyId: string, enabled: boolean) => invoke("set_local_gateway_key_enabled", { keyId, enabled }),
  deleteKey: (keyId: string) => invoke("delete_local_gateway_key", { keyId }),
  startGateway: () => invoke("start_local_gateway"),
  stopGateway: () => invoke("stop_local_gateway"),

  createAutomation: (input: Record<string, unknown>) => invoke("create_quota_wake_automation", { input }),
  updateAutomation: (taskId: string, input: Record<string, unknown>) => invoke("update_quota_wake_automation", { taskId, input }),
  setAutomationEnabled: (taskId: string, enabled: boolean) => invoke("set_quota_wake_automation_enabled", { taskId, enabled }),
  deleteAutomation: (taskId: string) => invoke("delete_quota_wake_automation", { taskId }),
  runWakeConfirmations: () => invoke<number>("run_due_quota_wake_confirmations", { maxClaims: 2 }),

  attachCodexGateway: (keyId: string) => invoke("attach_codex_to_local_gateway", { keyId }),
  launchCodex: () => invoke<string>("launch_saved_codex"),
  restoreCodex: () => invoke("restore_codex_profile"),
  attachCodexAccount: (accountId: string, profileDir?: string) => invoke("attach_codex_to_account", { accountId, profileDir: profileDir || null }),
  profileBindings: () => invoke<ProfileBinding[]>("list_codex_account_bindings"),
  restoreAccountProfile: (profileDir: string) => invoke("restore_codex_account_profile", { profileDir }),
  openFolder: (folder: "data" | "profile_backups") => invoke("open_relay_folder", { folder }),
  resetLocalData: () => invoke("reset_local_pool_data"),
  exportUsage: (rows: UsageExportRow[]) => invoke<string>("export_usage", { rows }),
  exportSupportBundle: (context: SupportExportContext) => invoke<string>("export_support_bundle", { context }),

  connectRemote: (input: Record<string, unknown>) => invoke<{ target: RemoteTarget }>("connect_remote_server", { input }),
  disconnectRemote: () => invoke("disconnect_remote_server"),
  refreshRemoteCapabilities: () => invoke("refresh_remote_server_capabilities"),
  prepareRemoteDeployment: (publicBaseUrl: string) => invoke<{ directory: string; publicBaseUrl: string; managementToken: string; composeCommand: string }>("prepare_remote_server_deployment", { input: { publicBaseUrl } }),
  remoteAction: (action: Record<string, unknown>, payload?: unknown) => invoke("execute_remote_server_action", { input: { action, payload: payload ?? null } }),
};

export function defaultWakeInput(name: string): Omit<WakeTask, "id" | "createdAtMs" | "updatedAtMs" | "trigger"> {
  return {
    name,
    enabled: true,
    accountSelector: { kind: "all_eligible" },
    windowKinds: ["primary", "secondary"],
    modelPolicy: { kind: "lightest_supported" },
    executionPolicy: "automatic",
    jitterSeconds: 0,
    maxAttemptsPerCycle: 1,
  };
}
