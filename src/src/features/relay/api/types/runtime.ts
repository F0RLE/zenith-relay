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

export type CodexMemoryStage = "extraction" | "consolidation";

export type ModelCatalogIdentity = Pick<ModelSummary, "catalogProvider" | "catalogFamily"> & {
  /** Position across complete member inventories, including saved rules and prices. */
  catalogOrder?: number | null;
};

export type RuntimeSnapshot = {
  schemaVersion: number;
  gateway: {
    toolPolicy?: ToolPolicy;
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
    accountConnectors?: Array<{
      id: string;
      providerFamily: "openai" | "anthropic" | "google" | "moonshot" | "xai" | "meta" | "cognition";
      oauthClientKind?: "codex" | "siwc" | "excel_bps";
      signInFlow: "authorization_code_pkce" | "dynamic_client_pkce" | "device_authorization";
    }>;
  };
  sources: SourceSummary[];
  accounts: AccountSummary[];
  automations: WakeTask[];
  wakeHistory: WakeHistory[];
  warnings: string[];
  pricing?: PricingMetadata;
};
