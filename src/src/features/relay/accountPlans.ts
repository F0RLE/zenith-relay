const subscriptionPlanPriority = ["enterprise", "business", "pro-500", "pro-200", "pro-100", "plus", "go", "edu", "free", "unknown"];
const accountPlanOrder = ["plus", "pro-100", "pro-200", "pro-500", "business", "enterprise", "free", "go", "edu", "unknown"];
const proTierLabels: Record<string, string> = {
  prolite: "Pro 100",
  pro5x: "Pro 100",
  pro100: "Pro 100",
  chatgptprolite: "Pro 100",
  pro: "Pro 200",
  proplan: "Pro 200",
  pro20x: "Pro 200",
  pro200: "Pro 200",
  chatgptpro: "Pro 200",
  chatgptproplan: "Pro 200",
  promax: "Pro 500",
  pro500: "Pro 500",
  chatgptpromax: "Pro 500",
};

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
  const proTier = proTierLabels[key];
  if (proTier) return proTier;
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
