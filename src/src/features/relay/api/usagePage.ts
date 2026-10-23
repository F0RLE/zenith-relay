import type { LocalUsagePage, UsageQuery } from "./types";

type UsagePageReader = (query: UsageQuery) => Promise<LocalUsagePage>;

/** Older hosts expose historical models/members through aggregate projections. */
export function createUsagePageReader(read: UsagePageReader) {
  const pendingHistory = new Map<string, Promise<LocalUsagePage>>();

  return async (query: UsageQuery = {}) => {
    const page = await read(query);
    const models = page.filterOptions?.models;
    const poolMembers = page.filterOptions?.poolMembers;
    if (!query.includeFilterOptions || (Array.isArray(models) && Array.isArray(poolMembers))) return page;

    // These choices describe the whole period, not the currently filtered page.
    // Use an existing read-only command without fetching all request records.
    const historyQuery: UsageQuery = {
      ...(query.range !== undefined ? { range: query.range } : {}),
      ...(query.fromMs !== undefined ? { fromMs: query.fromMs } : {}),
      ...(query.toMs !== undefined ? { toMs: query.toMs } : {}),
      includeEvents: false,
      includeModels: true,
      includePoolMembers: true,
    };
    const key = JSON.stringify(historyQuery);
    let history = pendingHistory.get(key);
    if (!history) {
      history = read(historyQuery);
      pendingHistory.set(key, history);
      const settled = () => {
        if (pendingHistory.get(key) === history) pendingHistory.delete(key);
      };
      void history.then(settled, settled);
    }
    const aggregates = await history;
    return {
      ...page,
      filterOptions: {
        ...page.filterOptions,
        models: Array.isArray(models) ? models : [...new Set(aggregates.models.map((group) => group.key).filter(Boolean))].sort(),
        poolMembers: Array.isArray(poolMembers) ? poolMembers : [...new Set(aggregates.poolMembers.map((group) => group.key).filter(Boolean))].sort(),
      },
    };
  };
}
