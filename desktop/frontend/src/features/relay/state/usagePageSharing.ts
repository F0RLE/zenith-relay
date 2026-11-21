import type { LocalUsagePage } from "../api/types";

const CHOICE_FIELDS = ["models", "poolMembers", "wireApis", "transports", "errorCategories"] as const;

/** Share unchanged choices without retaining another report or altering usage facts. */
export function shareUsageFilterChoices(page: LocalUsagePage, older: LocalUsagePage["filterOptions"]): LocalUsagePage {
  const current = page.filterOptions;
  if (!current || !older) return page;
  let choices = current;
  for (const field of CHOICE_FIELDS) {
    const next = current[field];
    const before = older[field];
    if (next !== before && next && before && next.length === before.length
      && next.every((value, index) => value === before[index])) {
      if (choices === current) choices = { ...current };
      choices[field] = before;
    }
  }
  return choices === current ? page : { ...page, filterOptions: choices };
}
