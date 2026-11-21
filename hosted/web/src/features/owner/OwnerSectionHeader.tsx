import type { ReactNode } from "react";
import { useLocale } from "../../i18n/LocaleProvider";
import type { MessageKey } from "../../i18n/types";

export function OwnerSectionHeader<T extends string>({ title, group, tabs, tab, select, action }: {
  title: MessageKey; group: MessageKey;
  tabs: readonly (readonly [T, MessageKey])[]; tab: T; select: (tab: T) => void; action?: ReactNode;
}) {
  const { t } = useLocale();
  return <header className="relay-page-header relay-workspace-header owner-section-header">
    <h1>{t(title)}</h1>
    <div className="relay-page-navigation"><div className="relay-tabs" role="group" aria-label={t(group)}>
      {tabs.map(([id, label]) => <button key={id} className={tab === id ? "active" : ""} type="button"
        aria-pressed={tab === id} onClick={() => select(id)}>{t(label)}</button>)}
    </div></div>
    {action && <div className="relay-page-actions">{action}</div>}
  </header>;
}
