export type RelayMode = "local" | "remote" | "zenith";
export type PageId = "overview" | "connections" | "pool" | "gateway" | "usage" | "profiles" | "settings";

export type QuotaWindow = {
  kind: "primary" | "secondary";
  availableBasisPoints: number | null;
  explicitlyFull: boolean | null;
  resetAtMs: number | null;
  windowMinutes: number | null;
  observedAtMs: number;
};

export type SupplementalQuotaWindow = {
  id: string;
  label: string;
  window: QuotaWindow;
};

export type QuotaSnapshot = {
  primary: QuotaWindow | null;
  secondary: QuotaWindow | null;
  supplemental?: SupplementalQuotaWindow[];
  resetCreditsAvailable: number | null;
  updatedAtMs: number | null;
  error: { code: string; observedAtMs: number } | null;
};

export type QuotaWindowVisibility = {
  primary: boolean;
  secondary: boolean;
};

export type SourceSummary = {
  id: string;
  name: string;
  enabled: boolean;
  draining: boolean;
  baseUrl: string;
  wireApi: "responses" | "chat_completions" | "messages";
  models: string[];
  allowedModels: string[];
  excludedModels: string[];
  priority: number;
  weight: number;
  secretAvailable: boolean;
  lastErrorCode: string | null;
};

export type AccountSummary = {
  id: string;
  label: string;
  identityHint: string;
  enabled: boolean;
  draining: boolean;
  authState: string | { state: string; reason?: string };
  health: string;
  models: string[];
  allowedModels: string[];
  excludedModels: string[];
  priority: number;
  weight: number;
  subscription: { planType: string | null; activeUntilMs: number | null; status: string; updatedAtMs: number | null };
  quota: QuotaSnapshot;
  secretAvailable: boolean;
  proxyMode?: "direct" | "common" | "account";
  proxyAvailable?: boolean;
  lastErrorCode: string | null;
};

export type RevealedAccountIdentity = {
  accountId: string;
  identity: string;
};

export type KeySummary = {
  id: string;
  label: string;
  enabled: boolean;
  sourceIds: string[] | null;
  accountIds: string[] | null;
  allowedModels: string[];
  excludedModels: string[];
  modelPrefix: string | null;
  createdAtMs: number;
  lastUsedAtMs: number | null;
};

export type WakeTask = {
  id: string;
  name: string;
  enabled: boolean;
  accountSelector: { kind: "all_eligible" } | { kind: "account_ids" | "tags"; values: string[] };
  windowKinds: Array<"primary" | "secondary">;
  modelPolicy: { kind: "lightest_supported" } | { kind: "explicit"; value: string };
  trigger: { kind: "quota_full" };
  executionPolicy: "automatic" | "require_confirmation";
  jitterSeconds: number;
  maxAttemptsPerCycle: number;
  createdAtMs: number;
  updatedAtMs: number;
};

export type WakeHistory = {
  taskId: string;
  accountId: string;
  windowKind: "primary" | "secondary";
  modelId: string | null;
  outcome: string;
  startedAtMs: number;
  completedAtMs: number;
  errorCode: string | null;
};

export type RuntimeSnapshot = {
  schemaVersion: number;
  runtimeTarget: { kind: "local" | "remote"; connected: boolean; origin: string | null; serverId: string | null; version: string | null };
  gateway: {
    running: boolean;
    baseUrl: string;
    candidateCount: number;
    visibleModelIds: string[];
    commonProxyConfigured?: boolean;
    commonProxyAvailable?: boolean;
  };
  platform: string;
  capabilities: { features: string[]; [key: string]: unknown };
  sources: SourceSummary[];
  accounts: AccountSummary[];
  keys: KeySummary[];
  automations: WakeTask[];
  wakeHistory: WakeHistory[];
  warnings: string[];
};

export type ProxyAssignmentResult = {
  assigned: number;
  unused: number;
};

export type AccountExportFormat = "cpa" | "sub2api" | "cockpit" | "9router" | "codex" | "axon_hub" | "codex_manager";

export type AccountExportInput = {
  accountIds: string[];
  format: AccountExportFormat;
  destination: "copy" | "download";
};

export type AccountExportResult = {
  format: AccountExportFormat;
  accountCount: number;
  fileName: string;
  content?: string;
  path?: string;
};

