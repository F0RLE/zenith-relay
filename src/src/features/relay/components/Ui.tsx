import { ReactNode, useEffect, useRef, useState } from "react";
import { CheckCircle2, CircleAlert, CircleHelp, CircleOff, Copy, Eye, EyeOff, Loader2, MoreHorizontal, X } from "lucide-react";
import type { TFunction } from "i18next";
import { useTranslation } from "react-i18next";
import type { AccountSummary, QuotaSnapshot, QuotaWindow } from "../api/types";

export function isCodexOauthAccountEligible(account: AccountSummary) {
  const authState = typeof account.authState === "string" ? account.authState : account.authState.state;
  return account.enabled && account.inPool && !account.draining && account.secretAvailable && !account.routingExclusion && authState === "active";
}

export function formatAccountPlan(planType: string | null, unknown: string) {
  const value = planType?.trim();
  if (!value) return unknown;
  const key = value.toLocaleLowerCase().replace(/[\s_-]/g, "");
  if (key.includes("team") || key.includes("business")) return "Business";
  if (key.includes("enterprise")) return "Enterprise";
  if (key === "prolite") return "Pro 5x";
  if (key === "promax") return "Pro 20x";
  if (key === "pro") return "Pro";
  if (key.includes("plus")) return "Plus";
  if (key === "free") return "Free";
  if (key === "go") return "Go";
  if (key === "edu" || key.includes("education")) return "Edu";
  return value;
}

const accountPlanOrder = ["plus", "pro", "pro-5x", "pro-20x", "business", "enterprise", "free", "go", "edu", "unknown"];

export function accountPlanOption(planType: string | null, unknown: string) {
  const label = formatAccountPlan(planType, unknown);
  return { id: label.toLocaleLowerCase().replace(/[^a-z0-9]+/g, "-").replace(/^-|-$/g, "") || "unknown", label };
}

export function compareAccountPlans(left: { id: string; label: string }, right: { id: string; label: string }) {
  const leftRank = accountPlanOrder.indexOf(left.id);
  const rightRank = accountPlanOrder.indexOf(right.id);
  return (leftRank < 0 ? accountPlanOrder.length : leftRank) - (rightRank < 0 ? accountPlanOrder.length : rightRank) || left.label.localeCompare(right.label);
}

export type ApiSourceRole = "primary" | "stabilizer" | "reserve";

const API_SOURCE_PRIMARY_PRIORITY = 1_000_000;
const API_SOURCE_RESERVE_PRIORITY = -1_000_000;

export function apiSourceRole(priority: number): ApiSourceRole {
  if (priority >= API_SOURCE_PRIMARY_PRIORITY) return "primary";
  if (priority <= API_SOURCE_RESERVE_PRIORITY) return "reserve";
  return "stabilizer";
}

export function apiSourcePriority(role: ApiSourceRole) {
  if (role === "primary") return API_SOURCE_PRIMARY_PRIORITY;
  if (role === "reserve") return API_SOURCE_RESERVE_PRIORITY;
  return 0;
}

export function PageHeader({ title, subtitle, actions }: { title: string; subtitle?: string; actions?: ReactNode }) {
  return (
    <header className="relay-page-header">
      <div><h1>{title}</h1>{subtitle ? <p>{subtitle}</p> : null}</div>
      {actions ? <div className="relay-page-actions">{actions}</div> : null}
    </header>
  );
}

export function Button({ children, icon, variant = "secondary", busy, ...props }: React.ButtonHTMLAttributes<HTMLButtonElement> & { icon?: ReactNode; variant?: "primary" | "secondary" | "ghost" | "danger"; busy?: boolean }) {
  return <button type={props.type ?? "button"} className={`relay-button ${variant}`} {...props} disabled={busy || props.disabled}>{busy ? <Loader2 className="spin" aria-hidden /> : icon}<span>{children}</span></button>;
}

export function IconButton({ label, icon, ...props }: React.ButtonHTMLAttributes<HTMLButtonElement> & { label: string; icon: ReactNode }) {
  return <button type={props.type ?? "button"} className="relay-icon-button" aria-label={label} title={label} {...props}>{icon}</button>;
}

