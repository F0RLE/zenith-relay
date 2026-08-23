export type RelayMode = "local" | "remote" | "zenith";
export type PageId = "overview" | "connections" | "pool" | "gateway" | "usage" | "profiles" | "settings" | "help";
export type DefaultServiceTier = "standard" | "fast";
export type OperationalStatus = "rotation" | "quotaWait" | "unavailable" | "disabled";

export type QuotaWindow = {
  kind: "primary" | "secondary";
  providerCycleId?: string | null;
  windowStartMs?: number | null;
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
  limitReached: boolean;
  resetCreditsAvailable: number | null;
  directBalanceMicroUsd?: number | null;
  updatedAtMs: number | null;
  error: { code: string; occurredAtMs: number } | null;
};

export type ResetCredit = {
  status?: string;
  resetType?: string;
  grantedAt?: number;
  expiresAt?: number;
  redeemedAt?: number;
};

export type ResetCreditsSnapshot = {
  availableCount: number | null;
  credits: ResetCredit[];
  nextExpiresAt: number | null;
};

export type ConsumeResetCreditResponse = {
  refreshed: boolean;
  refreshError?: string;
};

export type ApiEquivalentSummary = {
  microUsd: number;
  pricedTokens: number;
  unpricedTokens: number;
};

export type ApiModelPriceOverride = {
  inputMicroUsdPerMillion: number;
  cachedInputMicroUsdPerMillion?: number | null;
  cacheWrite5mMicroUsdPerMillion?: number | null;
  cacheWrite1hMicroUsdPerMillion?: number | null;
  outputMicroUsdPerMillion: number;
};

export type SourceWireApi = "responses" | "chat_completions" | "messages";

export type SourceAdapter = "native" | "responses_to_messages" | "responses_to_gemini";

export type MessagesReasoningMode = "disabled" | "budget" | "adaptive";
export type CacheWriteTtl = "provider" | "5m" | "1h";

export type SourceProtocolBinding = {
  wireApi: SourceWireApi;
  modelIds: string[];
  /**
   * Older Relay records do not have adapter metadata. The editor and runtime
   * treat an omitted value as the native passthrough.
   */
  adapter?: SourceAdapter;
  reasoningMode?: MessagesReasoningMode;
  cacheWriteTtl?: CacheWriteTtl;
};

export type SourceSummary = {
  id: string;
  name: string;
  enabled: boolean;
  inPool: boolean;
  draining: boolean;
  operationalStatus: OperationalStatus;
  baseUrl: string;
  wireApi: SourceWireApi;
  protocolBindings?: SourceProtocolBinding[];
  models: string[];
  allowedModels: string[];
  excludedModels: string[];
  priority: number;
  weight: number;
  recoveryDelaySeconds: number;
  modelPriceOverrides?: Record<string, ApiModelPriceOverride>;
  detectedModelPrices?: Record<string, ApiModelPriceOverride>;
  apiEquivalent: ApiEquivalentSummary;
  secretAvailable: boolean;
  lastErrorCode: string | null;
};

export type SourceStats = {
  provider: "zenith" | "openrouter" | "unsupported";
  balanceMicroUsd: number | null;
  spentMicroUsd: number | null;
  requests: number | null;
  totalTokens: number | null;
};

export type AccountSummary = {
  id: string;
  label: string;
  identityHint: string;
  enabled: boolean;
  inPool: boolean;
  draining: boolean;
  authState: { state: string; reason?: string };
  health: string;
  operationalStatus: OperationalStatus;
  models: string[];
  allowedModels: string[];
  excludedModels: string[];
  priority: number;
  weight: number;
  apiEquivalent: ApiEquivalentSummary;
  purchaseCostMicroUsd?: number | null;
  subscription: { planType: string | null; activeUntilMs: number | null; status: string; updatedAtMs: number | null };
  quota: QuotaSnapshot;
  quotaRefreshStatus: "pending" | "refreshing" | "updated" | "failed" | "requires_reauth";
  secretAvailable: boolean;
  remoteLocation?: { serverId: string; remoteAccountId: string } | null;
  proxyMode?: "direct" | "common" | "account";
  proxyAvailable?: boolean;
  proxyId?: string | null;
  routingBlockReason?: "disabled" | "not_in_pool" | "draining" | "secret_unavailable" | "proxy_unavailable" | "reauth_required" | "auth_error" | "checkpoint" | "captcha" | "subscription_forbidden" | "subscription_expired" | "account_unhealthy" | "quota_exhausted" | null;
  lastErrorCode: string | null;
};

