import type { LocalUsagePage, UsageFilterOptions } from "../api/types";

const CHOICE_FIELDS = ["models", "poolMembers", "wireApis", "transports", "errorCategories"] as const;

/** Share unchanged choices without retaining another report or altering usage facts. */
export function shareUsageFilterChoices(page: LocalUsagePage, older: LocalUsagePage["filterOptions"]): LocalUsagePage {
  const current = page.filterOptions;
  if (!current || !older) return page;
  const choices = { ...current };
  let changed = false;
  const share = <K extends keyof UsageFilterOptions>(field: K) => {
    const next = current[field];
    const before = older[field];
    if (next !== before && next && before && next.length === before.length
      && next.every((value, index) => value === before[index])) {
      choices[field] = before;
      changed = true;
    }
  };
  CHOICE_FIELDS.forEach(share);
  return changed ? { ...page, filterOptions: choices } : page;
}
