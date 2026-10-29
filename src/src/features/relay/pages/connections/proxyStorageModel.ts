import type { AccountSummary, ProxyCheckResult, ProxyPoolEntry, SourceSummary } from "../../api/types";

export function latestProxyCheck(entry: Pick<ProxyPoolEntry, "lastCheck">, manual?: ProxyCheckResult) {
  const saved = entry.lastCheck;
  return manual && (!saved || manual.checkedAtMs > saved.checkedAtMs) ? manual : saved ?? undefined;
}

/** A fresh check can explicitly clear a stale zone after an exit-IP change. */
export function proxyTimezone(entry: Pick<ProxyPoolEntry, "timezone">, result?: ProxyCheckResult): string | null {
  return result?.timezone === undefined ? entry.timezone ?? null : result.timezone;
}

type ProxyBinding = {
  accountNames: string[];
  sourceNames: string[];
  isCommon: boolean;
};

type ProxyBindingsInput = {
  entries: readonly Pick<ProxyPoolEntry, "id" | "assignedAccountIds">[];
  accounts: readonly Pick<AccountSummary, "id" | "label" | "proxyMode">[];
  sources: readonly Pick<SourceSummary, "proxyId" | "name">[];
  commonProxyId: string | null | undefined;
  unknownAccountLabel: string;
};

/** Project storage bindings once, separately from search and rendering. */
export function buildProxyBindings({
  entries, accounts, sources, commonProxyId, unknownAccountLabel,
}: ProxyBindingsInput): Map<string, ProxyBinding> {
  const accountsById = new Map(accounts.map((account) => [account.id, account]));
  const commonAccountIds = accounts.filter((account) => account.proxyMode === "common")
    .map((account) => account.id);
  const sourceNamesByProxyId = new Map<string, string[]>();
  for (const source of sources) {
    if (!source.proxyId) continue;
    const names = sourceNamesByProxyId.get(source.proxyId) ?? [];
    names.push(source.name);
    sourceNamesByProxyId.set(source.proxyId, names);
  }
  return new Map(entries.map((entry) => {
    const isCommon = commonProxyId === entry.id;
    const assignedIds = new Set(entry.assignedAccountIds);
    if (isCommon) for (const accountId of commonAccountIds) assignedIds.add(accountId);
    return [entry.id, {
      accountNames: [...assignedIds].map((accountId) => accountsById.get(accountId)?.label ?? unknownAccountLabel),
      sourceNames: sourceNamesByProxyId.get(entry.id) ?? [],
      isCommon,
    }];
  }));
}
