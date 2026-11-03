import { useId, type ReactNode } from "react";

export function RequestSection({ title, badge, className = "", children }: {
  title: string;
  badge?: ReactNode;
  className?: string;
  children: ReactNode;
}) {
  const titleId = useId();
  return <section className={`request-detail-section ${className}`} aria-labelledby={titleId}>
    <div className="request-section-heading">
      <h3 id={titleId}>{title}</h3>
      {badge != null ? <span className="request-section-badge">{badge}</span> : null}
    </div>
    {children}
  </section>;
}
