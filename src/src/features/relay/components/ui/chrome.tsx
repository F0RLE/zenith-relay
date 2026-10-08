import type { ReactNode } from "react";
import type { TFunction } from "i18next";
import { accountErrorTranslationKey } from "../../accountStatus";
import { accountPlanOption } from "../../accountPlans";

export function accountErrorLabel(code: string, t: TFunction) {
  return t(accountErrorTranslationKey(code));
}

export function AccountPlanBadge({ planType, unknown }: { planType: string | null; unknown: string }) {
  const plan = accountPlanOption(planType, unknown);
  return <span className="account-plan-badge" data-plan={plan.id}>{plan.label}</span>;
}

export function PageHeader({ title, subtitle, actions, navigation, workspace = false }: { title: string; subtitle?: string; actions?: ReactNode; navigation?: ReactNode; workspace?: boolean }) {
  return (
    <header className={`relay-page-header${navigation || workspace ? " relay-workspace-header" : ""}`}>
      <div><h1>{title}</h1>{subtitle ? <p>{subtitle}</p> : null}</div>
      {actions ? <div className="relay-page-actions">{actions}</div> : null}
      {navigation ? <div className="relay-page-navigation">{navigation}</div> : null}
    </header>
  );
}


export function Tabs({ value, items, onChange, label }: { value: string; items: Array<{ id: string; label: string }>; onChange: (itemId: string) => void; label: string }) {
  const selectAdjacent = (event: React.KeyboardEvent<HTMLButtonElement>, index: number) => {
    const direction = event.key === "ArrowRight" ? 1 : event.key === "ArrowLeft" ? -1 : 0;
    const nextIndex = event.key === "Home" ? 0 : event.key === "End" ? items.length - 1 : direction ? (index + direction + items.length) % items.length : -1;
    if (nextIndex < 0) return;
    event.preventDefault();
    const nextItem = items[nextIndex];
    if (!nextItem) return;
    onChange(nextItem.id);
    event.currentTarget.parentElement?.querySelectorAll<HTMLButtonElement>('[role="tab"]')[nextIndex]?.focus();
  };
  return <div className="relay-tabs" role="tablist" aria-label={label}>{items.map((item, index) => <button key={item.id} role="tab" aria-selected={value === item.id} tabIndex={value === item.id ? 0 : -1} className={value === item.id ? "active" : ""} onClick={() => onChange(item.id)} onKeyDown={(event) => selectAdjacent(event, index)} type="button">{item.label}</button>)}</div>;
}
