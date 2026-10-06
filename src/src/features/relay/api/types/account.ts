import type { OperationalStatus } from "./common";
import type {
  ApiEquivalentSummary,
  QuotaSnapshot,
  QuotaWindowUsage,
} from "./pricing";
import type { RefreshStatus } from "./source";

export type AccountSummary = {
  id: string;
  label: string;
  identityHint: string;
  basisPointsAvailable?: boolean;
  basisPointsEnabled?: boolean;
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
  quotaWindowUsage?: QuotaWindowUsage | null;
  purchaseCostMicroUsd?: number | null;
  subscription: { planType: string | null; activeUntilMs: number | null; status: string; updatedAtMs: number | null };
  quota: QuotaSnapshot;
  quotaRefreshStatus: "pending" | "refreshing" | "updated" | "failed" | "requires_reauth";
  /** Optional on older servers; saved values are not assumed fresh after restart. */
  refreshState?: { models: RefreshStatus; quota: RefreshStatus };
  secretAvailable: boolean;
  remoteLocation?: { serverId: string; remoteAccountId: string } | null;
  proxyMode?: "direct" | "common" | "account";
  proxyAvailable?: boolean;
  proxyId?: string | null;
  routingBlockReason?: "disabled" | "not_in_pool" | "draining" | "secret_unavailable" | "proxy_unavailable" | "reauth_required" | "auth_error" | "checkpoint" | "captcha" | "subscription_forbidden" | "subscription_expired" | "account_unhealthy" | "quota_exhausted" | null;
  lastErrorCode: string | null;
  clientAuthStatus?: "login_required" | "available" | null;
  lastClientLoginRedirectAtMs?: number | null;
};

export type CredentialRefreshResult = {
  accountId: string;
  status: "refreshed" | "retryable_failure" | "requires_reauth";
  code: string;
  expiresAtMs?: number | null;
  generation?: number | null;
};

export type RevealedAccountIdentity = {
  accountId: string;
  identity: string;
};

export type AccountLoginDetails = {
  accountId: string;
  email: string | null;
  phone: string | null;
  password: string | null;
  totpSecret: string | null;
  totpCode: string | null;
  totpExpiresAtMs: number | null;
};

export type AccountLoginUpdate = {
  accountId: string;
  email: string;
  phone: string;
  password: string;
  totpSecret: string;
};

export type AccountTotpPreview = {
  code: string | null;
  expiresAtMs: number | null;
};

export type AccountExportFormat = "zenith" | "cpa" | "sub2api" | "9router" | "codex" | "axon_hub" | "codex_manager";

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
  targetAccountId?: string;
};

export type OAuthFlowEvent = Pick<OAuthFlow, "loginId" | "status">;

export type OAuthCompletion = {
  account: { id: string };
};