export function ActionMenu({ children, className = "", label }: { children: ReactNode; className?: string; label?: string }) {
  const { t } = useTranslation();
  const resolvedLabel = label ?? t("common.actions");
  return <details className={`relay-action-menu ${className}`.trim()}><summary aria-label={resolvedLabel} title={resolvedLabel} aria-haspopup="menu"><MoreHorizontal aria-hidden /></summary><div role="menu">{children}</div></details>;
}

export function ActionMenuItem({ children, icon, danger = false, className = "", onClick, ...props }: React.ButtonHTMLAttributes<HTMLButtonElement> & { icon: ReactNode; danger?: boolean }) {
  const classes = [danger ? "danger" : "", className].filter(Boolean).join(" ");
  return <button type="button" role="menuitem" className={classes || undefined} {...props} onClick={(event) => { const menu = event.currentTarget.closest("details"); if (menu) menu.open = false; onClick?.(event); }}>{icon}<span>{children}</span></button>;
}

export function StatusBadge({ status, label }: { status: "ready" | "warning" | "error" | "info" | "disabled"; label: string }) {
  const Icon = status === "ready" ? CheckCircle2 : status === "disabled" ? CircleOff : status === "info" ? CircleHelp : CircleAlert;
  return <span className={`relay-status ${status}`}><Icon aria-hidden />{label}</span>;
}

export function Tabs({ value, items, onChange, label }: { value: string; items: Array<{ id: string; label: string }>; onChange: (id: string) => void; label: string }) {
  const selectAdjacent = (event: React.KeyboardEvent<HTMLButtonElement>, index: number) => {
    const direction = event.key === "ArrowRight" ? 1 : event.key === "ArrowLeft" ? -1 : 0;
    const nextIndex = event.key === "Home" ? 0 : event.key === "End" ? items.length - 1 : direction ? (index + direction + items.length) % items.length : -1;
    if (nextIndex < 0) return;
    event.preventDefault();
    onChange(items[nextIndex].id);
    event.currentTarget.parentElement?.querySelectorAll<HTMLButtonElement>('[role="tab"]')[nextIndex]?.focus();
  };
  return <div className="relay-tabs" role="tablist" aria-label={label}>{items.map((item, index) => <button key={item.id} role="tab" aria-selected={value === item.id} tabIndex={value === item.id ? 0 : -1} className={value === item.id ? "active" : ""} onClick={() => onChange(item.id)} onKeyDown={(event) => selectAdjacent(event, index)} type="button">{item.label}</button>)}</div>;
}

export function Dialog({ title, children, onClose, footer, wide = false }: { title: string; children: ReactNode; onClose: () => void; footer: ReactNode; wide?: boolean }) {
  const { t } = useTranslation();
  const dialogRef = useRef<HTMLElement>(null);
  useEffect(() => {
    const previouslyFocused = document.activeElement as HTMLElement | null;
    const focusable = () => Array.from(dialogRef.current?.querySelectorAll<HTMLElement>('button:not([disabled]), input:not([disabled]), select:not([disabled]), textarea:not([disabled]), [tabindex]:not([tabindex="-1"])') ?? []);
    focusable()[0]?.focus();
    const onKey = (event: KeyboardEvent) => {
      if (event.key === "Escape") onClose();
      if (event.key !== "Tab") return;
      const items = focusable();
      if (!items.length) return;
      const first = items[0];
      const last = items[items.length - 1];
      if (event.shiftKey && document.activeElement === first) { event.preventDefault(); last.focus(); }
      else if (!event.shiftKey && document.activeElement === last) { event.preventDefault(); first.focus(); }
    };
    window.addEventListener("keydown", onKey);
    return () => { window.removeEventListener("keydown", onKey); previouslyFocused?.focus(); };
  }, [onClose]);
  return <div className="relay-modal-backdrop" role="presentation"><section ref={dialogRef} className={`relay-dialog ${wide ? "wide" : ""}`} role="dialog" aria-modal="true" aria-labelledby="relay-dialog-title"><header><h2 id="relay-dialog-title">{title}</h2><IconButton label={t("common.close")} icon={<X aria-hidden />} onClick={onClose} /></header><div className="relay-dialog-body">{children}</div><footer>{footer}</footer></section></div>;
}

export function EmptyState({ title, description, action }: { title: string; description: string; action?: ReactNode }) {
  return <div className="relay-empty"><CircleHelp aria-hidden /><strong>{title}</strong><p>{description}</p>{action}</div>;
}

