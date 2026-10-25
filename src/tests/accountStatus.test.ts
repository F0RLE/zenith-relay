import { describe, expect, test } from "bun:test";
import type { AccountSummary, CandidateRuntimeSnapshot } from "../src/features/relay/api/types";
import { accountErrorContext } from "../src/features/relay/pages/connections/accountErrorContext";
import {
  accountQuotaRefreshState,
  accountErrorTranslationKey,
  accountSurfaceTone,
  canLaunchCodexAccount,
  currentAccountErrorCode,
  isCodexOauthAccountEligible,
  operationalStatusTone,
  transientCandidateTone,
} from "../src/features/relay/accountStatus";

test("local recovery blocks direct launch even with apparently usable credentials", () => {
  const pending = account({ recoveryRequired: true });
  expect(canLaunchCodexAccount(pending)).toBe(false);
  expect(isCodexOauthAccountEligible(pending)).toBe(false);
  expect(accountSurfaceTone(pending)).toBe("info");
});

test("SIWC uses the pool rather than native credential injection and explains missing consent", () => {
  const siwc = account({ oauthClientKind: "siwc", lastErrorCode: "siwc_plan_use_disabled" });
  expect(canLaunchCodexAccount(siwc)).toBe(false);
  expect(isCodexOauthAccountEligible(siwc)).toBe(false);
  expect(currentAccountErrorCode(siwc)).toBe("siwc_plan_use_disabled");
  expect(accountErrorTranslationKey("siwc_plan_use_disabled")).toBe("accounts.errors.siwcPlanUseDisabled");
});

function account(overrides: Partial<AccountSummary> = {}): AccountSummary {
  return {
    id: "account",
    label: "Account",
    identityHint: "Account",
    enabled: true,
    inPool: true,
    draining: false,
    authState: { state: "active" },
    health: "healthy",
    operationalStatus: "rotation",
    models: [],
    allowedModels: [],
    excludedModels: [],
    priority: 1,
    weight: 1,
    apiEquivalent: { microUsd: 0, unpricedTokens: 0 },
    subscription: { planType: null, activeUntilMs: null, status: "active", updatedAtMs: null },
    quota: {},
    quotaRefreshStatus: "updated",
    secretAvailable: true,
    lastErrorCode: null,
    ...overrides,
  };
}

test("saved model errors never borrow the current proxy or a quota timestamp", () => {
  const context = accountErrorContext(account({
    proxyMode: "account",
    proxyAvailable: true,
    lastErrorCode: "models_forbidden",
    quota: { error: { code: "quota_timeout", occurredAtMs: 123 } },
    refreshState: { models: "stale", quota: "fresh" },
  }), "models_forbidden");
  expect(context).toEqual({
    observed_at: null,
    error_scope: "models",
    failed_request_route: null,
    current_route: { proxy_mode: "account", configuration_valid: true },
    model_catalog_status: "stale",
  });
});

test("quota errors keep only their matching timestamp and no invented route evidence", () => {
  const value = account({ quota: { error: { code: "quota_timeout", occurredAtMs: 123 } } });
  expect(accountErrorContext(value, "quota_timeout").observed_at).toBe("1970-01-01T00:00:00.123Z");
  expect(accountErrorContext(value, "quota_timeout").failed_request_route).toBeNull();
  expect(accountErrorContext(value, "models_forbidden").observed_at).toBeNull();
  value.quota.error!.occurredAtMs = Number.NaN;
  expect(accountErrorContext(value, "quota_timeout").observed_at).toBeNull();
});

test("SIWC without its own counters does not expose a quota-polling state", () => {
  const siwc = account({ oauthClientKind: "siwc", quotaRefreshStatus: "pending" });
  expect(accountQuotaRefreshState(siwc)).toBe("not_available");
  expect(accountSurfaceTone(siwc)).toBe("ready");
  expect(accountQuotaRefreshState({ ...siwc, quotaRefreshStatus: "updated" })).toBe("not_available");
  expect(accountQuotaRefreshState({ ...siwc, quotaRefreshStatus: "refreshing" })).toBe("not_available");
  expect(accountQuotaRefreshState({ ...siwc, quotaRefreshStatus: "failed" })).toBe("not_available");
  expect(accountQuotaRefreshState({
    ...siwc, quotaRefreshStatus: "updated", quota: {
      ...siwc.quota,
      primary: { kind: "primary", availableBasisPoints: 7500, explicitlyFull: false, resetAtMs: 5000, windowMinutes: 300, observedAtMs: 1 },
    },
  })).toBe("updated");
  expect(accountQuotaRefreshState({ ...siwc, authState: { state: "requires_reauth", reason: "invalid_grant" } })).toBe("requires_reauth");
  expect(accountQuotaRefreshState({ ...siwc, oauthClientKind: "codex" })).toBe("pending");
});

