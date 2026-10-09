import { useEffect, useRef, useState } from "react";
import type { ReactNode } from "react";
import { Check, CheckCheck, Layers, Plus, Search, Server, UserRound } from "lucide-react";
import { useTranslation } from "react-i18next";
import { AccountBadges, Button, Dialog, EmptyState, OptionMenu, accountErrorLabel } from "../../components/Ui";
import { accountPlanOption, compareAccountPlans } from "../../accountPlans";
import { accountQuotaRefreshState, currentAccountErrorCode } from "../../accountStatus";
import { compareStableText, toggle } from "../../poolHelpers";
import { updatePoolMembership } from "../../poolMembership";
import { useRelayState } from "../../state/RelayStateProvider";
import { usePoolAccountWarning } from "../../hooks/usePoolAccountWarning";
import { accountPickerTone, compareMemberPickerHealth, MEMBER_PICKER_HEALTH_ORDER, memberPickerHealth, sourcePickerTone, type MemberPickerHealth } from "./memberPickerStatus";
import type { AccountSummary, SourceSummary } from "../../api/types";
import type { TFunction } from "i18next";

type MemberView = "all" | "accounts" | "sources" | "selected";
type SurfaceTone = "ready" | "warning" | "error" | "info" | "disabled";
type HealthFilter = "all" | MemberPickerHealth;

function MemberOption({ name, detail, status, statusLabel, icon, badge, checked, disabled, onChange }: {
  name: string; detail?: string; status: SurfaceTone; statusLabel: string; icon: ReactNode; badge?: ReactNode;
  checked: boolean; disabled: boolean; onChange: () => void;
}) {
  return <label className="pool-picker-option" data-selected={checked}>
    <span className="pool-picker-avatar" data-status={status} data-relay-tooltip={statusLabel} aria-hidden>{icon}</span>
    <span className="pool-member-option-copy">
      <strong>{name}</strong>
      <small className="pool-picker-status" data-status={status}>{statusLabel}</small>
      {detail ? <small>{detail}</small> : null}
    </span>
    {badge}
    <input type="checkbox" aria-label={`${name}, ${statusLabel}`} checked={checked} disabled={disabled} onChange={onChange} />
  </label>;
}

function accountPickerStatus(account: AccountSummary, onServer: boolean, t: TFunction): { status: SurfaceTone; statusLabel: string } {
  const status = accountPickerTone(account, onServer);
  const operationalLabel = t(`pool.memberStatus.${account.operationalStatus}`);
  if (onServer) return { status, statusLabel: t("accounts.onServerHint") };
  const quotaStatus = accountQuotaRefreshState(account);
  const errorCode = quotaStatus === "refreshing" ? null : currentAccountErrorCode(account);
  if (errorCode) return { status, statusLabel: accountErrorLabel(errorCode, t) };
  if (quotaStatus !== "updated") return { status, statusLabel: `${t(`accounts.quotaRefreshStatus.${quotaStatus}`)} · ${operationalLabel}` };
  if (account.clientAuthStatus === "login_required") return { status, statusLabel: `${t("accounts.clientAuthWarning")} · ${operationalLabel}` };
  return { status, statusLabel: operationalLabel };
}

function sourcePickerStatus(source: SourceSummary, t: TFunction): { status: SurfaceTone; statusLabel: string } {
  const status = sourcePickerTone(source);
  const errorCode = source.lastErrorCode?.trim();
  if (errorCode) return { status, statusLabel: t("pool.runtimeError", { code: errorCode }) };
  return { status, statusLabel: t(`pool.memberStatus.${source.operationalStatus}`) };
}