export type RevealedAccountIdentity = {
  accountId: string;
  identity: string;
};

export type ModelSummary = {
  id: string;
  enabled: boolean;
  memberCount: number;
  codexVisible: boolean;
  codexDisplayName: string;
  catalogRank: number | null;
  inputMicroUsdPerMillion: number | null;
  cachedInputMicroUsdPerMillion?: number | null;
  cacheWrite5mMicroUsdPerMillion?: number | null;
  cacheWrite1hMicroUsdPerMillion?: number | null;
  outputMicroUsdPerMillion: number | null;
  imageRequestPrices?: ImageRequestPrice[];
  customPrice: boolean;
  reasoningLevels?: string[];
  reasoningSupportedLevels?: string[];
  reasoningAllowedLevels?: string[];
  reasoningConfigurable?: boolean;
  reasoningProbeAvailable?: boolean;
  reasoningProbe?: ReasoningProbeProgress;
};

export type ImageRequestPrice = {
  operation: "generation" | "edit" | string;
  quality: string;
  size: string;
  microUsd: number;
};

export type ReasoningProbeProgress = {
  status: "queued" | "running" | "confirmed" | "rejected" | "inconclusive" | string;
  total: number;
  running: number;
  success: number;
  failed: number;
  confirmed: number;
  rejected: number;
  inconclusive: number;
  pending: number;
  lastProbeAt: string | null;
};

export type ModelReasoningProbeSourceResult = {
  sourceId: string;
  sourceName: string;
  available: boolean;
};

export type ModelReasoningProbeResult = {
  modelId: string;
  level: string;
  sourceCount: number;
  availableCount: number;
  appliedToSettings: boolean;
  sources: ModelReasoningProbeSourceResult[];
};

export type CandidateRuntimeSnapshot = {
  candidateId: string;
  kind: "api_source" | "oauth_account";
  available: boolean;
  inFlight: number;
  activeRequestCount?: number;
  activeModels?: Array<{
    model: string;
    requestCount: number;
  }>;
  modelRetries?: Array<{
    model: string;
    retryAtMs: number;
  }>;
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
  trigger: { kind: "quota_full" } | { kind: "weekly" };
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
  configurationRevision?: string | null;
  runtimeTarget: { kind: "local" | "remote"; connected: boolean; origin: string | null; serverId: string | null; version: string | null };
  gateway: {
    running: boolean;
    baseUrl: string;
    candidateCount: number;
    visibleModelIds: string[];
    maxRetryCandidates: number;
    cooldownAfterFailures?: number;
    keepLastCandidateAvailable?: boolean;
    routingStrategy: RoutingStrategy;
    subscriptionPlanOrder?: string[];
    defaultServiceTier: DefaultServiceTier;
    models?: ModelSummary[];
    commonProxyConfigured?: boolean;
    commonProxyAvailable?: boolean;
    commonProxyId?: string | null;
    accountProxyRequired?: boolean;
    quotaRequestTimeoutSeconds?: number;
    chatgptInterfaceQuotaReserveBasisPoints?: number;
    codexBackgroundTasksEnabled?: boolean;
    codexWebsocketsEnabled?: boolean;
    routingOrder?: CandidateRuntimeSnapshot[];
  };
  platform: string;
  capabilities: {
    features: string[];
    supportedWireApis?: Array<"responses" | "chat_completions" | "messages">;
    [key: string]: unknown;
  };
  sources: SourceSummary[];
  accounts: AccountSummary[];
  automations: WakeTask[];
  wakeHistory: WakeHistory[];
  warnings: string[];
};

type ConfigurationPresetMemberRule = {
  id: string;
  enabled: boolean;
  inPool: boolean;
  allowedModels: string[];
  excludedModels: string[];
  priority: number;
  weight: number;
};