test("SIWC ignores obsolete quota polling errors without hiding route failures", () => {
  const siwc = account({
    oauthClientKind: "siwc", health: "degraded", quotaRefreshStatus: "failed",
    quota: { error: { code: "quota_prepare", occurredAtMs: 123 } }, lastErrorCode: "quota_prepare",
  });
  expect(accountQuotaRefreshState(siwc)).toBe("not_available");
  expect(currentAccountErrorCode(siwc)).toBeNull();
  expect(accountSurfaceTone(siwc)).toBe("ready");
  expect(currentAccountErrorCode({ ...siwc, lastErrorCode: "models_transport" })).toBe("models_transport");
  expect(accountSurfaceTone({ ...siwc, lastErrorCode: "models_transport" })).toBe("warning");
  expect(currentAccountErrorCode({ ...siwc, operationalStatus: "unavailable", lastErrorCode: "upstream_transport_connect" })).toBe("upstream_transport_connect");
  expect(accountSurfaceTone({ ...siwc, secretAvailable: false })).toBe("error");
  expect(currentAccountErrorCode({ ...siwc, authState: { state: "requires_reauth", reason: "invalid_grant" } })).toBe("auth_invalid_grant");
  expect(accountQuotaRefreshState({ ...siwc, quota: { ...siwc.quota, limitReached: true } })).toBe("failed");
  expect(currentAccountErrorCode({ ...siwc, oauthClientKind: "codex" })).toBe("quota_prepare");
});

