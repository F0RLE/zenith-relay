import type { InputHTMLAttributes, ReactNode } from "react";
import { CircleHelp } from "lucide-react";

export function EmptyState({ title, description, action }: { title: string; description?: string; action?: ReactNode }) {
  return <div className="relay-empty"><CircleHelp aria-hidden /><strong>{title}</strong>{description ? <p>{description}</p> : null}{action}</div>;
}

export function ToggleSwitch({ label, checked, onChange, className = "", ...props }: Omit<InputHTMLAttributes<HTMLInputElement>, "type" | "checked" | "defaultChecked" | "onChange"> & {
  label: string;
  checked: boolean;
  onChange: (checked: boolean) => void;
}) {
  return <input {...props} className={`relay-switch${className ? ` ${className}` : ""}`} type="checkbox" checked={checked} aria-label={label} data-relay-tooltip={label} onChange={(event) => onChange(event.target.checked)} />;
}

export function SettingToggle({ label, description, checked, disabled = false, onChange, className = "", tone = "default" }: {
  label: string;
  description: string;
  checked: boolean;
  disabled?: boolean;
  onChange: (checked: boolean) => void;
  className?: string;
  tone?: "default" | "warning";
}) {
  return <label className={`setting-toggle ${tone}${className ? ` ${className}` : ""}`}>
    <span><strong>{label}</strong><small>{description}</small></span>
    <ToggleSwitch label={label} checked={checked} disabled={disabled} onChange={onChange} />
  </label>;
}