export type LocalUsage = {
  id: number;
  createdAt: string;
  requestId: string;
  attempt: number;
  localKeyId: string;
  sourceId: string;
  accountId?: string | null;
  requestedModel: string | null;
  resolvedModel: string | null;
  wireApi: "responses" | "chat_completions" | "messages";
  success: boolean;
  httpStatus: number;
  errorCategory: string | null;
  latencyMs: number;
  inputTokens: number | null;
  outputTokens: number | null;
  totalTokens: number | null;
};

export type UsageExportRow = {
  time: string;
  success: boolean;
  model: string | null;
  connection: string;
  latencyMs: number;
  tokens: number | null;
  requestId: string | null;
  httpStatus: number | null;
  errorCategory: string | null;
};

export type SupportExportContext = {
  mode: RelayMode;
  schemaVersion: number | null;
  gatewayRunning: boolean;
  sourceCount: number;
  accountCount: number;
  keyCount: number;
  automationCount: number;
  usageCount: number;
  warningCount: number;
};

export type GatewayDiagnostic = {
  stream: boolean;
  model: string;
  latencyMs: number;
  bytesReceived: number;
};

export type RemoteUsage = {
  id: number;
  requestId: string;
  localKeyId: string;
  candidateKind: string;
  candidateHint: string;
  requestedModel: string | null;
  resolvedModel: string | null;
  wireApi: "responses" | "chat_completions" | "messages";
  success: boolean;
  httpStatus: number;
  errorCategory: string | null;
  latencyMs: number;
  inputTokens: number | null;
  outputTokens: number | null;
  totalTokens: number | null;
  createdAtMs: number;
};

export type RemoteUsageQuery = {
  page?: number;
  pageSize?: number;
  range?: "daily" | "weekly" | "monthly" | "custom";
  fromMs?: number;
  toMs?: number;
  modelQuery?: string;
  sourceOrAccountQuery?: string;
  localKeyQuery?: string;
  wireApi?: "responses" | "chat_completions" | "messages";
  success?: boolean;
  errorCategory?: string;
  requestIdQuery?: string;
};

export type RemoteUsagePage = {
  events: RemoteUsage[];
  total: number;
  page: number;
  pageSize: number;
  totalPages: number;
};

export type ImportSession = {
  sessionId: string;
  prepared: boolean;
  preview: {
    format: string;
    rows: Array<{
      itemId: string;
      label: string;
      identity: string;
      authMode: string;
      sourceName: string;
      quotaStatus: string;
      status: string;
      plan?: string;
      defaultSelected: boolean;
      selectable: boolean;
      existing: boolean;
      warnings: Array<{ code: string; count?: number }>;
      error?: { code: string; message: string };
    }>;
    warnings: Array<{ code: string; count?: number }>;
  };
};

export type ConfirmAccountImportResponse = {
  sessionId: string;
  results: Array<{
    itemId: string;
    status: "succeeded" | "failed";
    error?: { code: string; message: string };
  }>;
};

export type OAuthFlow = {
  loginId: string;
  authorizationUrl: string;
  redirectUri: string;
  expiresAtMs: number;
  status: string;
};

export type RemoteTarget = {
  origin: string;
  serverId: string;
  identityFingerprint: string;
  serverVersion: string;
  protocolVersion: number;
  allowInsecureHttp: boolean;
  connectedAtMs: number;
};

export type ProfileBinding = {
  profileDir: string;
  credentialKind: string;
  credentialId: string;
};

export type OpenCodeProfileState = {
  attached: boolean;
  backupAvailable: boolean;
  changed: boolean;
  configPath: string;
};

export type HistoryRepairPreview = {
  sessionId: string;
  targetProvider: "openai" | "zenith_relay_local";
  profileCount: number;
  rolloutFileCount: number;
  rolloutRecordCount: number;
  sqliteRowCount: number;
  codexRunning: boolean;
  expiresAtMs: number;
};

export type HistoryRepairResult = {
  backupId: string;
  backupPath: string;
  rolloutRecordsChanged: number;
  sqliteRowsChanged: number;
};

export type SupportBundlePreview = {
  bundle: {
    generatedAt: string;
    appVersion: string;
    platform: string;
    mode: "local" | "remote" | "zenith";
    schemaVersion: number | null;
    gatewayRunning: boolean;
    sourceCount: number;
    accountCount: number;
    keyCount: number;
    automationCount: number;
    usageCount: number;
    warningCount: number;
  };
  excluded: string[];
};
