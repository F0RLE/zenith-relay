import { useId } from "react";
import { OptionMenu } from "../../components/OptionMenu";

type Choice = { value: string; label: string; shortLabel?: string };

export function CatalogChoice({ label, description, value, options, onChange }: {
  label: string;
  description?: string;
  value: string;
  options: Choice[];
  onChange: (value: string) => void;
}) {
  const hint = useId();
  if (!options.length) return null;
  const selected = options.find(option => option.value === value) ?? options[0]!;

  return <div className="catalog-choice" role="group" aria-label={label} aria-describedby={description ? hint : undefined} title={description}>
    {description && <span id={hint} className="sr-only">{description}</span>}
    {options.length === 1
      ? <span className="catalog-static-choice" aria-label={selected.label}>{selected.shortLabel ?? selected.label}</span>
      : options.length === 2
        ? <div className="catalog-choice-buttons">{options.map(option => <button
            key={option.value} type="button" aria-pressed={option.value === value}
            aria-label={option.label} aria-describedby={description ? hint : undefined}
            onClick={() => onChange(option.value)}
          >{option.shortLabel ?? option.label}</button>)}</div>
        : <OptionMenu label={label} value={value} options={options} onChange={onChange} triggerClassName="catalog-choice-trigger" />}
  </div>;
}
