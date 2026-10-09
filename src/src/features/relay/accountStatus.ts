import type { AccountSummary, CandidateRuntimeSnapshot, OperationalStatus } from "./api/types";

const operationalStatusOrder: Record<OperationalStatus, number> = {
  rotation: 0,
  quotaWait: 1,
  unavailable: 2,
  disabled: 3,
};

/** Presentation groups only; dispatch priority stays owned by the scheduler. */
export function compareOperationalStatus(left: OperationalStatus, right: OperationalStatus) {
  return operationalStatusOrder[left] - operationalStatusOrder[right];
}

export function operationalStatusTone(status: OperationalStatus): "ready" | "warning" | "error" | "disabled" {
  if (status === "rotation") return "ready";
  if (status === "quotaWait") return "warning";
  if (status === "unavailable") return "error";
  return "disabled";
}

/** Same icon color as the account card, without live pool-runtime hints. */
export function accountSurfaceTone(account: AccountSummary, onServer = false): "ready" | "warning" | "error" | "info" | "disabled" {
  if (onServer) return "info";
  const quotaStatus = accountQuotaRefreshState(account);
  const displayedError = quotaStatus === "refreshing" ? null : currentAccountErrorCode(account);
  if (displayedError) return "error";
  if (account.operationalStatus === "unavailable" || account.operationalStatus === "disabled") {
    return operationalStatusTone(account.operationalStatus);
  }
  if (quotaStatus === "refreshing" || quotaStatus === "pending") return "disabled";
  if (quotaStatus === "failed" || quotaStatus === "requires_reauth") return "error";
  if (account.clientAuthStatus === "login_required") return "warning";
  return operationalStatusTone(account.operationalStatus);
}

export function transientCandidateTone(
  candidate: CandidateRuntimeSnapshot | undefined,
  nowMs: number,
  includeCooldown: boolean,
): "warning" | "info" | null {
  if (candidate?.halfOpen) return "info";
  if (includeCooldown && candidate?.nextRetryAtMs != null && candidate.nextRetryAtMs > nowMs) {
    return "warning";
  }
  return null;
}

export function isCodexOauthAccountEligible(account: AccountSummary) {
  return account.oauthClientKind !== "excel_bps" && account.inPool && (account.operationalStatus === "rotation" || account.operationalStatus === "quotaWait");
}

export function requiresAccountReauthentication(account: Pick<AccountSummary, "authState" | "routingBlockReason">) {
  return account.routingBlockReason === "reauth_required" || (
    account.authState.state === "requires_reauth"
    && account.authState.reason !== "reused_refresh_token"
  );
}

/**
 * Launching ChatGPT directly needs a usable local credential and a healthy
 * account state. Pool membership and a temporary quota wait do not prevent a
 * direct launch, but terminal account failures do.
 */
export function canLaunchCodexAccount(account: Pick<AccountSummary, "oauthClientKind" | "enabled" | "secretAvailable" | "proxyAvailable" | "authState" | "routingBlockReason" | "clientAuthStatus" | "health">) {
  if (account.oauthClientKind === "excel_bps") return false;
  if (!account.enabled || !account.secretAvailable || account.proxyAvailable === false || requiresAccountReauthentication(account)) return false;
  if (account.clientAuthStatus === "login_required") return false;
  if (account.authState.state === "error" || account.health === "unhealthy" || account.health === "blocked") return false;
  const terminalBlockReasons = new Set([
    "disabled",
    "secret_unavailable",
    "proxy_unavailable",
    "auth_error",
    "checkpoint",
    "captcha",
    "subscription_forbidden",
    "account_unhealthy",
  ]);
  return !terminalBlockReasons.has(account.routingBlockReason ?? "");
}

/**
 * A real sign-in requirement takes precedence over a stale quota refresh
 * result, so every account surface exposes the same available action.
 */
