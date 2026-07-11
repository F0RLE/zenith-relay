import { ReactNode, useEffect, useRef, useState } from "react";
import { CheckCircle2, CircleAlert, CircleHelp, Copy, Eye, EyeOff, Loader2, X } from "lucide-react";
import type { TFunction } from "i18next";
import { useTranslation } from "react-i18next";
import type { QuotaSnapshot, QuotaWindow } from "../api/types";

export function PageHeader({ title, subtitle, actions }: { title: string; subtitle?: string; actions?: ReactNode }) {
  return (
    <header className="relay-page-header">
      <div><h1>{title}</h1>{subtitle ? <p>{subtitle}</p> : null}</div>
      {actions ? <div className="relay-page-actions">{actions}</div> : null}
    </header>
  );
}

export function Button({ children, icon, variant = "secondary", busy, ...props }: React.ButtonHTMLAttributes<HTMLButtonElement> & { icon?: ReactNode; variant?: "primary" | "secondary" | "ghost" | "danger"; busy?: boolean }) {
  return <button className={`relay-button ${variant}`} {...props} disabled={busy || props.disabled}>{busy ? <Loader2 className="spin" aria-hidden /> : icon}<span>{children}</span></button>;
}

export function IconButton({ label, icon, ...props }: React.ButtonHTMLAttributes<HTMLButtonElement> & { label: string; icon: ReactNode }) {
  return <button className="relay-icon-button" aria-label={label} title={label} {...props}>{icon}</button>;
}

export function StatusBadge({ status, label }: { status: "ready" | "warning" | "error" | "info" | "disabled"; label: string }) {
  const Icon = status === "ready" ? CheckCircle2 : status === "error" ? CircleAlert : status === "warning" ? CircleAlert : CircleHelp;
  return <span className={`relay-status ${status}`}><Icon aria-hidden />{label}</span>;
}

export function Tabs({ value, items, onChange, label }: { value: string; items: Array<{ id: string; label: string }>; onChange: (id: string) => void; label: string }) {
  return <div className="relay-tabs" role="tablist" aria-label={label}>{items.map((item) => <button key={item.id} role="tab" aria-selected={value === item.id} className={value === item.id ? "active" : ""} onClick={() => onChange(item.id)} type="button">{item.label}</button>)}</div>;
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
  const reported = [
    ...(["primary", "secondary"] as const).flatMap((kind) => snapshot[kind] ? [{ id: kind, label: "", window: snapshot[kind] }] : []),
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
