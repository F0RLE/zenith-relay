import type { ComponentType } from "react";
import { Tabs } from "../../components/Ui";

export type IntegrationOption = {
  id: string;
  label: string;
  icon: string;
  Component: ComponentType;
};

export function IntegrationWorkspace({ items, value, onChange, label }: {
  items: readonly IntegrationOption[];
  value: string;
  onChange: (value: string) => void;
  label: string;
}) {
  const selected = items.find((item) => item.id === value) ?? items[0];
  if (!selected) return null;
  const Settings = selected.Component;
  const hasChoices = items.length >= 2;
  return <div className={`integration-workspace${hasChoices ? " has-choices" : ""}`}>
    {hasChoices ? <div className="integration-picker">
      <Tabs
        value={selected.id}
        onChange={onChange}
        orientation="vertical"
        label={label}
        items={items.map(({ id, label: name, icon }) => ({
          id,
          label: name,
          icon: <img src={icon} alt="" />,
        }))}
      />
    </div> : null}
    <div className="integrations-settings">
      {hasChoices ? <div className="integration-detail-heading">
        <span className="integration-detail-icon"><img src={selected.icon} alt="" /></span>
        <h3>{selected.label}</h3>
      </div> : null}
      <Settings />
    </div>
  </div>;
}