export type ConfigurationPresetSourceRule = ConfigurationPresetMemberRule & {
  name: string;
  baseUrl: string;
  wireApi: SourceWireApi;
  protocolBindings?: SourceProtocolBinding[];
  serviceTier?: DefaultServiceTier;
  recoveryDelaySeconds: number;
  modelPriceOverrides: Record<string, ApiModelPriceOverride>;
};

export type ConfigurationPresetAccountRule = ConfigurationPresetMemberRule & {
  identityHint: string;
  proxyId: string | null;
  bypassCommonProxy?: boolean;
};

export type ConfigurationPreset = {
  format: "zenith-relay-configuration";
  schemaVersion: number;
  settings: {
    sources: ConfigurationPresetSourceRule[];
    accounts: ConfigurationPresetAccountRule[];
    routing: {
      maxRetryCandidates: number;
      cooldownAfterFailures?: number;
      keepLastCandidateAvailable?: boolean;
      routingStrategy: RoutingStrategy;
      subscriptionPlanOrder: string[];
      defaultServiceTier: DefaultServiceTier;
      imageBaseModel: string | null;
    };
    quota: {
      requestTimeoutSeconds: number;
      accountProxyRequired: boolean;
      commonProxyId: string | null;
    };
    hiddenModels: string[];
    modelPriceOverrides: Record<string, ApiModelPriceOverride>;
  };
};

export type ConfigurationPresetChange = {
  path: string;
  before: unknown;
  after: unknown;
};

export type ConfigurationPresetPreview = {
  baseRevision: string;
  preset: ConfigurationPreset;
  changes: ConfigurationPresetChange[];
};

export type ConfigurationPresetApplyResult = {
  previousRevision: string;
  revision: string;
  changes: ConfigurationPresetChange[];
};

export type RoutingStrategy = "adaptive" | "quota_highest" | "subscription_expiry" | "subscription_plan";

export type ProxyAssignmentResult = {
  assigned: number;
  unused: number;
};

export type ProxyPoolEntry = {
  id: string;
  endpoint: string;
  assignedAccountIds: string[];
  countryCode: string | null;
  region: string | null;
  createdAtMs: number;
};

export type ProxyPoolSummary = {
  entries: ProxyPoolEntry[];
  total: number;
  free: number;
  assigned: number;
};

export type ProxyPoolImportResult = {
  added: number;
  duplicates: number;
  pool: ProxyPoolSummary;
};

export type StoredProxyAssignmentResult = {
  assigned: number;
  unchanged: number;
  unavailable: number;
  pool: ProxyPoolSummary;
};

export type AccountExportFormat = "zenith" | "cpa" | "sub2api" | "cockpit" | "9router" | "codex" | "axon_hub" | "codex_manager";

export type AccountExportInput = {
  accountIds: string[];
  format: AccountExportFormat;
  destination: "copy" | "download";
  description?: string;
};

export type AccountExportResult = {
  format: AccountExportFormat;
  accountCount: number;
  fileName: string;
  content?: string;
  path?: string;
};

export type MoveAccountsToRemoteResult = {
  moved: number;
  remoteAccountIds: string[];
};

export type RoutingDiagnostics = {
  reason: "response_affinity" | "prompt_cache_affinity" | "session_affinity" | "connection_affinity" | "only_eligible" | "routing_tier" | "source_role" | "parallel_load" | "source_load" | "pool_policy" | "quota_headroom" | "adaptive_balance" | "subscription_expiry" | "subscription_plan" | "weighted_rotation" | "fair_rotation" | "fallback_attempt" | "least_recently_used" | "manual_priority" | "manual_weight" | "stable_tie_break";
  eligibleCandidates: number;
  quotaRemainingBasisPoints: number | null;
  inFlightBefore: number;
  dispatchesBefore: number;
};

export type ToolUseDiagnostics = {
  clientToolCount: number;
  forwardedToolCount: number;
  toolChoice: "unspecified" | "auto" | "required" | "none" | "allowed_tools" | "specific";
  toolCallCount: number;
  textOutput: boolean;
  terminalOutput: "unknown" | "empty" | "text" | "tool_call" | "mixed";
};

export type ErrorOrigin = "provider" | "account" | "relay";
export type ReasoningEffort = "minimal" | "low" | "medium" | "high" | "xhigh" | "max" | "ultra";

