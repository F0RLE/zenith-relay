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

export type QuotaSnapshot = {
  primary: QuotaWindow | null;
  secondary: QuotaWindow | null;
  resetCreditsAvailable: number | null;
  updatedAtMs: number | null;
  error: { code: string; observedAtMs: number } | null;
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
  lastErrorCode: string | null;
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
  gateway: { running: boolean; baseUrl: string; candidateCount: number; visibleModelIds: string[] };
  platform: string;
  capabilities: { features: string[]; [key: string]: unknown };
  sources: SourceSummary[];
  accounts: AccountSummary[];
  keys: KeySummary[];
  automations: WakeTask[];
  wakeHistory: WakeHistory[];
  warnings: string[];
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
  success: boolean;
  httpStatus: number;
  errorCategory: string | null;
  latencyMs: number;
  inputTokens: number | null;
  outputTokens: number | null;
  totalTokens: number | null;
};

export type RemoteUsage = {
  id: number;
  requestId: string;
  localKeyId: string;
  candidateKind: string;
  candidateHint: string;
  requestedModel: string | null;
  resolvedModel: string | null;
  success: boolean;
  httpStatus: number;
  errorCategory: string | null;
  latencyMs: number;
  inputTokens: number | null;
  outputTokens: number | null;
  totalTokens: number | null;
  createdAtMs: number;
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
      warnings: Array<{ code: string; message: string }>;
      error?: { code: string; message: string };
    }>;
    warnings: Array<{ code: string; message: string }>;
  };
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
