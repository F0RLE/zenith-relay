import type { AccountSummary } from "./api/types";
import type { TFunction } from "i18next";
import { accountQuotaRefreshState } from "./accountStatus";
import { accountPlanOption } from "./accountPlans";
import { isFastSupplementalQuota } from "./quotaFormatting";

type AccountRouteIndex = {
  byId: ReadonlyMap<string, AccountSummary>;
  byGroup: ReadonlyMap<string, readonly AccountSummary[]>;
};

const routeIndexes = new WeakMap<readonly AccountSummary[], AccountRouteIndex>();

// Runtime inventories are immutable snapshots. Release the index with its
// snapshot; cache grouping only, never derived quota/auth decisions.
function accountRouteIndex(accounts: readonly AccountSummary[]): AccountRouteIndex {
  const cached = routeIndexes.get(accounts);
  if (cached) return cached;
  const byId = new Map<string, AccountSummary>();
  const byGroup = new Map<string, AccountSummary[]>();
  for (const account of accounts) {
    if (!byId.has(account.id)) byId.set(account.id, account);
    const groupId = account.routeGroupId;
    if (!groupId) continue;
    const group = byGroup.get(groupId);
    if (group) group.push(account);
    else byGroup.set(groupId, [account]);
  }
  const index = { byId, byGroup };
  routeIndexes.set(accounts, index);
  return index;
}

export function visibleAccounts(accounts: AccountSummary[]): AccountSummary[] {
  return accounts.filter((account) => !account.routeInactive);
}

export function accountRouteConnections(account: AccountSummary, accounts: readonly AccountSummary[]): AccountSummary[] {
  const groupId = account.routeGroupId;
  // Preserve the caller-owned result array; sorting/removing a result cannot
  // mutate the index used by the other cards.
  return groupId ? accountRouteIndex(accounts).byGroup.get(groupId)?.slice() ?? [] : [account];
}

// Display freshness only; mirrors the core's default QUOTA_STALE_AFTER_MS, not admission.
const ROUTE_METADATA_MAX_AGE_MS = 20 * 60_000;

let metadataDateFormatter: { locale: string; formatter: Intl.DateTimeFormat } | undefined;

function formatMetadataDate(timestampMs: number, locale: string) {
  if (metadataDateFormatter?.locale === locale) {
    return metadataDateFormatter.formatter.format(timestampMs);
  }
  const formatter = new Intl.DateTimeFormat(locale, { dateStyle: "short", timeStyle: "short" });
  metadataDateFormatter = { locale, formatter };
  return formatter.format(timestampMs);
}

export function accountMetadataObservation(account: AccountSummary, resource: "plan" | "quota") {
  const windows = [account.quota.primary, account.quota.secondary,
    ...(account.quota.supplemental ?? []).filter((item) => !isFastSupplementalQuota(item)).map((item) => item.window),
  ].filter((window) => window != null);
  const timestamps = resource === "plan" ? [account.subscription.updatedAtMs] : windows.map((window) => window.observedAtMs);
  const observedAtMs = timestamps.length && timestamps.every((time) => time != null && Number.isFinite(time) && time > 0 && time <= 8.64e15)
    ? Math.min(...timestamps as number[]) : null;
  const latestObservedAtMs = observedAtMs == null ? null : Math.max(...timestamps as number[]);
  const deadlines = resource === "plan" ? [account.subscription.activeUntilMs] : [account.subscription.activeUntilMs, ...windows.map((window) => window.resetAtMs)];
  const staleAtMs = observedAtMs == null ? null : Math.min(observedAtMs + ROUTE_METADATA_MAX_AGE_MS,
    ...deadlines.filter((time): time is number => time != null && Number.isFinite(time)));
  return { observedAtMs, latestObservedAtMs, staleAtMs };
}

export function accountMetadataIsStale(account: AccountSummary, resource: "plan" | "quota", nowMs: number) {
  const { latestObservedAtMs, staleAtMs } = accountMetadataObservation(account, resource);
  return latestObservedAtMs == null || latestObservedAtMs > nowMs || staleAtMs == null || nowMs >= staleAtMs
    || account.quotaRefreshStatus === "failed" || Boolean(account.quota.error)
    || (account.refreshState != null && account.refreshState.quota !== "fresh");
}

