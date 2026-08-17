import type { AccountSummary, CandidateRuntimeSnapshot, OperationalStatus } from "./api/types";

export function operationalStatusTone(status: OperationalStatus): "ready" | "warning" | "error" | "disabled" {
  if (status === "rotation") return "ready";
  if (status === "quotaWait") return "warning";
  if (status === "unavailable") return "error";
  return "disabled";
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
  return account.inPool && (account.operationalStatus === "rotation" || account.operationalStatus === "quotaWait");
}

export function currentAccountErrorCode(account: AccountSummary) {
  const quotaError = account.quota.error?.code.trim();
  if (account.quotaRefreshStatus === "failed" && quotaError) return quotaError;
  if (account.routingBlockReason === "reauth_required" || account.authState.state === "requires_reauth") {
    return account.authState.reason ? `auth_${account.authState.reason}` : "auth_requires_reauth";
  }
  if (account.operationalStatus !== "unavailable") return null;
  return account.lastErrorCode?.trim() || quotaError || account.routingBlockReason || "account_unavailable";
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
  if (/credential|secret/.test(normalized)) return "accounts.errors.credentialsMissing";
  if (/deactivated|disabled.*workspace|workspace.*(?:disabled|expired|terminated)/.test(normalized)) return "accounts.errors.blocked";
  if (/forbidden|blocked/.test(normalized)) return "accounts.errors.blocked";
  if (/rate.?limit|too_many/.test(normalized)) return "accounts.errors.rateLimited";
  if (/transport|timeout|network|connect/.test(normalized)) return "accounts.errors.connection";
  if (normalized === "quota_exhausted" || normalized === "upstream_quota_exhausted") return "accounts.errors.quotaExhausted";
  if (/quota/.test(normalized)) return "accounts.errors.quota";
  if (/auth_error|unauthorized|authentication/.test(normalized)) return "accounts.errors.authorization";
  if (/response|parse|decode|malformed/.test(normalized)) return "accounts.errors.invalidResponse";
  return "accounts.errors.unknown";
}
