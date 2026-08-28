import { describe, expect, test } from "bun:test";
import type { AccountSummary, RuntimeSnapshot, WakeTask } from "../src/features/relay/api/types";
import {
  automationAccountSelectionValid,
  automationFormValid,
  availableAutomationModels,
  buildAutomationSubmission,
  eligibleAutomationAccounts,
  resolveAutomationModel,
  selectedAutomationAccounts,
} from "../src/features/relay/pages/connections/automationModel";

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
  models: ["gpt-5.4", "gpt-5.4-mini"],
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
});

const gateway: RuntimeSnapshot["gateway"] = {
  running: true,
  baseUrl: "http://127.0.0.1:0",
  candidateCount: 1,
  visibleModelIds: ["gpt-5.4", "gpt-5.4-mini"],
  maxRetryCandidates: 3,
  routingStrategy: "adaptive",
  defaultServiceTier: "standard",
};

describe("automation model", () => {
  test("limits accounts and intersects model entitlements", () => {
    const first = account("one", { models: ["gpt-5.4", "gpt-5.4-mini"] });
    const second = account("two", { models: ["gpt-5.4"] });
    const pool = eligibleAutomationAccounts([first, account("disabled", { enabled: false })]);
    const selected = selectedAutomationAccounts(pool, ["one"]);
    expect(pool.map((item) => item.id)).toEqual(["one"]);
    expect(selected).toEqual([first]);
    expect(availableAutomationModels(gateway, [first, second], "account_ids")).toEqual(["gpt-5.4"]);
  });

  test("validates selector state and resolves a stale model to the first available one", () => {
    const pool = [account("one")];
    expect(automationAccountSelectionValid("all_eligible", pool, [], pool)).toBe(true);
    expect(automationAccountSelectionValid("account_ids", pool, ["one"], pool)).toBe(true);
    expect(automationAccountSelectionValid("account_ids", pool, ["missing"], [])).toBe(false);
    expect(resolveAutomationModel(["gpt-5.4"], "removed")).toBe("gpt-5.4");
    expect(automationFormValid(" Name ", true, false, "gpt-5.4")).toBe(true);
    expect(automationFormValid("", true, true, "")).toBe(false);
  });

  test("builds weekly reset payloads with automatic secondary execution", () => {
    const submission = buildAutomationSubmission({
      task: null,
      name: "Weekly reset",
      executionPolicy: "require_confirmation",
      triggerKind: "weekly",
      selectorKind: "account_ids",
      accountIds: ["one"],
      selectedModel: "gpt-5.4",
      nowMs: 123,
    });
    expect(submission.operationId).toBe("automation-create");
    expect(submission.base).toMatchObject({
      accountSelector: { kind: "account_ids", values: ["one"] },
      windowKinds: ["secondary"],
      modelPolicy: { kind: "lightest_supported" },
      executionPolicy: "automatic",
      trigger: { kind: "weekly" },
    });
    expect(submission.remoteInput).toMatchObject({ id: "", createdAtMs: 123, updatedAtMs: 123, fallbackSchedule: null });
  });

  test("preserves an existing task identity and retry settings on update", () => {
    const task: WakeTask = {
      id: "task-1",
      name: "Old",
      enabled: false,
      accountSelector: { kind: "all_eligible" },
      windowKinds: ["primary"],
      modelPolicy: { kind: "lightest_supported" },
      trigger: { kind: "quota_full" },
      executionPolicy: "automatic",
      jitterSeconds: 4,
      maxAttemptsPerCycle: 3,
      createdAtMs: 1,
      updatedAtMs: 2,
    };
    const submission = buildAutomationSubmission({ task, name: "New", executionPolicy: "automatic", triggerKind: "quota_full", selectorKind: "all_eligible", accountIds: [], selectedModel: "gpt-5.4", nowMs: 5 });
    expect(submission.operationId).toBe("automation-update-task-1");
    expect(submission.base).toMatchObject({ name: "New", enabled: false, jitterSeconds: 4, maxAttemptsPerCycle: 3 });
    expect(submission.remoteInput).toMatchObject({ id: "task-1", createdAtMs: 1, updatedAtMs: 5 });
  });
});
