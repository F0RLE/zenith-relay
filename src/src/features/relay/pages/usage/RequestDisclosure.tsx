import type { ReactNode } from "react";
import { ChevronDown } from "lucide-react";

export function RequestDisclosure({ title, icon, badge, className = "", children }: {
  title: string;
  icon?: ReactNode;
  badge?: ReactNode;
  className?: string;
  children: ReactNode;
}) {
  return <details className={`request-disclosure ${className}`}>
    <summary>
      {icon ? <span className="request-disclosure-icon" aria-hidden>{icon}</span> : null}
      <span className="request-disclosure-title">{title}</span>
      {badge != null ? <span className="request-disclosure-badge">{badge}</span> : null}
      <ChevronDown className="request-disclosure-chevron" aria-hidden />
    </summary>
    <div className="request-disclosure-body">{children}</div>
  </details>;
}
