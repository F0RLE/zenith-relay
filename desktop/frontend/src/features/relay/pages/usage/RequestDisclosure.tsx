import { useState, type ReactNode } from "react";
import { ChevronDown } from "lucide-react";

export function RequestDisclosure({ title, className, children }: {
  title: string;
  className: string;
  children: () => ReactNode;
}) {
  const [open, setOpen] = useState(false);
  return <details className={`request-disclosure ${className}`} onToggle={(event) => setOpen(event.currentTarget.open)}>
    <summary>
      <span className="request-disclosure-title">{title}</span>
      <ChevronDown className="request-disclosure-chevron" aria-hidden />
    </summary>
    {open ? <div className="request-disclosure-body">{children()}</div> : null}
  </details>;
}
