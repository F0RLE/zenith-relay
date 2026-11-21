import type { Rule } from "../../api/catalog";
import type { Serving } from "../../api/connections";
import type { PoolActivity } from "../../api/poolActivity";

export interface PoolMember {
  id: string;
  label: string;
  serving: Serving;
  proxy: boolean;
  models: string[];
}

export function inventoryForPool(rows: readonly Rule[] | undefined) {
  const members = new Map<string, PoolMember>();
  const candidates: Rule[] = [];
  for (const row of rows ?? []) {
    if (!row.admitted && !row.enabled && row.ready && row.serving?.state === "enabled") candidates.push(row);
    if (!row.admitted || !row.serving?.in_pool) continue;
    let member = members.get(row.source_id);
    if (!member) {
      member = { id: row.source_id, label: row.source, serving: row.serving, proxy: row.proxy, models: [] };
      members.set(row.source_id, member);
    }
    if (!member.models.includes(row.model)) member.models.push(row.model);
  }
  return { members: [...members.values()], candidates };
}

export function livePoolActivity(view: PoolActivity | undefined, members: readonly PoolMember[]) {
  const sources = new Map(members.map(member => [member.id, member.label]));
  const current = new Map<string, { name: string }>();
  const requests = new Map<string, number>();
  const models = new Map<string, { id: string; requests: number }>();
  let total = 0;
  for (const row of view?.items ?? []) {
    current.set(row.source_id, { name: sources.get(row.source_id) ?? row.source });
    requests.set(row.source_id, (requests.get(row.source_id) ?? 0) + row.requests);
    models.set(row.model, { id: row.model, requests: (models.get(row.model)?.requests ?? 0) + row.requests });
    total += row.requests;
  }
  return { total, observed: !!view, current: [...current.values()], models: [...models.values()], requests };
}

export function liveMemberOrder(members: PoolMember[], view: PoolActivity | undefined) {
  if (view?.strategy !== "failover") return members;
  const ranks = new Map(view.member_order.map((id, rank) => [id, rank]));
  const rank = (member: PoolMember) => ranks.get(member.id) ?? ranks.size;
  return [...members].sort((a, b) => rank(a) - rank(b) || a.id.localeCompare(b.id, "en"));
}