export type LocalUsage = {
  id: number;
  createdAt: string;
  requestId: string;
  attempt: number;
  sourceId: string;
  accountId?: string | null;
  clientContextId?: string | null;
  routing?: RoutingDiagnostics | null;
  requestedModel: string | null;
  resolvedModel: string | null;
  requestedReasoningEffort?: ReasoningEffort | null;
  effectiveReasoningEffort?: ReasoningEffort | null;
  wireApi: "responses" | "chat_completions" | "messages";
  serviceTier?: DefaultServiceTier;
  appliedServiceTier?: DefaultServiceTier | null;
  success: boolean;
  httpStatus: number;
  errorCategory: string | null;
  errorOrigin?: ErrorOrigin | null;
  toolUse?: ToolUseDiagnostics;
  latencyMs: number;
  ttftMs: number | null;
  generationMs: number | null;
  inputTokens: number | null;
  cachedInputTokens: number | null;
  cacheWriteInputTokens?: number | null;
  cacheWriteTtl?: Exclude<CacheWriteTtl, "provider"> | null;
  reasoningTokens: number | null;
  outputTokens: number | null;
  totalTokens: number | null;
  apiEquivalent?: ApiEquivalentSummary;
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
  requestedReasoningEffort?: ReasoningEffort | null;
  effectiveReasoningEffort?: ReasoningEffort | null;
  connection: string;
  latencyMs: number;
  ttftMs: number | null;
  inputTokens: number | null;
  cachedInputTokens: number | null;
  cacheWriteInputTokens?: number | null;
  cacheWriteTtl?: Exclude<CacheWriteTtl, "provider"> | null;
  reasoningTokens: number | null;
  outputTokens: number | null;
  tokens: number | null;
  requestId: string | null;
  httpStatus: number | null;
  errorCategory: string | null;
  errorOrigin?: ErrorOrigin | null;
  serviceTier?: DefaultServiceTier;
  appliedServiceTier?: DefaultServiceTier | null;
};

export type SupportExportContext = {
  mode: RelayMode;
  schemaVersion: number | null;
  gatewayRunning: boolean;
  sourceCount: number;
  accountCount: number;
  automationCount: number;
  usageCount: number;
  warningCount: number;
};

export type RemoteUsage = {
  id: number;
  requestId: string;
  candidateKind: "account" | "source";
  candidateHint: string;
  candidateLabel?: string | null;
  routing?: RoutingDiagnostics | null;
  requestedModel: string | null;
  resolvedModel: string | null;
  requestedReasoningEffort?: ReasoningEffort | null;
  effectiveReasoningEffort?: ReasoningEffort | null;
  wireApi: "responses" | "chat_completions" | "messages";
  serviceTier?: DefaultServiceTier;
  appliedServiceTier?: DefaultServiceTier | null;
  success: boolean;
  httpStatus: number;
  errorCategory: string | null;
  errorOrigin?: ErrorOrigin | null;
  toolUse?: ToolUseDiagnostics;
  latencyMs: number;
  ttftMs?: number | null;
  generationMs?: number | null;
  inputTokens: number | null;
  cachedInputTokens: number | null;
  cacheWriteInputTokens?: number | null;
  cacheWriteTtl?: Exclude<CacheWriteTtl, "provider"> | null;
  reasoningTokens: number | null;
  outputTokens: number | null;
  totalTokens: number | null;
  apiEquivalent?: ApiEquivalentSummary;
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
    description?: string;
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
    account?: { account: { id: string } };
    error?: { code: string; message: string };
  }>;
};

export type AccountImportProgress = {
  sessionId: string;
  completed: number;
  total: number;
  succeeded: number;
  failed: number;
  currentLabel?: string;
};

export type AccountTransferProgress = {
  completed: number;
  total: number;
  phase: "preparing" | "transferring" | "committing" | "complete";
  currentAccountId?: string;
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
};

export type ProfileSnapshot = {
  id: string;
  name: string;
  profileDir: string;
  createdAtMs: number;
  configAvailable: boolean;
  authAvailable: boolean;
  isOriginal?: boolean;
};

export type ProfileSnapshotList = {
  snapshots: ProfileSnapshot[];
  invalidCount: number;
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
    automationCount: number;
    usageCount: number;
    warningCount: number;
  };
  excluded: string[];
};

export type RelayStorageInfo = {
  dataPath: string;
};