export function QuotaMeter({ window, kind, label }: { window: QuotaWindow | null; kind?: "primary" | "secondary"; label?: string }) {
  const { i18n, t } = useTranslation();
  const windowKind = kind ?? window?.kind ?? "primary";
  const resolvedLabel = label ?? quotaWindowLabel(window, windowKind, t);
  if (!window?.availableBasisPoints && window?.availableBasisPoints !== 0) {
    const unavailable = window ? t("common.unknown") : t("quota.notReported");
    return <div className="quota-meter unavailable"><div className="quota-meter-heading"><span>{resolvedLabel}</span><small title={unavailable}>{unavailable}</small><strong>-</strong></div><div className="quota-track" aria-label={`${resolvedLabel}: ${unavailable}`} /></div>;
  }
  const percent = Math.round(window.availableBasisPoints / 100);
  const reset = window.resetAtMs ? new Intl.DateTimeFormat(i18n.language, { dateStyle: "short", timeStyle: "short" }).format(new Date(window.resetAtMs)) : t("common.unknown");
  const resetLabel = t("quota.reset", { value: reset });
  return <div className="quota-meter"><div className="quota-meter-heading"><span>{resolvedLabel}</span><small title={resetLabel}>{resetLabel}</small><strong>{percent}%</strong></div><div className="quota-track" aria-label={`${resolvedLabel} ${percent}%`}><span style={{ width: `${percent}%` }} /></div></div>;
}

export function QuotaStack({ snapshot }: { snapshot: QuotaSnapshot }) {
  const { t } = useTranslation();
  const coreBlocked = [snapshot.primary, snapshot.secondary]
    .some((window) => window?.availableBasisPoints === 0);
  const reported = [
    ...(["primary", "secondary"] as const).flatMap((kind) => {
      const window = snapshot[kind];
      if (!window) return [];
      return [{ id: kind, label: "", window: coreBlocked ? { ...window, availableBasisPoints: 0 } : window }];
    }),
    ...(snapshot.supplemental ?? []),
  ];
  if (!reported.length) return <div className="quota-stack"><QuotaMeter window={null} /></div>;
  return <div className="quota-stack">{reported.map((item) => <QuotaMeter key={item.id} window={item.window} label={item.label ? `${item.label} · ${quotaWindowLabel(item.window, item.window.kind, t)}` : undefined} />)}</div>;
}

export function quotaWindowLabel(window: QuotaWindow | null, kind: "primary" | "secondary", t: TFunction) {
  const minutes = window?.windowMinutes;
  if (!minutes) return t(`quota.${kind}`);
  const weeks = Math.round(minutes / 10_080);
  if (weeks > 0 && Math.abs(minutes - weeks * 10_080) <= 1) return weeks === 1 ? t("quota.week") : t("quota.weeks", { count: weeks });
  if (minutes % 1_440 === 0) return t("quota.days", { count: minutes / 1_440 });
  if (minutes % 60 === 0) return t("quota.hours", { count: minutes / 60 });
  return t("quota.minutes", { count: minutes });
}

export function SecretField({ label, value, onChange, placeholder }: { label: string; value: string; onChange: (value: string) => void; placeholder?: string }) {
  const { t } = useTranslation();
  const [visible, setVisible] = useState(false);
  return <label className="relay-field"><span>{label}</span><div className="secret-field"><input type={visible ? "text" : "password"} value={value} onChange={(event) => onChange(event.target.value)} placeholder={placeholder} autoComplete="off" spellCheck={false} /><IconButton label={visible ? t("common.hide") : t("common.reveal")} icon={visible ? <EyeOff aria-hidden /> : <Eye aria-hidden />} onClick={() => setVisible((current) => !current)} type="button" /></div></label>;
}

export async function copyText(value: string) {
  await navigator.clipboard.writeText(value);
}

export function CopyButton({ value, label }: { value: string; label: string }) {
  const [copied, setCopied] = useState(false);
  return <IconButton label={copied ? `${label}: copied` : label} icon={copied ? <CheckCircle2 aria-hidden /> : <Copy aria-hidden />} onClick={async () => { await copyText(value); setCopied(true); window.setTimeout(() => setCopied(false), 1500); }} />;
}
