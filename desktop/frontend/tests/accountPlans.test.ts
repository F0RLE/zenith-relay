import { describe, expect, test } from "bun:test";
import { accountPlanOption, compareAccountPlans, formatAccountPlan } from "../src/features/relay/accountPlans";

describe("account plan badges", () => {
  test("shows the current OpenAI Pro tiers and still recognizes the old labels", () => {
    expect(formatAccountPlan("prolite", "Unknown")).toBe("Pro 100");
    expect(formatAccountPlan("Pro 5x", "Unknown")).toBe("Pro 100");
    expect(formatAccountPlan("pro-100", "Unknown")).toBe("Pro 100");
    expect(formatAccountPlan("pro", "Unknown")).toBe("Pro 200");
    expect(formatAccountPlan("pro-20x", "Unknown")).toBe("Pro 200");
    expect(formatAccountPlan("promax", "Unknown")).toBe("Pro 500");
    expect(formatAccountPlan("PRO-500", "Unknown")).toBe("Pro 500");
    expect(formatAccountPlan("plus", "Unknown")).toBe("Plus");
    expect(formatAccountPlan("team", "Unknown")).toBe("Business");
    expect(formatAccountPlan(null, "Unknown")).toBe("Unknown");
  });

  test("orders Pro 100, 200, and 500 and keeps the Pro badge color", () => {
    const plans = ["promax", "pro", "prolite", "plus"].map((planType) => accountPlanOption(planType, "Unknown"));
    expect(plans.map((plan) => plan.id)).toEqual(["pro-500", "pro-200", "pro-100", "plus"]);
    expect(plans.slice(0, 3).every((plan) => plan.id.startsWith("pro"))).toBe(true);
    expect([...plans].sort(compareAccountPlans).map((plan) => plan.label)).toEqual(["Plus", "Pro 100", "Pro 200", "Pro 500"]);
  });
});