export function AddMembersDialog({ onClose, onAddSource }: { onClose: () => void; onAddSource: () => void }) {
  const { t } = useTranslation();
  const { mode, runtime, perform, busy } = useRelayState();
  const confirmPoolAccounts = usePoolAccountWarning();
  const addPending = useRef(false);
  const canAddSource = mode !== "remote" || Boolean(runtime?.capabilities.features.includes("sources"));
  const [accountIds, setAccountIds] = useState<string[]>([]);
  const [sourceIds, setSourceIds] = useState<string[]>([]);
  const [query, setQuery] = useState("");
  const [view, setView] = useState<MemberView>("all");
  const [planFilter, setPlanFilter] = useState("all");
  const [healthFilter, setHealthFilter] = useState<HealthFilter>("all");
  const listRef = useRef<HTMLDivElement>(null);
  const allAccounts = (runtime?.accounts ?? []).filter((account) => !account.inPool);
  const allSources = (runtime?.sources ?? []).filter((source) => !source.inPool);
  const planOptions = new Map<string, { id: string; label: string; count: number }>();
  for (const account of allAccounts) {
    const option = accountPlanOption(account.subscription.planType, t("common.unknown"));
    const existingOption = planOptions.get(option.id);
    planOptions.set(option.id, { ...option, count: (existingOption?.count ?? 0) + 1 });
  }
  const plans = [...planOptions.values()].sort(compareAccountPlans);
  const activePlan = view !== "accounts" || !planOptions.has(planFilter) ? "all" : planFilter;
  const normalizedQuery = query.trim().toLocaleLowerCase();
  const matches = (...searchFields: Array<string | null | undefined>) => !normalizedQuery || searchFields.some((searchField) => searchField?.toLocaleLowerCase().includes(normalizedQuery));
  const accountOnServer = (account: AccountSummary) => mode === "local" && Boolean(account.remoteLocation);
  const listedAccounts = allAccounts
    .filter((account) => view !== "sources" && (view !== "selected" || accountIds.includes(account.id)))
    .filter((account) => activePlan === "all" || accountPlanOption(account.subscription.planType, t("common.unknown")).id === activePlan)
    .filter((account) => matches(account.identityHint, account.label, account.subscription.planType));
  const listedSources = allSources
    .filter((source) => view !== "accounts" && (view !== "selected" || sourceIds.includes(source.id)) && matches(source.name, source.baseUrl));
  const healthCounts = new Map<MemberPickerHealth, number>();
  const countHealth = (health: MemberPickerHealth) => healthCounts.set(health, (healthCounts.get(health) ?? 0) + 1);
  const rankedAccounts = listedAccounts.map((account) => ({
    account,
    health: memberPickerHealth(accountPickerTone(account, accountOnServer(account))),
  }));
  const rankedSources = listedSources.map((source) => ({
    source,
    health: memberPickerHealth(sourcePickerTone(source)),
  }));
  for (const rankedAccount of rankedAccounts) countHealth(rankedAccount.health);
  for (const rankedSource of rankedSources) countHealth(rankedSource.health);
  const healthOptions = MEMBER_PICKER_HEALTH_ORDER
    .filter((health) => (healthCounts.get(health) ?? 0) > 0)
    .map((health) => ({ health, count: healthCounts.get(health) ?? 0 }));
  const availableHealth = healthOptions.map((option) => option.health).join("\n");
  const activeHealth: HealthFilter = healthFilter !== "all" && healthOptions.some((option) => option.health === healthFilter) ? healthFilter : "all";
  const matchesHealth = (health: MemberPickerHealth) => activeHealth === "all" || health === activeHealth;
  const accounts = rankedAccounts
    .filter((rankedAccount) => matchesHealth(rankedAccount.health))
    .sort((left, right) => compareMemberPickerHealth(left.health, right.health)
      || compareAccountPlans(accountPlanOption(left.account.subscription.planType, t("common.unknown")), accountPlanOption(right.account.subscription.planType, t("common.unknown")))
      || compareStableText(left.account.label, right.account.label))
    .map((rankedAccount) => rankedAccount.account);
  const sources = rankedSources
    .filter((rankedSource) => matchesHealth(rankedSource.health))
    .sort((left, right) => compareMemberPickerHealth(left.health, right.health) || compareStableText(left.source.name, right.source.name))
    .map((rankedSource) => rankedSource.source);
  // Do not submit members that were removed or added elsewhere during a refresh.
  const selectedAccounts = accountIds.filter((accountId) => allAccounts.some((account) => account.id === accountId));
  const selectedSources = sourceIds.filter((sourceId) => allSources.some((source) => source.id === sourceId));
  const selectedCount = selectedAccounts.length + selectedSources.length;
  const availableCount = allAccounts.length + allSources.length;
  const shownCount = accounts.length + sources.length;
  const checkedCount = accounts.filter((account) => accountIds.includes(account.id)).length + sources.filter((source) => sourceIds.includes(source.id)).length;
  const shownSelected = shownCount > 0 && checkedCount === shownCount;
  const toggleShown = () => {
    const mergeVisibleIds = (selectedIds: string[], visibleIds: string[]) => shownSelected
      ? selectedIds.filter((memberId) => !visibleIds.includes(memberId))
      : [...new Set([...selectedIds, ...visibleIds])];
    setAccountIds((previousAccountIds) => mergeVisibleIds(previousAccountIds, accounts.map((account) => account.id)));
    setSourceIds((previousSourceIds) => mergeVisibleIds(previousSourceIds, sources.map((source) => source.id)));
  };
  useEffect(() => { listRef.current?.scrollTo({ top: 0 }); }, [view, query, activePlan, activeHealth]);
  useEffect(() => {
    if (healthFilter !== "all" && !availableHealth.split("\n").includes(healthFilter)) setHealthFilter("all");
  }, [availableHealth, healthFilter]);
  const add = async (bypassWarning = false) => {
    if (addPending.current) return;
    addPending.current = true;
    const accountsToAdd = allAccounts.filter((account) => selectedAccounts.includes(account.id));
    const sourcesToAdd = [...selectedSources];
    try {
      const accepted = await confirmPoolAccounts(accountsToAdd, bypassWarning);
      const accountsToInclude = accountsToAdd.filter((account) => accepted || account.oauthClientKind === "excel_bps").map((account) => account.id);
      if (!accountsToInclude.length && !sourcesToAdd.length) return;
      const ok = await perform("pool-add-members", () => updatePoolMembership(mode, { accountIds: accountsToInclude, sourceIds: sourcesToAdd, inPool: true }), "feedback.saved", { backgroundRefresh: true });
      if (ok) onClose();
    } finally {
      addPending.current = false;
    }
  };
  const saving = busy === "pool-add-members";
  const requestClose = () => {
    if (!addPending.current) onClose();
  };
  const views: Array<{ id: MemberView; label: string; icon: ReactNode }> = [
    { id: "all", label: t("pool.allConnections"), icon: <Layers aria-hidden /> },
    { id: "accounts", label: t("connections.accounts"), icon: <UserRound aria-hidden /> },
    { id: "sources", label: t("connections.sources"), icon: <Server aria-hidden /> },
    { id: "selected", label: t("pool.selectedMembers"), icon: <CheckCheck aria-hidden /> },
  ];
  return <Dialog className="pool-add-dialog" title={t("pool.addMembersTitle")} onClose={requestClose} footer={<>
    <div className="pool-picker-summary" role="status">
      <span className="pool-picker-summary-icon" data-active={selectedCount > 0}><Check aria-hidden /></span>
      <span>{selectedCount ? t("pool.selectionCount", { count: selectedCount }) : t("pool.chooseMembers")}</span>
    </div>
    <Button variant="secondary" disabled={saving} onClick={requestClose}>{t("common.cancel")}</Button>
    <Button
      variant="primary"
      busy={saving}
      disabled={!selectedCount}
      aria-label={t("pool.addSelected", { count: selectedCount })}
      onClick={() => void add()}
      data-relay-context-action
      onContextMenu={(event) => {
        event.preventDefault();
        event.stopPropagation();
        if (selectedCount && !saving) void add(true);
      }}
    >
      {t("pool.confirmAdd")}
    </Button>
  </>}>
    <div className="pool-member-picker">
      <aside className="pool-picker-sidebar">
        <nav aria-label={t("pool.memberTypes")}>
          {views.map((viewOption) => (
            <button key={viewOption.id} type="button" aria-pressed={view === viewOption.id} onClick={() => setView(viewOption.id)}>
              {viewOption.icon}
              <span>{viewOption.label}</span>
              {viewOption.id === "selected" && selectedCount ? <small>{selectedCount}</small> : null}
            </button>
          ))}
        </nav>
        <Button
          className="pool-picker-new"
          variant="ghost"
          icon={<Plus aria-hidden />}
          aria-label={t("sources.addToPool")}
          disabled={!canAddSource || saving}
          title={!canAddSource ? t("remote.capabilityUnavailable") : undefined}
          onClick={onAddSource}
        >
          {t("pool.newSource")}
        </Button>
      </aside>
      <div className="pool-picker-content">
        <div className="pool-picker-toolbar">
          <label className="pool-picker-search">
            <Search aria-hidden />
            <input
              type="search"
              aria-label={t("pool.searchMembers")}
              value={query}
              onChange={(event) => setQuery(event.target.value)}
              placeholder={t("pool.searchMembersPlaceholder")}
            />
          </label>
          {view === "accounts" && plans.length > 1 ? <OptionMenu className="pool-picker-plan" label={t("accounts.filterByPlan")} value={activePlan} options={[
            { value: "all", label: t("accounts.allPlans") },
            ...plans.map((plan) => ({ value: plan.id, label: plan.label })),
          ]} onChange={setPlanFilter} /> : null}
          {healthOptions.length > 1 ? <div className="pool-picker-health" role="group" aria-label={t("pool.healthFilter")}>
            <button type="button" aria-pressed={activeHealth === "all"} data-health="all" onClick={() => setHealthFilter("all")}>
              <span>{t("pool.healthFilters.all")}</span>
              <small>{listedAccounts.length + listedSources.length}</small>
            </button>
            {healthOptions.map((option) => (
              <button key={option.health} type="button" aria-pressed={activeHealth === option.health} data-health={option.health} onClick={() => setHealthFilter(option.health)}>
                <span>{t(`pool.healthFilters.${option.health}`)}</span>
                <small>{option.count}</small>
              </button>
            ))}
          </div> : null}
        </div>
        <div className="pool-picker-selection">
          <label>
            <input
              type="checkbox"
              ref={(node) => { if (node) node.indeterminate = checkedCount > 0 && !shownSelected; }}
              checked={shownSelected}
              disabled={!shownCount || saving}
              onChange={toggleShown}
            />
            <span>{t("pool.selectVisible")}</span>
          </label>
          {selectedCount ? <Button variant="ghost" disabled={saving} onClick={() => { setAccountIds([]); setSourceIds([]); }}>{t("accounts.clearSelection")}</Button> : null}
        </div>
        <div ref={listRef} className="pool-picker-list">
          {accounts.length ? <section aria-label={t("connections.accounts")} className="pool-member-options">
            {accounts.map((account) => (
              <MemberOption
                key={account.id}
                {...accountPickerStatus(account, mode === "local" && Boolean(account.remoteLocation), t)}
                name={account.label}
                icon={<UserRound />}
                badge={<AccountBadges planType={account.subscription.planType} oauthClientKind={account.oauthClientKind} unknown={t("common.unknown")} />}
                checked={accountIds.includes(account.id)}
                disabled={saving}
                onChange={() => setAccountIds((previousAccountIds) => toggle(previousAccountIds, account.id))}
              />
            ))}
          </section> : null}
          {sources.length ? <section aria-label={t("connections.sources")} className="pool-member-options">
            {sources.map((source) => (
              <MemberOption
                key={source.id}
                {...sourcePickerStatus(source, t)}
                name={source.name}
                detail={source.baseUrl}
                icon={<Server />}
                checked={sourceIds.includes(source.id)}
                disabled={saving}
                onChange={() => setSourceIds((previousSourceIds) => toggle(previousSourceIds, source.id))}
              />
            ))}
          </section> : null}
          {!availableCount ? (
            <EmptyState title={t("pool.noAvailableMembers")} description={t("pool.noAvailableMembersHint")} />
          ) : !shownCount ? (
            <EmptyState
              title={t(view === "selected" && !selectedCount ? "pool.noSelectedMembers" : "pool.noMatchingMembers")}
              description={t(view === "selected" && !selectedCount ? "pool.noSelectedMembersHint" : "pool.noMatchingMembersHint")}
            />
          ) : null}
        </div>
      </div>
    </div>
  </Dialog>;
}
