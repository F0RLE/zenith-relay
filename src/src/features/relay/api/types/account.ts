import type { OperationalStatus } from "./common";
import type {
  ApiEquivalentSummary,
  QuotaSnapshot,
  QuotaWindowUsage,
} from "./pricing";
import type { RefreshStatus } from "./source";

export type OAuthClientKind = "codex" | "siwc" | "excel_bps";

export type TrashedAccountSummary = {
  id: string;
  label: string;
  trashedAtMs: number;
};

export type AccountSummary = {
  routeGroupId?: string | null;
  routeInactive?: boolean;
  id: string;
  label: string;
  identityHint: string;
  /** Shared provider credit ledger when reported by the account transport. */
  creditBalanceKey?: string | null;
  /** Older saved accounts use the Codex client. */
  oauthClientKind?: OAuthClientKind;
  basisPointsAvailable?: boolean;
  basisPointsEnabled?: boolean;
  credentialDiagnostics?: {
    status: "pending" | "not_expired" | "expired" | "revoked" | "auth_failed" | "unknown";
    expiresAtMs: number | null;
    hasRefreshToken: boolean;
    recovery: "none" | "refresh_next_request" | "sign_in" | "retry_later";
  } | null;
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
  /** Saved values are not assumed fresh after restart. */
  refreshState?: { models: RefreshStatus; quota: RefreshStatus };
  secretAvailable: boolean;
  recoveryRequired?: boolean;
  proxyMode?: "direct" | "common" | "account";
  proxyAvailable?: boolean;
  proxyId?: string | null;
  routingBlockReason?: "disabled" | "not_in_pool" | "draining" | "secret_unavailable" | "proxy_unavailable" | "reauth_required" | "auth_error" | "checkpoint" | "captcha" | "subscription_forbidden" | "subscription_expired" | "account_unhealthy" | "quota_exhausted" | null;
  lastErrorCode: string | null;
  clientAuthStatus?: "login_required" | "available" | null;
  lastClientLoginRedirectAtMs?: number | null;
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
      oauthClientKind?: OAuthClientKind;
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
    accountId?: string;
    source?: { id: string };
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

export type OAuthFlowStatus = "pending" | "callback_received" | "callback_rejected" | "canceled" | "completed" | "expired" | "failed";

export type OAuthFlowDiagnostic = {
  stage: "callback" | "token_exchange" | "identity_validation" | "saving";
  elapsedMs: number;
  reason?: "callback_invalid" | "token_exchange_failed" | "identity_invalid" | "save_failed" | "storage_unavailable" | "conflict" | "recovery_required";
};

export type OAuthFlow = {
  /** Renderer-only hint; credentials and automation stay in Rust. */
  automaticLogin?: boolean;
  loginId: string;
  authorizationUrl: string;
  redirectUri: string;
  expiresAtMs: number;
  status: OAuthFlowStatus;
  diagnostic?: OAuthFlowDiagnostic | undefined;
  targetAccountId?: string;
  notesAccountId?: string;
  clientKind?: OAuthClientKind;
};

export type OAuthFlowEvent = Pick<OAuthFlow, "loginId" | "status" | "automaticLogin" | "diagnostic">;

export type OAuthCompletion = {
  account: { id: string };
};