describe("account status policy", () => {
  test("rotation recovery requires sign-in without mislabeling temporary storage outages", () => {
    const recovery = account({ authState: { state: "requires_reauth", reason: "token_rotation_recovery_required" } });
    expect(accountQuotaRefreshState(recovery)).toBe("requires_reauth");
    expect(currentAccountErrorCode(recovery)).toBe("auth_token_rotation_recovery_required");
    for (const code of ["auth_token_rotation_recovery_required", "account_rotation_recovery_required", "siwc_rotation_outcome_unknown", "siwc_rotation_checkpoint_invalid", "siwc_rotation_recovery_required"]) {
      expect(accountErrorTranslationKey(code)).toBe("accounts.errors.tokenRotationRecovery");
    }
    expect(accountErrorTranslationKey("siwc_rotation_store_unavailable")).toBe("accounts.errors.rotationStorageUnavailable");
    expect(accountErrorTranslationKey("account_credential_store_unavailable")).toBe("accounts.errors.rotationStorageUnavailable");
  });
  test("SIWC quota wait is a warning without inventing missing quota counters", () => {
    const waiting = account({ oauthClientKind: "siwc", operationalStatus: "quotaWait", quotaRefreshStatus: "pending" });
    expect(accountSurfaceTone(waiting)).toBe("warning");
    expect(currentAccountErrorCode(waiting)).toBeNull();
    expect(accountQuotaRefreshState(waiting)).toBe("not_available");
    expect(waiting.quota.primary).toBeUndefined();
  });

  test("maps operational and transient scheduler state without treating cooldown as failure", () => {
    expect(operationalStatusTone("rotation")).toBe("ready");
    expect(operationalStatusTone("quotaWait")).toBe("warning");
    expect(operationalStatusTone("unavailable")).toBe("error");
    expect(transientCandidateTone({ halfOpen: true } as CandidateRuntimeSnapshot, 100, true)).toBe("info");
    expect(transientCandidateTone({ nextRetryAtMs: 101 } as CandidateRuntimeSnapshot, 100, true)).toBe("warning");
    expect(transientCandidateTone({ nextRetryAtMs: 101 } as CandidateRuntimeSnapshot, 100, false)).toBeNull();
  });

  test("a connection cooldown is temporary and never hides an account block", () => {
    const waiting = account({ operationalStatus: "unavailable", lastErrorCode: "upstream_transport_connect" });
    const candidate = { available: false, nextRetryAtMs: 200 } as CandidateRuntimeSnapshot;
    expect(accountSurfaceTone(waiting, false, candidate)).toBe("warning");
    expect(accountSurfaceTone({ ...waiting, proxyAvailable: false }, false, candidate)).toBe("error");
    expect(accountSurfaceTone({ ...waiting, authState: { state: "requires_reauth", reason: "invalid_grant" } }, false, candidate)).toBe("error");
    expect(accountSurfaceTone({ ...waiting, health: "blocked" }, false, candidate)).toBe("error");
    expect(accountSurfaceTone({ ...waiting, health: "unhealthy" }, false, candidate)).toBe("error");
    expect(accountSurfaceTone({ ...waiting, routingBlockReason: "account_unhealthy" }, false, candidate)).toBe("error");
    expect(accountSurfaceTone(account(), false, { ...candidate, available: true, nextRetryAtMs: null })).toBe("ready");
  });

  test("keeps account eligibility and error precedence explicit", () => {
    expect(isCodexOauthAccountEligible(account({ operationalStatus: "quotaWait" }))).toBeTrue();
    expect(isCodexOauthAccountEligible(account({ inPool: false }))).toBeTrue();
    expect(isCodexOauthAccountEligible(account({ oauthClientKind: "excel_bps" }))).toBeFalse();
    expect(isCodexOauthAccountEligible(account({ draining: true }))).toBeFalse();
    expect(accountErrorTranslationKey("basis_points_free_account_unsupported")).toBe("accounts.errors.basisPointsFreeUnsupported");
    expect(currentAccountErrorCode(account({ operationalStatus: "quotaWait", routingBlockReason: "quota_exhausted" }))).toBeNull();
    expect(currentAccountErrorCode(account({ quotaRefreshStatus: "failed", quota: { error: { code: "quota_timeout" } } }))).toBe("quota_timeout");
    expect(currentAccountErrorCode(account({ authState: { state: "requires_reauth", reason: "expired" } }))).toBe("auth_expired");
    expect(currentAccountErrorCode(account({ authState: { state: "requires_reauth", reason: "expired" }, lastErrorCode: "models_prepare" }))).toBe("auth_expired");
    expect(currentAccountErrorCode(account({ operationalStatus: "unavailable", lastErrorCode: "provider_timeout" }))).toBe("provider_timeout");
    expect(currentAccountErrorCode(account({ operationalStatus: "unavailable", authState: { state: "active" }, health: "healthy" }))).toBeNull();
  });

  test("does not let a stale quota result hide a required sign-in", () => {
    expect(accountQuotaRefreshState(account({
      quotaRefreshStatus: "updated",
      authState: { state: "requires_reauth", reason: "expired_refresh_token" },
    }))).toBe("requires_reauth");
    expect(accountQuotaRefreshState(account({
      quotaRefreshStatus: "updated",
      authState: { state: "requires_reauth", reason: "reused_refresh_token" },
    }))).toBe("updated");
  });

  test("disables direct ChatGPT launch for terminal account states", () => {
    expect(canLaunchCodexAccount(account())).toBeTrue();
    expect(canLaunchCodexAccount(account({
      authState: { state: "requires_reauth", reason: "invalid_grant" },
      routingBlockReason: "reauth_required",
    }))).toBeFalse();
    expect(canLaunchCodexAccount(account({ enabled: false, routingBlockReason: "disabled" }))).toBeFalse();
    expect(canLaunchCodexAccount(account({ proxyAvailable: false, routingBlockReason: "proxy_unavailable" }))).toBeFalse();
    expect(canLaunchCodexAccount(account({ clientAuthStatus: "login_required" }))).toBeFalse();
    expect(canLaunchCodexAccount(account({ health: "unhealthy", routingBlockReason: "account_unhealthy" }))).toBeFalse();
    expect(canLaunchCodexAccount(account({ inPool: false, routingBlockReason: "not_in_pool" }))).toBeTrue();
  });

  test("does not surface an expired subscription as a generic account error", () => {
    expect(currentAccountErrorCode(account({
      operationalStatus: "unavailable",
      routingBlockReason: "subscription_expired",
      subscription: { planType: "plus", activeUntilMs: 1, status: "expired", updatedAtMs: 2 },
    }))).toBeNull();
  });

  test("keeps the account failure visible when quota monitoring also fails", () => {
    for (const lastErrorCode of ["workspace_disabled", "upstream_unauthorized", "checkpoint", "captcha"]) {
      const unavailable = account({
        operationalStatus: "unavailable",
        lastErrorCode,
        quotaRefreshStatus: "failed",
        quota: { error: { code: "quota_timeout" } },
      });
      expect(currentAccountErrorCode(unavailable)).toBe(lastErrorCode);
      expect(accountQuotaRefreshState(unavailable)).toBe("failed");
      expect(accountSurfaceTone(unavailable)).toBe("error");
    }
  });

  test("monitoring failures warn without presenting a routable account as broken", () => {
    for (const code of ["quota_timeout", "quota_transport"]) {
      const failed = account({ quotaRefreshStatus: "failed", quota: { error: { code } } });
      expect(currentAccountErrorCode(failed)).toBe(code);
      expect(accountSurfaceTone(failed)).toBe("warning");
      expect(canLaunchCodexAccount(failed)).toBeTrue();
      expect(accountSurfaceTone({ ...failed, operationalStatus: "unavailable", routingBlockReason: "proxy_unavailable" })).toBe("error");
      expect(accountSurfaceTone({ ...failed, authState: { state: "requires_reauth", reason: "invalid_grant" } })).toBe("error");
      expect(accountSurfaceTone({ ...failed, enabled: false, operationalStatus: "disabled" })).toBe("disabled");
      expect(accountSurfaceTone({ ...failed, quotaRefreshStatus: "updated", quota: {} })).toBe("ready");
    }
    for (const lastErrorCode of ["models_timeout", "models_transport"]) {
      expect(accountSurfaceTone(account({ lastErrorCode }))).toBe("warning");
      expect(accountSurfaceTone(account({ lastErrorCode, operationalStatus: "unavailable" }))).toBe("error");
    }
    expect(accountSurfaceTone(account({ lastErrorCode: "siwc_plan_use_disabled" }))).toBe("error");
  });

  test("maps safe error codes to stable translation keys", () => {
    expect(accountErrorTranslationKey("HTTP 429 rate-limit")).toBe("accounts.errors.rateLimited");
    expect(accountErrorTranslationKey("INVALID_GRANT")).toBe("accounts.errors.invalidGrant");
    expect(accountErrorTranslationKey("quota_exhausted")).toBe("accounts.errors.quotaExhausted");
    expect(accountErrorTranslationKey("unknown_provider_problem")).toBe("accounts.errors.unknown");
    expect(accountErrorTranslationKey("upstream_forbidden")).toBe("usage.errorCategories.upstream_forbidden");
    expect(accountErrorTranslationKey("models_forbidden")).toBe("accounts.importFailureReasons.modelsForbidden");
    expect(accountErrorTranslationKey("models_prepare")).toBe("accounts.errors.models");
    expect(accountErrorTranslationKey("models_transport")).toBe("accounts.errors.modelsConnection");
    expect(accountErrorTranslationKey("quota_timeout")).toBe("accounts.errors.quotaTimeout");
    expect(accountErrorTranslationKey("quota_transport")).toBe("accounts.errors.quotaConnection");
    expect(accountErrorTranslationKey("proxy_unavailable")).toBe("accounts.errors.connection");
    expect(accountErrorTranslationKey("quota_forbidden")).toBe("accounts.errors.quota");
    expect(accountErrorTranslationKey("subscription_forbidden")).toBe("accounts.errors.blocked");
    expect(accountErrorTranslationKey("checkpoint")).toBe("accounts.errors.verificationRequired");
    expect(accountErrorTranslationKey("deactivated_workspace")).toBe("accounts.errors.blocked");
    expect(accountErrorTranslationKey("account_blocked")).toBe("accounts.errors.blocked");
  });

  test("uses the same surface color outside the pool as on the account card", () => {
    expect(accountSurfaceTone(account({ inPool: false, operationalStatus: "rotation" }))).toBe("ready");
    expect(accountSurfaceTone(account({ inPool: false, operationalStatus: "quotaWait" }))).toBe("warning");
    expect(accountSurfaceTone(account({ inPool: false, operationalStatus: "disabled" }))).toBe("disabled");
    expect(accountSurfaceTone(account({ inPool: false, operationalStatus: "unavailable", lastErrorCode: "provider_timeout" }))).toBe("error");
    expect(accountSurfaceTone(account({
      inPool: false,
      operationalStatus: "rotation",
      quotaRefreshStatus: "failed",
      quota: { error: { code: "quota_timeout" } },
    }))).toBe("warning");
    expect(accountSurfaceTone(account({ inPool: false }), true)).toBe("info");
  });
});
