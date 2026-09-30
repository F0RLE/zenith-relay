const subscriptionPlanPriority = ["enterprise", "business", "pro-20x", "pro-5x", "pro", "plus", "go", "edu", "free", "unknown"];
const accountPlanOrder = ["plus", "pro", "pro-5x", "pro-20x", "business", "enterprise", "free", "go", "edu", "unknown"];

export function compareSubscriptionPlanPriority(left: { id: string; label: string }, right: { id: string; label: string }) {
  const leftRank = subscriptionPlanPriority.indexOf(left.id);
  const rightRank = subscriptionPlanPriority.indexOf(right.id);
  return (leftRank < 0 ? subscriptionPlanPriority.length : leftRank) - (rightRank < 0 ? subscriptionPlanPriority.length : rightRank) || left.label.localeCompare(right.label);
}

export function formatAccountPlan(planType: string | null, unknown: string) {
  const value = planType?.trim();
  if (!value) return unknown;
  const key = value.toLocaleLowerCase().replace(/[\s_-]/g, "");
  if (key.includes("team") || key.includes("business")) return "Business";
  if (key.includes("enterprise")) return "Enterprise";
  if (key === "prolite") return "Pro 5x";
  if (key === "promax") return "Pro 20x";
  if (key === "pro") return "Pro";
  if (key.includes("plus")) return "Plus";
  if (key === "free") return "Free";
  if (key === "go") return "Go";
  if (key === "edu" || key.includes("education")) return "Edu";
  return value;
}

export function accountPlanOption(planType: string | null, unknown: string) {
  const label = formatAccountPlan(planType, unknown);
  return {
    id: label.toLocaleLowerCase().replace(/[^a-z0-9]+/g, "-").replace(/^-|-$/g, "") || "unknown",
    label,
  };
}

export function compareAccountPlans(left: { id: string; label: string }, right: { id: string; label: string }) {
  const leftRank = accountPlanOrder.indexOf(left.id);
  const rightRank = accountPlanOrder.indexOf(right.id);
  return (leftRank < 0 ? accountPlanOrder.length : leftRank) - (rightRank < 0 ? accountPlanOrder.length : rightRank) || left.label.localeCompare(right.label);
}
