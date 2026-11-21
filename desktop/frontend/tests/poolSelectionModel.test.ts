import { describe, expect, test } from "bun:test";
import type { AccountSummary } from "../src/features/relay/api/types";
import type { PoolMember } from "../src/features/relay/poolHelpers";
import { poolMemberKey, poolRefreshTargets } from "../src/features/relay/pages/pool/poolSelectionModel";
import { setVisibleIdsSelected, toggleSelectedId } from "../src/features/relay/state/selectionModel";

const account = (id: string, overrides: Partial<AccountSummary> = {}): AccountSummary => ({
  id, enabled: true, secretAvailable: true, oauthClientKind: "excel_bps", inPool: true,
  authState: { state: "active" }, quotaRefreshStatus: "updated",
  subscription: { planType: null, activeUntilMs: null, status: "unknown", updatedAtMs: null },
  quota: { primary: null, secondary: null, updatedAtMs: null, limitReached: false,
    resetCreditsAvailable: null, directBalanceMicroUsd: null, error: null },
  ...overrides,
} as AccountSummary);
const asMember = (account: AccountSummary): PoolMember => ({ ...account, kind: "account" });
const source = (id: string, secretAvailable = true): PoolMember => ({
  kind: "source", id, secretAvailable,
} as PoolMember);

describe("pool selection", () => {
  test("account and source IDs cannot collide or duplicate on select all", () => {
    const keys = [poolMemberKey(asMember(account("same"))), poolMemberKey(source("same"))];
    expect(keys).toEqual(["account:same", "source:same"]);
    expect(setVisibleIdsSelected([keys[0]!], keys, true)).toEqual(keys);
    expect(toggleSelectedId(keys, keys[0]!)).toEqual([keys[1]!]);
    expect(setVisibleIdsSelected(keys, keys, false)).toEqual([]);
  });

  test("refresh targets only selected eligible members", () => {
    const selected = account("selected");
    const others = [account("unselected"), account("disabled", { enabled: false }),
      account("draining", { draining: true }), account("recovery", { recoveryRequired: true }),
      account("missing-secret", { secretAvailable: false }), account("reauth", { quotaRefreshStatus: "requires_reauth" }),
      account("refreshing", { quotaRefreshStatus: "refreshing" })];
    expect(poolRefreshTargets([asMember(selected), ...others.slice(1).map(asMember), source("api"), source("no-key", false)], [selected, ...others]))
      .toEqual({ accountIds: ["selected"], sourceIds: ["api"] });
  });

  test("linked SIWC quota readers refresh once without including unrelated accounts", () => {
    const reader = account("reader", { routeGroupId: "group", routeInactive: true, inPool: false,
      quota: { ...account("blank").quota, primary: { kind: "primary", availableBasisPoints: 7000,
        resetAtMs: null, windowMinutes: 300, explicitlyFull: false, observedAtMs: Date.now() } },
    });
    const siwc = account("siwc", { oauthClientKind: "siwc", routeGroupId: "group" });
    const second = { ...siwc, id: "second" };
    const accounts = [siwc, second, reader, account("unselected")];
    const before = structuredClone(accounts);
    expect(poolRefreshTargets([asMember(siwc), asMember(second), asMember(reader)], accounts))
      .toEqual({ accountIds: ["reader"], sourceIds: [] });
    expect(accounts).toEqual(before);
  });
});
