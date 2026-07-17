export type RelayMode = "local" | "remote" | "zenith";
export type PageId = "overview" | "connections" | "pool" | "gateway" | "usage" | "profiles" | "settings";
export type DefaultServiceTier = "standard" | "fast";

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

export type ApiEquivalentSummary = {
  microUsd: number;
  pricedTokens: number;
  unpricedTokens: number;
};

export type SourceSummary = {
  id: string;
  name: string;
  enabled: boolean;
  inPool: boolean;
  draining: boolean;
  baseUrl: string;
  wireApi: "responses" | "chat_completions" | "messages";
  models: string[];
  allowedModels: string[];
  excludedModels: string[];
  priority: number;
  weight: number;
  apiEquivalent: ApiEquivalentSummary;
  secretAvailable: boolean;
  lastErrorCode: string | null;
};

export type AccountSummary = {
  id: string;
  label: string;
  identityHint: string;
  enabled: boolean;
  inPool: boolean;
  draining: boolean;
  authState: string | { state: string; reason?: string };
  health: string;
  models: string[];
  allowedModels: string[];
  excludedModels: string[];
  priority: number;
  weight: number;
  apiEquivalent: ApiEquivalentSummary;
  subscription: { planType: string | null; activeUntilMs: number | null; status: string; updatedAtMs: number | null };
  quota: QuotaSnapshot;
  secretAvailable: boolean;
  proxyMode?: "direct" | "common" | "account";
  proxyAvailable?: boolean;
  routingExclusion?: "free_plan_policy" | null;
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
  system: boolean;
  sourceIds: string[] | null;
  accountIds: string[] | null;
  allowedModels: string[];
  excludedModels: string[];
  modelPrefix: string | null;
  createdAtMs: number;
  lastUsedAtMs: number | null;
};

export type ModelSummary = {
  id: string;
  enabled: boolean;
  memberCount: number;
  catalogRank: number | null;
  inputMicroUsdPerMillion: number | null;
  outputMicroUsdPerMillion: number | null;
};

export type CandidateRuntimeSnapshot = {
  candidateId: string;
  kind: "api_source" | "oauth_account";
  available: boolean;
  inFlight: number;
  lastUsedAtMs: number | null;
  nextRetryAtMs: number | null;
  halfOpen: boolean;
  dispatches: number;
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
    maxRetryCandidates?: number | null;
    routingStrategy?: RoutingStrategy | null;
    defaultServiceTier?: DefaultServiceTier | null;
    imageBaseModel?: string | null;
    models?: ModelSummary[];
    commonProxyConfigured?: boolean;
    commonProxyAvailable?: boolean;
    accountProxyRequired?: boolean;
    quotaRefreshIntervalSeconds?: number;
    quotaRequestTimeoutSeconds?: number;
    useFreeAccounts?: boolean;
    routingOrder?: CandidateRuntimeSnapshot[];
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

export type RoutingStrategy = "adaptive" | "oldest_account";

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

export type RoutingDiagnostics = {
  reason: "response_affinity" | "prompt_cache_affinity" | "session_affinity" | "connection_affinity" | "only_eligible" | "routing_tier" | "parallel_load" | "pool_policy" | "quota_headroom" | "adaptive_balance" | "oldest_account" | "fair_rotation" | "least_recently_used" | "manual_priority" | "manual_weight" | "stable_tie_break";
  eligibleCandidates: number;
  quotaRemainingBasisPoints: number | null;
  inFlightBefore: number;
  dispatchesBefore: number;
};

export type LocalUsage = {
  id: number;
  createdAt: string;
  requestId: string;
  attempt: number;
  localKeyId: string;
  sourceId: string;
  accountId?: string | null;
  routing?: RoutingDiagnostics | null;
  requestedModel: string | null;
  resolvedModel: string | null;
  wireApi: "responses" | "chat_completions" | "messages";
  success: boolean;
  httpStatus: number;
  errorCategory: string | null;
  latencyMs: number;
  ttftMs: number | null;
  generationMs: number | null;
  inputTokens: number | null;
  cachedInputTokens: number | null;
  cacheWriteInputTokens?: number | null;
  reasoningTokens: number | null;
  outputTokens: number | null;
  totalTokens: number | null;
};

export type UsageTotals = {
  requests: number;
  successfulRequests: number;
  latencyMs: number;
  ttftMs: number;
  ttftSamples: number;
  generationMs: number;
  generationSamples: number;
  generationOutputTokens: number;
  inputTokens: number;
  cachedInputTokens: number;
  cachedInputSamples: number;
  cacheWriteInputTokens?: number;
  cacheWriteInputSamples?: number;
  reasoningTokens: number;
  outputTokens: number;
  totalTokens: number;
  speedOutputTokens: number;
  speedDurationMs: number;
  apiEquivalent: ApiEquivalentSummary;
};

export type UsageGroup = {
  key: string;
  label?: string | null;
  totals: UsageTotals;
};

export type UsageBucket = {
  startMs: number;
  totals: UsageTotals;
};

export type LocalUsagePage = {
  events: LocalUsage[];
  total: number;
  page: number;
  pageSize: number;
  totalPages: number;
  totals: UsageTotals;
  models: UsageGroup[];
  poolMembers: UsageGroup[];
  buckets?: UsageBucket[];
};

export type UsageExportRow = {
  time: string;
  success: boolean;
  model: string | null;
  connection: string;
  latencyMs: number;
  ttftMs: number | null;
  inputTokens: number | null;
  cachedInputTokens: number | null;
  cacheWriteInputTokens?: number | null;
  reasoningTokens: number | null;
  outputTokens: number | null;
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
  candidateLabel?: string | null;
  routing?: RoutingDiagnostics | null;
  requestedModel: string | null;
  resolvedModel: string | null;
  wireApi: "responses" | "chat_completions" | "messages";
  success: boolean;
  httpStatus: number;
  errorCategory: string | null;
  latencyMs: number;
  ttftMs?: number | null;
  generationMs?: number | null;
  inputTokens: number | null;
  cachedInputTokens: number | null;
  cacheWriteInputTokens?: number | null;
  reasoningTokens: number | null;
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
  bucketMs?: number;
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
  totals?: UsageTotals;
  models?: UsageGroup[];
  poolMembers?: UsageGroup[];
  buckets?: UsageBucket[];
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
      expiresAt?: string;
      subscriptionExpiresAt?: string;
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

export type OAuthFlowStatus = "pending" | "callback_received" | "callback_rejected" | "canceled" | "completed" | "expired" | "failed";

export type OAuthFlow = {
  loginId: string;
  authorizationUrl: string;
  redirectUri: string;
  expiresAtMs: number;
  status: OAuthFlowStatus;
};

export type OAuthFlowEvent = Pick<OAuthFlow, "loginId" | "status">;

export type OAuthCompletion = {
  account: { id: string };
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
  credentialKind: "oauth_account" | "api_key" | "local_gateway";
  credentialId: string;
  boundOauthAccountId: string | null;
  active: boolean;
};

export type ProfileActivation = {
  binding: ProfileBinding;
  previousCredentialKind: ProfileBinding["credentialKind"] | null;
  repairRecommended: boolean;
  stoppedRunningClient: boolean;
};

export type ProfileSnapshot = {
  id: string;
  name: string;
  profileDir: string;
  createdAtMs: number;
  configAvailable: boolean;
  authAvailable: boolean;
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

export type RelayStorageInfo = {
  rootPath: string;
  dataPath: string;
  recoveryPath: string;
  cachePath: string;
  logsPath: string;
  chatgptProfilePath: string;
  legacyDataPath: string | null;
};
