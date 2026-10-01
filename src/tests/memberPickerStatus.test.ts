import { describe, expect, test } from "bun:test";
import type { AccountSummary } from "../src/features/relay/api/types";
import {
  accountPickerTone,
  compareMemberPickerHealth,
  MEMBER_PICKER_HEALTH_ORDER,
  memberPickerHealth,
  sourcePickerTone,
} from "../src/features/relay/pages/pool/memberPickerStatus";

function account(overrides: Partial<AccountSummary> = {}): AccountSummary {
  return {
    id: "account",
    label: "Account",
    identityHint: "Account",
    enabled: true,
    inPool: false,
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

describe("pool member picker status", () => {
  test("orders working, cooldown, unavailable, then the remaining states", () => {
    expect(MEMBER_PICKER_HEALTH_ORDER).toEqual(["ready", "cooldown", "error", "disabled", "info"]);
    expect(memberPickerHealth("warning")).toBe("cooldown");
    expect(compareMemberPickerHealth("ready", "cooldown")).toBeLessThan(0);
    expect(compareMemberPickerHealth("cooldown", "error")).toBeLessThan(0);
    expect(compareMemberPickerHealth("error", "disabled")).toBeLessThan(0);
    expect(compareMemberPickerHealth("disabled", "info")).toBeLessThan(0);
  });

  test("classifies accounts and API sources with the same health groups", () => {
    expect(accountPickerTone(account(), false)).toBe("ready");
    expect(accountPickerTone(account({ operationalStatus: "quotaWait" }), false)).toBe("warning");
    expect(accountPickerTone(account({ operationalStatus: "unavailable", lastErrorCode: "account_unavailable" }), false)).toBe("error");
    expect(accountPickerTone(account({ operationalStatus: "disabled" }), false)).toBe("disabled");
    expect(accountPickerTone(account(), true)).toBe("info");

    expect(sourcePickerTone({ operationalStatus: "rotation", lastErrorCode: null })).toBe("ready");
    expect(sourcePickerTone({ operationalStatus: "quotaWait", lastErrorCode: null })).toBe("warning");
    expect(sourcePickerTone({ operationalStatus: "rotation", lastErrorCode: "upstream_failure" })).toBe("error");
    expect(sourcePickerTone({ operationalStatus: "unavailable", lastErrorCode: "   " })).toBe("error");
    expect(sourcePickerTone({ operationalStatus: "disabled", lastErrorCode: null })).toBe("disabled");
  });
});
