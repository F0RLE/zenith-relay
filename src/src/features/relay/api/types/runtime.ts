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

export type AccountTimezoneSource = "account" | "proxy";
export type CodexMemoryStage = "extraction" | "consolidation";

export type RequestTimezonePolicy = {
  enabled: boolean;
  accountTimezones: Record<string, string>;
  /** Missing entries inherit the global setting. */
  accountTimezoneBindings: Record<string, boolean>;
  /** Missing entries preserve legacy account timezone behavior. */
  accountTimezoneSources: Record<string, AccountTimezoneSource>;
};

export type RequestTimezoneUpdate = {
  policy: RequestTimezonePolicy;
  expectedPolicy: RequestTimezonePolicy;
};

export type ModelCatalogIdentity = Pick<ModelSummary, "catalogProvider" | "catalogFamily"> & {
  /** Position across complete member inventories, including saved rules and prices. */
  catalogOrder?: number | null;
};

export type RuntimeSnapshot = {
  schemaVersion: number;
  gateway: {
    toolPolicy?: ToolPolicy;
    requestTimezone?: RequestTimezonePolicy;
    poolRouting?: PoolRoutingSnapshot;
    running: boolean;
    baseUrl: string;
    candidateCount: number;
    visibleModelIds: string[];
    maxRetryCandidates: number;
    defaultServiceTier: DefaultServiceTier;
    models?: ModelSummary[];
    /** Display metadata for the complete inventory, independent of routing rules. */
    modelCatalog?: Record<string, ModelCatalogIdentity>;
    commonProxyConfigured?: boolean;
    commonProxyAvailable?: boolean;
    commonProxyId?: string | null;
    accountProxyRequired?: boolean;
    quotaRequestTimeoutSeconds?: number;
    chatgptInterfaceQuotaReserveBasisPoints?: number;
    codexBackgroundTasksEnabled?: boolean;
    codexMemoryExtractionEnabled?: boolean;
    codexMemoryConsolidationEnabled?: boolean;
    codexWebsocketsEnabled?: boolean;
    chatgptRetryUntilAvailable?: boolean;
    blockDegradedRoutesEnabled?: boolean;
    routingOrder?: CandidateRuntimeSnapshot[];
  };
  platform: string;
  capabilities: {
    features: string[];
  };
  sources: SourceSummary[];
  accounts: AccountSummary[];
  automations: WakeTask[];
  wakeHistory: WakeHistory[];
  warnings: string[];
  pricing?: PricingMetadata;
};