export function accountQuotaRefreshState(account: Pick<AccountSummary, "authState" | "routingBlockReason" | "quota" | "quotaRefreshStatus">): AccountSummary["quotaRefreshStatus"] {
  if (requiresAccountReauthentication(account)) return "requires_reauth";
  return account.quotaRefreshStatus ?? (
    account.quota.error
      ? "failed"
      : account.quota.updatedAtMs != null
        ? "updated"
        : "pending"
  );
}

export function currentAccountErrorCode(account: AccountSummary) {
  if (requiresAccountReauthentication(account)) {
    return account.authState.reason ? `auth_${account.authState.reason}` : "auth_requires_reauth";
  }
  const accountError = account.lastErrorCode?.trim();
  if (accountError && (account.operationalStatus === "unavailable" || accountError.startsWith("models_"))) return accountError;
  const quotaError = account.quota.error?.code.trim();
  if (account.quotaRefreshStatus === "failed" && quotaError) return quotaError;
  if (account.operationalStatus !== "unavailable") return null;
  // Runtime availability can be false for a route, capacity or a protected
  // quota reserve even when the account itself is healthy. Do not invent an
  // account failure when no account-owned error has been observed.
  return accountError || quotaError || (account.routingBlockReason === "subscription_expired" ? null : account.routingBlockReason) || null;
}

export function accountErrorTranslationKey(code: string) {
  const normalized = code.toLowerCase();
  if (normalized === "remote_missing") return "accounts.errors.remoteMissing";
  if (/reused_refresh_token|refresh_token_reused/.test(normalized)) return "accounts.errors.reusedRefreshToken";
  if (/expired_refresh_token|refresh_token_expired/.test(normalized)) return "accounts.errors.expiredRefreshToken";
  if (/invalidated_refresh_token|refresh_token_invalidated|token_invalidated/.test(normalized)) return "accounts.errors.invalidatedRefreshToken";
  if (/invalid_grant/.test(normalized)) return "accounts.errors.invalidGrant";
  if (/invalid_grant|requires_reauth|refresh_token/.test(normalized)) return "accounts.errors.requiresReauth";
  if (/verification|verify.*account|phone/.test(normalized)) return "accounts.errors.verificationRequired";
  if (normalized === "checkpoint" || normalized === "captcha") return "accounts.errors.verificationRequired";
  if (/credential|secret/.test(normalized)) return "accounts.errors.credentialsMissing";
  if (/deactivated|disabled.*workspace|workspace.*(?:disabled|expired|terminated)/.test(normalized)) return "accounts.errors.blocked";
  if (normalized === "upstream_forbidden") return "usage.errorCategories.upstream_forbidden";
  if (normalized === "models_forbidden") return "accounts.importFailureReasons.modelsForbidden";
  const endpointPermission = normalized === "quota_forbidden" || normalized === "subscription_forbidden";
  if (normalized === "subscription_forbidden") return "accounts.errors.blocked";
  if (!endpointPermission && /forbidden|blocked/.test(normalized)) return "accounts.errors.blocked";
  if (/rate.?limit|too_many/.test(normalized)) return "accounts.errors.rateLimited";
  if (normalized === "models_timeout") return "accounts.errors.modelsTimeout";
  if (normalized === "models_transport") return "accounts.errors.modelsConnection";
  if (/timeout/.test(normalized)) return "accounts.errors.connectionTimeout";
  if (/transport|network|connect|proxy/.test(normalized)) return "accounts.errors.connection";
  if (normalized.startsWith("models_")) return "accounts.errors.models";
  if (normalized === "quota_exhausted" || normalized === "upstream_quota_exhausted") return "accounts.errors.quotaExhausted";
  if (/quota/.test(normalized)) return "accounts.errors.quota";
  if (/auth_error|unauthorized|authentication/.test(normalized)) return "accounts.errors.authorization";
  if (/response|parse|decode|malformed/.test(normalized)) return "accounts.errors.invalidResponse";
  return "accounts.errors.unknown";
}
