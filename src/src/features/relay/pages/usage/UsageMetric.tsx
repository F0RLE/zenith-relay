import type { ReactNode } from "react";

export function UsageMetric({ icon, label, value, detail, title, className }: {
  icon?: ReactNode;
  label: string;
  value: ReactNode;
  detail?: ReactNode;
  title?: string;
  className?: string;
}) {
  return <div className={className} title={title}>{icon}<div className="usage-metric-copy"><span>{label}</span><strong>{value}</strong>{detail ? <small>{detail}</small> : null}</div></div>;
}
