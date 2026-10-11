import type { DefaultServiceTier, ToolPolicy } from "./common";
import type { PricingMetadata } from "./pricing";
import type { SourceSummary } from "./source";
import type { AccountSummary } from "./account";
import type { ModelSummary } from "./model";
import type {
  CandidateRuntimeSnapshot,
  WakeHistory,
  WakeTask,
} from "./activity";
import type { PoolRoutingSnapshot } from "./pool";

export type RequestTimezonePolicy = {
  enabled: boolean;
  accountTimezones: Record<string, string>;
};

export type RequestTimezoneUpdate = {
  policy: RequestTimezonePolicy;
  expectedPolicy: RequestTimezonePolicy;
};

export type RuntimeSnapshot = {
  schemaVersion: number;
  configurationRevision?: string | null;
  runtimeTarget: { kind: "local" | "remote"; connected: boolean; origin: string | null; serverId: string | null; version: string | null };
  gateway: {
    toolPolicy?: ToolPolicy;
    requestTimezone?: RequestTimezonePolicy;
    basisPointsEnabled?: boolean;
    poolRouting?: PoolRoutingSnapshot;
    running: boolean;
    baseUrl: string;
    candidateCount: number;
    visibleModelIds: string[];
    maxRetryCandidates: number;
    defaultServiceTier: DefaultServiceTier;
    models?: ModelSummary[];
    /** Display metadata for the complete inventory, independent of routing rules. */
    modelCatalog?: Record<string, Pick<ModelSummary, "catalogProvider" | "catalogFamily">>;
    commonProxyConfigured?: boolean;
    commonProxyAvailable?: boolean;
    commonProxyId?: string | null;
    accountProxyRequired?: boolean;
    quotaRequestTimeoutSeconds?: number;
    chatgptInterfaceQuotaReserveBasisPoints?: number;
    codexBackgroundTasksEnabled?: boolean;
    codexWebsocketsEnabled?: boolean;
    chatgptContextPolicy?: ApplicationContextPolicy;
    chatgptRetryUntilAvailable?: boolean;
    blockDegradedRoutesEnabled?: boolean;
    routingOrder?: CandidateRuntimeSnapshot[];
  };
  platform: string;
  capabilities: {
    features: string[];
    supportedWireApis?: Array<"responses" | "chat_completions" | "messages" | "gemini">;
    [key: string]: unknown;
  };
  sources: SourceSummary[];
  accounts: AccountSummary[];
  automations: WakeTask[];
  wakeHistory: WakeHistory[];
  warnings: string[];
  pricing?: PricingMetadata;
};

export type ContextWindowPolicy =
  | { mode: "client" | "short" | "extended" }
  | { mode: "custom"; tokens: number };

export type CompactionPolicy =
  | { mode: "client" | "early" }
  | { mode: "custom"; tokens: number };

export type ApplicationContextPolicy = ContextWindowPolicy
  | { mode: "configured"; context: ContextWindowPolicy; compaction: CompactionPolicy };
