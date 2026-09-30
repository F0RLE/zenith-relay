export type CandidateRuntimeSnapshot = {
  candidateId: string;
  kind: "api_source" | "oauth_account";
  available: boolean;
  nextForNewRequest?: boolean;
  activityRevision?: number;
  runtimeId?: number;
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

export type RuntimeActivitySnapshot = {
  runtimeId?: number;
  revision: number;
  candidateId: string;
  memberKey?: string;
  inFlight: number;
  activeRequestCount: number;
  activeModels: Array<{
    model: string;
    requestCount: number;
  }>;
};

/**
 * Ephemeral routing facts maintained by the local runtime event stream.
 *
 * The persisted runtime snapshot is still the source of truth for policy and
 * availability. This small overlay bridges the interval between a
 * reserve/release event and the next snapshot refresh; its latest release
 * marker also prevents a stale poll from bringing a completed route back.
 */
export type RuntimeActivityState = {
  runtimeId?: number;
  revision: number;
  lastCandidateId: string | null;
  /**
   * The latest event for every touched candidate. The runtime order is
   * eventually consistent, so the pool needs these facts while its next
   * snapshot is still in flight.
   */
  candidates: Readonly<Record<string, RuntimeActivitySnapshot>>;
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
