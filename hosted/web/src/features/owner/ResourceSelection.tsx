import { useLayoutEffect, useRef } from "react";
import { Check, Minus, Square } from "../../components/Icons";

/** Shared visual treatment; native checkbox semantics and keyboard behavior stay intact. */
export function ResourceSelection({ label, checked, mixed = false, disabled = false, onChange }: {
  label: string; checked: boolean; mixed?: boolean; disabled?: boolean; onChange: () => void;
}) {
  const input = useRef<HTMLInputElement>(null);
  useLayoutEffect(() => { if (input.current) input.current.indeterminate = mixed; }, [mixed]);
  return <label className="owner-resource-select" data-state={mixed ? "mixed" : checked ? "checked" : "unchecked"}>
    <input ref={input} type="checkbox" aria-label={label} checked={checked} disabled={disabled} onChange={onChange} />
    <span className="owner-selection-mark" aria-hidden="true">{mixed ? <Minus /> : checked ? <Check /> : <Square />}</span>
  </label>;
}
