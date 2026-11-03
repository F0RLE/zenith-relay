import type { OwnerConnection } from "./model";

export interface PoolActivityMember { id: string; kind: OwnerConnection["kind"] }
/** Safe display projection supplied by the server adapter, or the isolated preview. */
export interface OwnerPoolActivity {
  members: (PoolActivityMember & { activeRequests: number; models: { id: string; requests: number }[] })[];
  next?: PoolActivityMember | null;
  last?: PoolActivityMember | null;
}

export const poolActivityKey = (member: PoolActivityMember) => `${member.kind}:${member.id}`;
const positiveCount = (value: number) => Number.isSafeInteger(value) && value > 0 ? value : 0;

export function projectPoolActivity(connections: readonly OwnerConnection[], activity?: OwnerPoolActivity) {
  const inventory = new Map(connections.filter(member => member.inPool && !member.trashed).map(member => [poolActivityKey(member), member]));
  const memberRequests = new Map<string, number>();
  const models = new Map<string, { id: string; requests: number }>();
  let total = 0;
  for (const row of activity?.members ?? []) {
    const key = poolActivityKey(row), count = positiveCount(row.activeRequests);
    if (!inventory.has(key) || !count || memberRequests.has(key)) continue;
    memberRequests.set(key, count);
    total += count;
    for (const model of row.models) {
      const id = model.id.trim(), requests = positiveCount(model.requests);
      if (!id || !requests) continue;
      const modelKey = id.toLowerCase();
      const previous = models.get(modelKey);
      models.set(modelKey, { id: previous?.id ?? id, requests: (previous?.requests ?? 0) + requests });
    }
  }
  const resolve = (member: PoolActivityMember | null | undefined) => member ? inventory.get(poolActivityKey(member)) : undefined;
  return {
    observed: activity !== undefined,
    current: [...memberRequests.keys()].map(key => inventory.get(key)!),
    memberRequests, models: [...models.values()], total,
    next: resolve(activity?.next), last: resolve(activity?.last),
  };
}

export type PoolActivityPresentation = ReturnType<typeof projectPoolActivity>;
