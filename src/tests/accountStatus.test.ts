import { describe, expect, test } from "bun:test";
import type { AccountSummary, CandidateRuntimeSnapshot } from "../src/features/relay/api/types";
import {
  accountErrorTranslationKey,
  currentAccountErrorCode,
  isCodexOauthAccountEligible,
  operationalStatusTone,
  transientCandidateTone,
} from "../src/features/relay/accountStatus";

function account(overrides: Partial<AccountSummary> = {}): AccountSummary {
  return {
    id: "account",
    label: "Account",
    identityHint: "Account",
    enabled: true,
    inPool: true,
    draining: false,
    authState: { state: "ready" },
    health: "ready",
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

describe("account status policy", () => {
  test("maps operational and transient scheduler state without treating cooldown as failure", () => {
    expect(operationalStatusTone("rotation")).toBe("ready");
    expect(operationalStatusTone("quotaWait")).toBe("warning");
    expect(operationalStatusTone("unavailable")).toBe("error");
    expect(transientCandidateTone({ halfOpen: true } as CandidateRuntimeSnapshot, 100, true)).toBe("info");
    expect(transientCandidateTone({ nextRetryAtMs: 101 } as CandidateRuntimeSnapshot, 100, true)).toBe("warning");
    expect(transientCandidateTone({ nextRetryAtMs: 101 } as CandidateRuntimeSnapshot, 100, false)).toBeNull();
  });

  test("keeps account eligibility and error precedence explicit", () => {
    expect(isCodexOauthAccountEligible(account({ operationalStatus: "quotaWait" }))).toBeTrue();
    expect(isCodexOauthAccountEligible(account({ inPool: false }))).toBeFalse();
    expect(currentAccountErrorCode(account({ quotaRefreshStatus: "failed", quota: { error: { code: "quota_timeout" } } }))).toBe("quota_timeout");
    expect(currentAccountErrorCode(account({ authState: { state: "requires_reauth", reason: "expired" } }))).toBe("auth_expired");
    expect(currentAccountErrorCode(account({ operationalStatus: "unavailable", lastErrorCode: "provider_timeout" }))).toBe("provider_timeout");
  });

  test("maps safe error codes to stable translation keys", () => {
    expect(accountErrorTranslationKey("HTTP 429 rate-limit")).toBe("accounts.errors.rateLimited");
    expect(accountErrorTranslationKey("invalid_grant")).toBe("accounts.errors.invalidGrant");
    expect(accountErrorTranslationKey("unknown_provider_problem")).toBe("accounts.errors.unknown");
  });
});
