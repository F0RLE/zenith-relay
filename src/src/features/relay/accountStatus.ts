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

/** Runtime waits narrow readiness without hiding account-owned blocking states. */
export function accountDisplayStatus(account: AccountSummary, candidate?: CandidateRuntimeSnapshot): OperationalStatus | "cooldown" | "busy" | "draining" {
  if (!account.enabled) return "disabled";
  if (account.draining) return "draining";
  if (account.operationalStatus === "disabled") return "disabled";
  if (!account.secretAvailable || account.proxyAvailable === false || account.recoveryRequired
    || requiresAccountReauthentication(account) || account.authState.state === "error"
    || account.health === "unhealthy" || account.health === "blocked") return "unavailable";
  if (account.routingBlockReason && account.routingBlockReason !== "quota_exhausted"
    && account.routingBlockReason !== "not_in_pool") return "unavailable";
  if (candidate?.available === false) {
    if (candidate.quotaWait) return "quotaWait";
    if (candidate.nextRetryAtMs != null) return "cooldown";
    if (candidate.capacityWait) return "busy";
  }
  return account.operationalStatus;
}

/** Shared account indicator; monitoring errors never override routing blocks. */
export function accountSurfaceTone(account: AccountSummary, recoveryRequired = Boolean(account.recoveryRequired), candidate?: CandidateRuntimeSnapshot): "ready" | "warning" | "error" | "info" | "disabled" {
  if (recoveryRequired) return "info";
  const status = accountDisplayStatus(account, candidate);
  if (status === "disabled" || status === "draining") return "disabled";
  if (status === "unavailable") return "error";
  if (status === "cooldown" || status === "busy") return "warning";
  const quotaStatus = accountQuotaRefreshState(account);
  const displayedError = quotaStatus === "refreshing" ? null : currentAccountErrorCode(account);
  if (displayedError) {
    // Monitoring is separate from admission: a failed quota/model read does
    // not prove the account cannot execute. Actual routing blocks take priority.
    const monitoringError = displayedError.startsWith("models_")
      || (quotaStatus === "failed" && displayedError === account.quota.error?.code.trim());
    return monitoringError ? "warning" : "error";
  }
  if (quotaStatus === "refreshing" || quotaStatus === "pending") return "disabled";
  if (quotaStatus === "requires_reauth") return "error";
  if (quotaStatus === "failed") return "warning";
  if (account.clientAuthStatus === "login_required") return "warning";
  return operationalStatusTone(status);
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
  return account.oauthClientKind !== "excel_bps" && !account.recoveryRequired && !account.draining
    && canLaunchCodexAccount(account) && account.authState.state === "active";
}

function requiresAccountReauthentication(account: Pick<AccountSummary, "authState" | "routingBlockReason">) {
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
export function canLaunchCodexAccount(account: Pick<AccountSummary, "oauthClientKind" | "enabled" | "secretAvailable" | "proxyAvailable" | "authState" | "routingBlockReason" | "clientAuthStatus" | "health" | "recoveryRequired">) {
  if (account.recoveryRequired) return false;
  if (account.oauthClientKind === "excel_bps" || account.oauthClientKind === "siwc") return false;
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

function hasOwnQuotaMonitoring(account: Pick<AccountSummary, "oauthClientKind" | "quota">) {
  return account.oauthClientKind !== "siwc" || Boolean(account.quota.primary || account.quota.secondary
    || account.quota.supplemental?.length || account.quota.limitReached);
}

/** A selected route's sign-in requirement always takes precedence over monitoring. */
export function accountQuotaRefreshState(account: Pick<AccountSummary, "oauthClientKind" | "authState" | "routingBlockReason" | "quota" | "quotaRefreshStatus">): AccountSummary["quotaRefreshStatus"] | "not_available" {
  if (requiresAccountReauthentication(account)) return "requires_reauth";
  // SIWC executes requests but does not expose a quota-polling endpoint.
  // Linked counters retain their own reader/freshness in AccountQuotaPanel.
  if (!hasOwnQuotaMonitoring(account)) return "not_available";
  const status = account.quotaRefreshStatus ?? (
    account.quota.error
      ? "failed"
      : account.quota.updatedAtMs != null
        ? "updated"
        : "pending"
  );
  return status;
}

export function currentAccountErrorCode(account: AccountSummary) {
  if (requiresAccountReauthentication(account)) {
    return account.authState.reason ? `auth_${account.authState.reason}` : "auth_requires_reauth";
  }
  const savedQuotaError = account.quota.error?.code.trim();
  const unsupportedQuotaError = !hasOwnQuotaMonitoring(account) && savedQuotaError?.startsWith("quota_");
  const quotaError = unsupportedQuotaError ? undefined : savedQuotaError;
  const savedAccountError = account.lastErrorCode?.trim();
  const accountError = unsupportedQuotaError && savedAccountError === savedQuotaError ? undefined : savedAccountError;
  if (accountError && (account.operationalStatus === "unavailable" || accountError.startsWith("models_") || accountError === "siwc_plan_use_disabled")) return accountError;
  if (account.quotaRefreshStatus === "failed" && quotaError) return quotaError;
  if (account.operationalStatus !== "unavailable") return null;
  // Runtime availability can be false for a route, capacity or a protected
  // quota reserve even when the account itself is healthy. Do not invent an
  // account failure when no account-owned error has been observed.
  return accountError || quotaError || (account.routingBlockReason === "subscription_expired" ? null : account.routingBlockReason) || null;
}

export function accountErrorTranslationKey(code: string) {
  const normalized = code.toLowerCase();
  if (normalized === "siwc_rotation_store_unavailable" || normalized === "account_credential_store_unavailable") return "accounts.errors.rotationStorageUnavailable";
  if (/token_rotation_recovery_required|account_rotation_recovery_required|siwc_rotation_(outcome_unknown|checkpoint_invalid|recovery_required)/.test(normalized)) return "accounts.errors.tokenRotationRecovery";
  if (normalized === "siwc_plan_use_disabled") return "accounts.errors.siwcPlanUseDisabled";
  if (normalized === "basis_points_free_account_unsupported") return "accounts.errors.basisPointsFreeUnsupported";
  if (/reused_refresh_token|refresh_token_reused/.test(normalized)) return "accounts.errors.reusedRefreshToken";
  if (/expired_refresh_token|refresh_token_expired/.test(normalized)) return "accounts.errors.expiredRefreshToken";
  if (/invalidated_refresh_token|refresh_token_invalidated|token_invalidated/.test(normalized)) return "accounts.errors.invalidatedRefreshToken";
  if (/invalid_grant/.test(normalized)) return "accounts.errors.invalidGrant";
  if (/requires_reauth|refresh_token/.test(normalized)) return "accounts.errors.requiresReauth";
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
  if (normalized === "quota_timeout") return "accounts.errors.quotaTimeout";
  if (normalized === "quota_transport") return "accounts.errors.quotaConnection";
  if (/timeout/.test(normalized)) return "accounts.errors.connectionTimeout";
  if (/transport|network|connect|proxy/.test(normalized)) return "accounts.errors.connection";
  if (normalized.startsWith("models_")) return "accounts.errors.models";
  if (normalized === "quota_exhausted" || normalized === "upstream_quota_exhausted") return "accounts.errors.quotaExhausted";
  if (/quota/.test(normalized)) return "accounts.errors.quota";
  if (/auth_error|unauthorized|authentication/.test(normalized)) return "accounts.errors.authorization";
  if (/response|parse|decode|malformed/.test(normalized)) return "accounts.errors.invalidResponse";
  return "accounts.errors.unknown";
}
