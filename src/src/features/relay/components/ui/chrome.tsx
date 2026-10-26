import type { ReactNode } from "react";
import type { TFunction } from "i18next";
import { useTranslation } from "react-i18next";
import type { OAuthClientKind } from "../../api/types";
import { accountErrorTranslationKey } from "../../accountStatus";
import { accountPlanOption } from "../../accountPlans";

export function accountErrorLabel(code: string, t: TFunction) {
  return t(`usage.errorCategories.${code.trim().toLowerCase()}`, {
    defaultValue: t(accountErrorTranslationKey(code)),
  });
}

export function AccountPlanBadge({ planType, unknown, hint, stale }: { planType: string | null; unknown: string; hint?: string | undefined; stale?: boolean | undefined }) {
  const plan = accountPlanOption(planType, unknown);
  return <span className="account-plan-badge" data-plan={plan.id} data-relay-tooltip={hint} data-stale={hint ? stale : undefined}
    role={hint ? "note" : undefined} tabIndex={hint ? 0 : undefined} aria-label={hint ? `${plan.label}. ${hint}` : undefined}>
    {plan.label}
  </span>;
}

export function AccountBadges({ planType, oauthClientKind, unknown, routeControl, planHint, planStale, poolScope = false, codexTextModelsEnabled = false }: {
  planType: string | null;
  oauthClientKind?: OAuthClientKind | undefined;
  unknown: string;
  routeControl?: ReactNode;
  planHint?: string | undefined;
  planStale?: boolean | undefined;
  poolScope?: boolean | undefined;
  codexTextModelsEnabled?: boolean | undefined;
}) {
  const { t } = useTranslation();
  const client = oauthClientKind === "excel_bps" ? "BPS" : oauthClientKind === "siwc" ? "SIWC" : "Codex";
  const label = client === "BPS" ? "Excel · Basis Points" : client === "SIWC" ? "ChatGPT · SIWC" : "Codex";
  return <div className="account-badges">
    <AccountPlanBadge planType={planType} unknown={oauthClientKind === "siwc" ? t("accounts.siwcPlanUnavailable") : unknown} hint={planHint} stale={planStale} />
    {routeControl ?? <span className="account-client-badge" data-relay-tooltip={label} aria-label={label}>{client}</span>}
    {poolScope && oauthClientKind === "codex" && !codexTextModelsEnabled ? <span className="account-client-badge">{t("accounts.imagesOnly")}</span> : null}
  </div>;
}

export function PageHeader({ title, actions, navigation, workspace = false }: { title: string; actions?: ReactNode; navigation?: ReactNode; workspace?: boolean }) {
  return (
    <header className={`relay-page-header${navigation || workspace ? " relay-workspace-header" : ""}`}>
      <div><h1>{title}</h1></div>
      {actions ? <div className="relay-page-actions">{actions}</div> : null}
      {navigation ? <div className="relay-page-navigation">{navigation}</div> : null}
    </header>
  );
}


export function Tabs({ value, items, onChange, label, orientation = "horizontal" }: {
  value: string;
  items: Array<{ id: string; label: string; icon?: ReactNode }>;
  onChange: (itemId: string) => void;
  label: string;
  orientation?: "horizontal" | "vertical";
}) {
  const selectAdjacent = (event: React.KeyboardEvent<HTMLButtonElement>, index: number) => {
    const nextKey = orientation === "vertical" ? "ArrowDown" : "ArrowRight";
    const previousKey = orientation === "vertical" ? "ArrowUp" : "ArrowLeft";
    const direction = event.key === nextKey ? 1 : event.key === previousKey ? -1 : 0;
    const nextIndex = event.key === "Home" ? 0 : event.key === "End" ? items.length - 1 : direction ? (index + direction + items.length) % items.length : -1;
    if (nextIndex < 0) return;
    event.preventDefault();
    const nextItem = items[nextIndex];
    if (!nextItem) return;
    onChange(nextItem.id);
    event.currentTarget.parentElement?.querySelectorAll<HTMLButtonElement>('[role="tab"]')[nextIndex]?.focus();
  };
  return <div className="relay-tabs" role="tablist" aria-label={label} aria-orientation={orientation}>{items.map((item, index) => (
    <button key={item.id} role="tab" aria-label={item.label} aria-selected={value === item.id} tabIndex={value === item.id ? 0 : -1} className={value === item.id ? "active" : ""} onClick={() => onChange(item.id)} onKeyDown={(event) => selectAdjacent(event, index)} type="button">
      {item.icon}
      {item.label}
    </button>
  ))}</div>;
}
