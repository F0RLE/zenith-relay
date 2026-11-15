import type { AccountSummary } from "../../api/types";

/** A saved error is not evidence that a request used today's proxy settings. */
export function accountErrorContext(account: AccountSummary, code: string) {
  const quotaError = account.quota.error;
  const matchesQuota = quotaError?.code.trim() === code;
  const observedAtMs = matchesQuota ? quotaError?.occurredAtMs : null;
  const observedAt = typeof observedAtMs === "number" ? new Date(observedAtMs) : null;
  return {
    observed_at: observedAt && Number.isFinite(observedAt.getTime()) ? observedAt.toISOString() : null,
    error_scope: code.startsWith("models_") ? "models" : matchesQuota ? "quota" : "account",
    // Neither saved account errors nor quota errors retain their request route.
    failed_request_route: null,
    current_route: {
      proxy_mode: account.proxyMode ?? null,
      configuration_valid: account.proxyAvailable ?? null,
    },
    model_catalog_status: account.refreshState?.models ?? "unknown",
  };
}
