import { expect, test } from "bun:test";
import { buildAutomationSubmission, weeklyResetThresholdBasisPoints } from "../src/features/relay/pages/connections/automationModel";

test("weekly thresholds accept whole percentages and treat blank as exhausted", () => {
  for (const [input, expected] of [["", 0], ["  ", 0], ["0", 0], ["20", 2_000], ["100", 10_000]] as const) {
    expect(weeklyResetThresholdBasisPoints(input)).toBe(expected);
  }
  for (const invalid of ["-1", "101", "1.5", "1e2", "NaN", "Infinity"]) {
    expect(weeklyResetThresholdBasisPoints(invalid)).toBeNull();
  }
});

test("threshold is absent by default and cleared on a non-weekly rule", () => {
  const input = {
    task: null, automationName: "Synthetic weekly reset", triggerKind: "weekly" as const,
    selectorKind: "all_eligible" as const, accountIds: [], selectedModel: "",
  };
  expect(buildAutomationSubmission(input).base.weeklyResetRemainingBasisPoints).toBeNull();
  expect(buildAutomationSubmission({ ...input, weeklyResetRemainingBasisPoints: 2_000 }).base.weeklyResetRemainingBasisPoints).toBe(2_000);
  expect(buildAutomationSubmission({ ...input, triggerKind: "quota_full", weeklyResetRemainingBasisPoints: 2_000 }).base.weeklyResetRemainingBasisPoints).toBeNull();
});
