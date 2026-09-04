import type { LucideIcon } from "lucide-react";
import { SettingToggle } from "./Ui";

type CodexFeatureToggleControlProps = {
  className?: string;
  styleClassPrefix: string;
  icon: LucideIcon;
  title: string;
  hint: string;
  label: string;
  description: string;
  checked: boolean;
  disabled: boolean;
  onChange: (enabled: boolean) => void;
};

/** Shared visual shell for Codex runtime feature toggles. */
export function CodexFeatureToggleControl({
  className = "",
  styleClassPrefix,
  icon: Icon,
  title,
  hint,
  label,
  description,
  checked,
  disabled,
  onChange,
}: CodexFeatureToggleControlProps) {
  return <section className={`${styleClassPrefix}-control${className ? ` ${className}` : ""}`}>
    <div className={`${styleClassPrefix}-heading`}>
      <span className={`${styleClassPrefix}-icon`}><Icon aria-hidden /></span>
      <div>
        <h2>{title}</h2>
        <p>{hint}</p>
      </div>
    </div>
    <SettingToggle
      className={`${styleClassPrefix}-toggle`}
      label={label}
      description={description}
      checked={checked}
      disabled={disabled}
      onChange={onChange}
    />
  </section>;
}
