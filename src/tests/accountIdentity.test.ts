import { describe, expect, test } from "bun:test";
import type { AccountSummary } from "../src/features/relay/api/types";
import {
  buildAccountIdentityIndex,
  displayAccountIdentity,
  replaceRevealedAccountIdentities,
  revealableAccountIds,
} from "../src/features/relay/state/accountIdentity";

function account(overrides: Partial<AccountSummary>): AccountSummary {
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

describe("account identity display", () => {
  test("does not reveal an identity for ambiguous account references", () => {
    const index = buildAccountIdentityIndex([
      account({ id: "duplicate", label: "First" }),
      account({ id: "duplicate", label: "Second" }),
      account({ id: "third", label: "First" }),
    ]);
    const options = {
      index,
      identitiesVisible: true,
      canReveal: true,
      mode: "local" as const,
      revealedIdentities: { "local:duplicate": "secret@example.test" },
    };

    expect(displayAccountIdentity({ ...options, accountId: "duplicate", fallbackLabel: "Fallback" })).toBe("Fallback");
    expect(displayAccountIdentity({ ...options, fallbackLabel: "First" })).toBe("First");
  });

  test("uses a revealed identity only for a supported account with a secret", () => {
    const visible = account({ id: "visible", label: "Masked", secretAvailable: true });
    const hidden = account({ id: "hidden", label: "Still masked", secretAvailable: false });
    const index = buildAccountIdentityIndex([visible, hidden]);
    const revealedIdentities = { "remote:visible": "visible@example.test", "remote:hidden": "hidden@example.test" };

    expect(displayAccountIdentity({ index, accountId: visible.id, fallbackLabel: null, identitiesVisible: true, canReveal: true, mode: "remote", revealedIdentities })).toBe("visible@example.test");
    expect(displayAccountIdentity({ index, accountId: hidden.id, fallbackLabel: null, identitiesVisible: true, canReveal: true, mode: "remote", revealedIdentities })).toBe("Still masked");
    expect(displayAccountIdentity({ index, accountId: visible.id, fallbackLabel: null, identitiesVisible: false, canReveal: true, mode: "remote", revealedIdentities })).toBe("Masked");
  });

  test("keeps revealed identities scoped to the active relay mode", () => {
    expect(replaceRevealedAccountIdentities(
      { "local:one": "local@example.test", "remote:stale": "stale@example.test" },
      "remote",
      [{ accountId: "two", identity: "remote@example.test" }],
    )).toEqual({
      "local:one": "local@example.test",
      "remote:two": "remote@example.test",
    });
    expect(revealableAccountIds([
      account({ id: "available", secretAvailable: true }),
      account({ id: "hidden", secretAvailable: false }),
    ], true)).toEqual(["available"]);
    expect(revealableAccountIds([account({ id: "available" })], false)).toEqual([]);
  });
});
