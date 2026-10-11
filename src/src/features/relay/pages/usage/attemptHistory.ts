import type { RemoteUsageQuery } from "../../api/types";
import type { UsageRow } from "./usageData";

type AttemptPage = {
  rows: UsageRow[];
  page: number;
  totalPages: number;
};

export type AttemptHistory = { rows: UsageRow[]; complete: boolean };
export type AttemptPageLoader = (query: RemoteUsageQuery) => Promise<AttemptPage>;

const ATTEMPT_HISTORY_PAGE_SIZE = 200;
const ATTEMPT_HISTORY_MAX_PAGES = 100;

/** Independent of the report filters. Older servers may only support substring search. */
export async function loadRequestAttempts(
  requestId: string,
  loadPage: AttemptPageLoader,
  isActive: () => boolean = () => true,
): Promise<AttemptHistory> {
  const attemptsByRowId = new Map<UsageRow["id"], UsageRow>();
  // Bound old or inconsistent hosts; expose partial history instead of claiming completeness.
  let allPagesLoaded = false;
  for (let requestedPage = 1; requestedPage <= ATTEMPT_HISTORY_MAX_PAGES && isActive(); requestedPage += 1) {
    const attemptPage = await loadPage({
      page: requestedPage, pageSize: ATTEMPT_HISTORY_PAGE_SIZE, requestId, requestIdQuery: requestId,
      includeEvents: true, includeModels: false, includePoolMembers: false,
    });
    if (!isActive()) return { rows: [], complete: false };
    for (const attempt of attemptPage.rows) {
      if (attempt.requestId === requestId) attemptsByRowId.set(attempt.id, attempt);
    }
    if (attemptPage.page !== requestedPage || !Number.isFinite(attemptPage.totalPages)) break;
    if (requestedPage >= attemptPage.totalPages) {
      allPagesLoaded = true;
      break;
    }
  }
  return {
    rows: [...attemptsByRowId.values()].sort(compareAttemptHistoryRows),
    complete: allPagesLoaded,
  };
}

function compareAttemptHistoryRows(left: UsageRow, right: UsageRow) {
  return left.attempt - right.attempt
    || Date.parse(left.time) - Date.parse(right.time)
    || String(left.id).localeCompare(String(right.id), undefined, { numeric: true });
}
