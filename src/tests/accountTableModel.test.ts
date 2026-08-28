import { describe, expect, test } from "bun:test";
import type { AccountSummary } from "../src/features/relay/api/types";
import {
  accountCounts,
  accountPlanOptions,
  accountSelectionState,
  activeAccountPlan,
  filterAndSortAccounts,
  visiblePlanCounts,
} from "../src/features/relay/pages/connections/accountTableModel";

const account = (id: string, overrides: Partial<AccountSummary> = {}): AccountSummary => ({
  id,
  label: id,
  identityHint: id,
  enabled: true,
  inPool: true,
  draining: false,
  authState: { state: "ready" },
  health: "ready",
  operationalStatus: "rotation",
  models: ["gpt-5.4"],
  allowedModels: [],
  excludedModels: [],
  priority: 1,
  weight: 1,
  apiEquivalent: { microUsd: 0, unpricedTokens: 0 },
  subscription: { planType: "plus", activeUntilMs: null, status: "active", updatedAtMs: null },
  quota: {},
  quotaRefreshStatus: "updated",
  secretAvailable: true,
  lastErrorCode: null,
  ...overrides,
});

describe("account table model", () => {
  test("derives counts and plan options from the complete account list", () => {
    const accounts = [account("one"), account("two", { enabled: false, inPool: false, subscription: { planType: "business", activeUntilMs: null, status: "active", updatedAtMs: null } })];
    expect(accountCounts(accounts)).toEqual({ errorCount: 0, inPoolCount: 1, disabledCount: 1 });
    expect(accountPlanOptions(accounts, "Unknown")).toMatchObject([{ id: "plus", count: 1 }, { id: "business", count: 1 }]);
    expect(visiblePlanCounts(accounts, "Unknown")).toEqual(new Map([["plus", 1], ["business", 1]]));
  });

  test("filters by query, plan, and participation while preserving runtime order", () => {
    const accounts = [account("first", { label: "Zed" }), account("second", { label: "Amy", inPool: false, subscription: { planType: "business", activeUntilMs: null, status: "active", updatedAtMs: null } })];
    const order = new Map([["second", 0], ["first", 1]]);
    expect(filterAndSortAccounts(accounts, "", "all", "all", false, order, "Unknown").map((item) => item.id)).toEqual(["second", "first"]);
    expect(filterAndSortAccounts(accounts, "amy", "all", "excluded", false, order, "Unknown").map((item) => item.id)).toEqual(["second"]);
    expect(filterAndSortAccounts(accounts, "", "plus", "all", false, order, "Unknown").map((item) => item.id)).toEqual(["first"]);
  });

  test("keeps selection actions scoped to visible rows and all-account exports", () => {
    const all = [account("one"), account("two", { remoteLocation: { serverId: "server", remoteAccountId: "remote" } })];
    const visible = [all[0]!];
    const state = accountSelectionState(all, visible, ["one"]);
    expect(state).toMatchObject({ selectedIds: ["one"], selectedCount: 1, exportIds: ["one"], allSelected: true, canIncludeSelected: false, canExcludeSelected: true });
    expect(accountSelectionState(all, visible, [])).toMatchObject({ selectedCount: 0, exportIds: ["one", "two"], allSelected: false });
    expect(activeAccountPlan("removed", [], 0)).toBe("all");
  });
});
