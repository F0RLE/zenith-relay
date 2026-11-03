import type { ComponentType } from "react";
import { Tabs } from "../../components/Ui";

export type IntegrationOption = {
  id: string;
  label: string;
  icon: string;
  Component: ComponentType;
};

export function IntegrationWorkspace({ items, value, onChange, label, showPickerWhenSingle = false, className = "" }: {
  items: readonly IntegrationOption[];
  value: string;
  onChange: (value: string) => void;
  label: string;
  showPickerWhenSingle?: boolean;
  className?: string;
}) {
  const selected = items.find((item) => item.id === value) ?? items[0];
  if (!selected) return null;
  const Settings = selected.Component;
  const showPicker = showPickerWhenSingle || items.length >= 2;
  return <div className={`integration-workspace${showPicker ? " has-picker" : ""} ${className}`.trim()}>
    {showPicker ? <div className="integration-picker">
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
      <Settings key={selected.id} />
    </div>
  </div>;
}