export function accountMetadataHint(source: AccountSummary, resource: "plan" | "quota", nowMs: number, t: TFunction, locale: string) {
  const { observedAtMs } = accountMetadataObservation(source, resource);
  const time = observedAtMs != null && observedAtMs <= nowMs
    ? formatMetadataDate(observedAtMs, locale)
    : t("accounts.routeMetadataTimeUnknown");
  return [
    t("accounts.routeMetadataSource", { client: source.oauthClientKind === "excel_bps" ? "Excel / BPS" : "Codex", time }),
    accountMetadataIsStale(source, resource, nowMs) ? t("accounts.routeMetadataStale") : null,
    resource === "quota" && source.quota.limitReached ? t("pool.memberStatus.quotaWait") : null,
    t("accounts.routeMetadataFrom"),
  ].filter(Boolean).join(" · ");
}

/** Linked-route references only: association proves neither the same workspace nor shared limits. */
export function accountMetadataSources(account: AccountSummary, accounts: readonly AccountSummary[], nowMs = Date.now()) {
  let plan = account;
  let quota = account;
  if (account.oauthClientKind !== "siwc" || account.recoveryRequired
    || accountQuotaRefreshState(account) === "requires_reauth") return { plan, quota };
  const hasQuota = (candidate: AccountSummary) => Boolean(candidate.quota.primary || candidate.quota.secondary
    || candidate.quota.supplemental?.some((window) => !isFastSupplementalQuota(window)));
  for (const connection of accountMetadataConnections(account, accounts)) {
    if (!account.subscription.planType?.trim() && connection.subscription.planType?.trim()
      && (plan === account || (accountMetadataObservation(connection, "plan").observedAtMs ?? 0)
        > (accountMetadataObservation(plan, "plan").observedAtMs ?? 0))) plan = connection;
    if (!account.quota.limitReached && !hasQuota(account) && hasQuota(connection)) {
      const sourceStale = accountMetadataIsStale(connection, "quota", nowMs);
      const currentStale = accountMetadataIsStale(quota, "quota", nowMs);
      if (quota === account || (currentStale && !sourceStale)
        || (currentStale === sourceStale && (accountMetadataObservation(connection, "quota").observedAtMs ?? 0)
          > (accountMetadataObservation(quota, "quota").observedAtMs ?? 0))) quota = connection;
    }
  }
  return { plan, quota };
}

function accountMetadataConnections(account: AccountSummary, accounts: readonly AccountSummary[]) {
  return accountRouteConnections(account, accounts).filter((connection) =>
    connection.id !== account.id && connection.oauthClientKind !== "siwc" && !connection.recoveryRequired
    && connection.secretAvailable !== false && connection.enabled !== false && !connection.draining
    && connection.health !== "blocked" && connection.health !== "unhealthy"
    && accountQuotaRefreshState(connection) !== "requires_reauth" && connection.authState.state !== "error");
}

/** A linked date is display-only and must describe the displayed plan, never the selected token. */
export function accountSubscriptionSource(account: AccountSummary, accounts: readonly AccountSummary[]) {
  if (account.subscription.activeUntilMs != null || account.recoveryRequired
    || accountQuotaRefreshState(account) === "requires_reauth") return account;
  const { plan } = accountMetadataSources(account, accounts);
  const planId = accountPlanOption(plan.subscription.planType, "unknown").id;
  return accountMetadataConnections(account, accounts)
    .filter((connection) => connection.subscription.activeUntilMs != null
      && accountPlanOption(connection.subscription.planType, "unknown").id === planId)
    .reduce((source, connection) => source === account
      || (connection.subscription.updatedAtMs ?? 0) > (source.subscription.updatedAtMs ?? 0) ? connection : source, account);
}

export function expandAccountRouteIds(ids: string[], accounts: AccountSummary[]): string[] {
  if (!ids.length) return [];
  const index = accountRouteIndex(accounts);
  const expanded = new Set<string>();
  for (const id of ids) {
    const account = index.byId.get(id);
    const connections = account?.routeGroupId ? index.byGroup.get(account.routeGroupId) : undefined;
    if (connections) {
      for (const connection of connections) expanded.add(connection.id);
    } else expanded.add(id);
  }
  return [...expanded];
}
