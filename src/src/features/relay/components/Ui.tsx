import { createContext, ReactNode, useCallback, useContext, useEffect, useId, useLayoutEffect, useRef, useState } from "react";
import { createPortal } from "react-dom";
import { Check, CheckCircle2, ChevronDown, CircleAlert, CircleHelp, CircleOff, Copy, Eye, EyeOff, Loader2, MoreHorizontal, X } from "lucide-react";
import type { TFunction } from "i18next";
import { useTranslation } from "react-i18next";
import type { AccountSummary, QuotaSnapshot, QuotaWindow } from "../api/types";
import { accountErrorTranslationKey } from "../accountStatus";
import { buildAccountValueProjection } from "../accountEconomics";
import { formatAccountValueMicroUsd } from "../poolFormatting";
import { formatDetailedRemainingTime, quotaWindowLabel } from "../quotaFormatting";
import { accountPlanOption, apiSourcePriority, apiSourceRole, compareAccountPlans, formatAccountPlan, type ApiSourceRole } from "../routingOrder";

export { formatDetailedRemainingTime, formatRemainingTime, quotaWindowLabel } from "../quotaFormatting";

export { accountPlanOption, apiSourcePriority, apiSourceRole, compareAccountPlans, formatAccountPlan };
export type { ApiSourceRole };
// Compatibility exports keep existing feature and test imports stable while
// the status policy itself lives in its domain module.
export {
  currentAccountErrorCode,
  isCodexOauthAccountEligible,
  operationalStatusTone,
  transientCandidateTone,
} from "../accountStatus";

export function accountErrorLabel(code: string, t: TFunction) {
  return t(accountErrorTranslationKey(code));
}

export function AccountPlanBadge({ planType, unknown }: { planType: string | null; unknown: string }) {
  const plan = accountPlanOption(planType, unknown);
  return <span className="account-plan-badge" data-plan={plan.id}>{plan.label}</span>;
}

export function AccountValueStrip({ account }: { account: AccountSummary }) {
  const { t, i18n } = useTranslation();
  const locale = i18n.resolvedLanguage ?? i18n.language;
  const { purchaseCostMicroUsd: purchaseCost, potential, payback, approximate } = buildAccountValueProjection(account.apiEquivalent, account.quota, account.purchaseCostMicroUsd);
  const paybackTitle = purchaseCost == null
    ? t("accounts.accountValue.purchaseMissing")
    : t("accounts.accountValue.paybackHint", {
      used: formatAccountValueMicroUsd(account.apiEquivalent.microUsd, locale),
      purchase: formatAccountValueMicroUsd(purchaseCost, locale),
    });
  return <dl className="account-value-strip">
    <div title={t("accounts.accountValue.usedHint", { count: account.apiEquivalent.unpricedTokens })}><dt>{t("accounts.accountValue.used")}</dt><dd>{formatAccountValueMicroUsd(account.apiEquivalent.microUsd, locale, approximate)}</dd></div>
    <div title={t("accounts.accountValue.potentialHint")}><dt>{t("accounts.accountValue.potential")}</dt><dd>{potential == null ? "—" : formatAccountValueMicroUsd(potential.microUsd, locale, potential.approximate)}</dd></div>
    <div title={paybackTitle} data-state={payback != null && payback >= 1 ? "paid" : undefined}><dt>{t("accounts.accountValue.payback")}</dt><dd>{payback == null ? "—" : `${approximate ? "≈" : ""}${new Intl.NumberFormat(locale, { style: "percent", maximumFractionDigits: 0 }).format(payback)}`}</dd></div>
  </dl>;
}

export function PageHeader({ title, subtitle, actions }: { title: string; subtitle?: string; actions?: ReactNode }) {
  return (
    <header className="relay-page-header">
      <div><h1>{title}</h1>{subtitle ? <p>{subtitle}</p> : null}</div>
      {actions ? <div className="relay-page-actions">{actions}</div> : null}
    </header>
  );
}

type ConfirmOptions = { title?: string; confirmLabel?: string; danger?: boolean };
type ConfirmRequest = ConfirmOptions & { message: string };
type ConfirmHandler = (message: string, options?: ConfirmOptions) => Promise<boolean>;

const ConfirmContext = createContext<ConfirmHandler | null>(null);

export function ConfirmProvider({ children }: { children: ReactNode }) {
  const { t } = useTranslation();
  const [request, setRequest] = useState<ConfirmRequest | null>(null);
  const resolver = useRef<((accepted: boolean) => void) | null>(null);
  const confirm = useCallback<ConfirmHandler>((message, options = {}) => new Promise((resolve) => {
    resolver.current?.(false);
    resolver.current = resolve;
    setRequest({ message, ...options });
  }), []);
  const settle = useCallback((accepted: boolean) => {
    const resolve = resolver.current;
    resolver.current = null;
    setRequest(null);
    resolve?.(accepted);
  }, []);
  useEffect(() => () => resolver.current?.(false), []);
  return <ConfirmContext.Provider value={confirm}>
    {children}
    {request ? <Dialog
      title={request.title ?? t("common.confirmationTitle")}
      onClose={() => settle(false)}
      footer={<><Button variant="secondary" onClick={() => settle(false)}>{t("common.cancel")}</Button><Button variant={request.danger ? "danger" : "primary"} onClick={() => settle(true)}>{request.confirmLabel ?? t("common.confirm")}</Button></>}
    ><p className="confirm-dialog-message">{request.message}</p></Dialog> : null}
  </ConfirmContext.Provider>;
}

export function useConfirm() {
  const confirm = useContext(ConfirmContext);
  if (!confirm) throw new Error("ConfirmProvider is missing");
  return confirm;
}

export function Button({ children, icon, variant = "secondary", busy, className, ...props }: React.ButtonHTMLAttributes<HTMLButtonElement> & { icon?: ReactNode; variant?: "primary" | "secondary" | "ghost" | "danger"; busy?: boolean }) {
  return <button type={props.type ?? "button"} className={`relay-button ${variant}${className ? ` ${className}` : ""}`} {...props} disabled={busy || props.disabled}>{busy ? <Loader2 className="spin" aria-hidden /> : icon}<span>{children}</span></button>;
}

function useTooltip<T extends HTMLElement>(label: string) {
  const anchorRef = useRef<T>(null);
  const tooltipRef = useRef<HTMLDivElement>(null);
  const pointerDown = useRef(false);
  const tooltipId = useId();
  const [visible, setVisible] = useState(false);
  const [position, setPosition] = useState<{ left: number; top: number; placement: "top" | "bottom" } | null>(null);

  const show = () => {
    setPosition(null);
    setVisible(true);
  };
  const hide = () => setVisible(false);
  const pointerStart = () => {
    pointerDown.current = true;
    hide();
    window.setTimeout(() => { pointerDown.current = false; }, 0);
  };

  useLayoutEffect(() => {
    if (!visible) return;
    const anchor = anchorRef.current?.getBoundingClientRect();
    const tooltip = tooltipRef.current;
    if (!anchor || !tooltip) return;
    const margin = 9;
    const gap = 8;
    let placement: "top" | "bottom" = "bottom";
    let top = anchor.bottom + gap;
    if (top + tooltip.offsetHeight > window.innerHeight - margin && anchor.top - tooltip.offsetHeight - gap >= margin) {
      placement = "top";
      top = anchor.top - tooltip.offsetHeight - gap;
    }
    const centered = anchor.left + anchor.width / 2 - tooltip.offsetWidth / 2;
    const left = Math.max(margin, Math.min(centered, window.innerWidth - tooltip.offsetWidth - margin));
    setPosition({ left, top, placement });
  }, [label, visible]);

  useEffect(() => {
    if (!visible) return;
    const onKeyDown = (event: KeyboardEvent) => { if (event.key === "Escape") hide(); };
    window.addEventListener("resize", hide);
    window.addEventListener("scroll", hide, true);
    document.addEventListener("keydown", onKeyDown);
    return () => {
      window.removeEventListener("resize", hide);
      window.removeEventListener("scroll", hide, true);
      document.removeEventListener("keydown", onKeyDown);
    };
  }, [visible]);

  const tooltip = visible && typeof document !== "undefined" ? createPortal(
    <div
      ref={tooltipRef}
      id={tooltipId}
      className="relay-tooltip"
      role="tooltip"
      data-placement={position?.placement}
      data-positioned={Boolean(position)}
      style={position ? { left: position.left, top: position.top } : undefined}
    >
      {label}
    </div>,
    document.body,
  ) : null;

  return {
    anchorRef,
    describedBy: visible ? tooltipId : undefined,
    hide,
    hideAfterHover: () => { if (document.activeElement !== anchorRef.current) hide(); },
    show,
    showAfterFocus: () => { if (!pointerDown.current && anchorRef.current?.matches(":focus-visible")) show(); },
    pointerStart,
    tooltip,
  };
}

export function IconButton({ label, icon, className = "", title, onMouseEnter, onMouseLeave, onFocus, onBlur, onPointerDown, ...props }: React.ButtonHTMLAttributes<HTMLButtonElement> & { label: string; icon: ReactNode }) {
  const tooltip = useTooltip<HTMLButtonElement>(title ?? label);
  return <>
    <button
      ref={tooltip.anchorRef}
      type={props.type ?? "button"}
      className={`relay-icon-button ${className}`.trim()}
      aria-label={label}
      aria-describedby={tooltip.describedBy}
      title={props.disabled ? title ?? label : undefined}
      {...props}
      onMouseEnter={(event) => { tooltip.show(); onMouseEnter?.(event); }}
      onMouseLeave={(event) => { tooltip.hideAfterHover(); onMouseLeave?.(event); }}
      onFocus={(event) => { tooltip.showAfterFocus(); onFocus?.(event); }}
      onBlur={(event) => { tooltip.hide(); onBlur?.(event); }}
      onPointerDown={(event) => { tooltip.pointerStart(); onPointerDown?.(event); }}
    >
      {icon}
    </button>
    {tooltip.tooltip}
  </>;
}

export function StatusIcon({ status, label, className = "", children }: { status: "ready" | "warning" | "error" | "info" | "disabled"; label: string; className?: string; children?: ReactNode }) {
  const tooltip = useTooltip<HTMLSpanElement>(label);
  return <>
    <span ref={tooltip.anchorRef} className={`relay-status-icon ${className}`.trim()} data-status={status} role="img" tabIndex={0} aria-label={label} aria-describedby={tooltip.describedBy} onMouseEnter={tooltip.show} onMouseLeave={tooltip.hideAfterHover} onFocus={tooltip.showAfterFocus} onBlur={tooltip.hide} onPointerDown={tooltip.pointerStart}>{children ?? <StatusBadge status={status} label="" />}</span>
    {tooltip.tooltip}
  </>;
}

export function ActionMenu({ children, className = "", label }: { children: ReactNode; className?: string; label?: string }) {
  const { t } = useTranslation();
  const resolvedLabel = label ?? t("common.actions");
  const tooltip = useTooltip<HTMLElement>(resolvedLabel);
  return <details className={`relay-action-menu ${className}`.trim()}><summary ref={tooltip.anchorRef} aria-label={resolvedLabel} aria-describedby={tooltip.describedBy} aria-haspopup="menu" onMouseEnter={tooltip.show} onMouseLeave={tooltip.hideAfterHover} onFocus={tooltip.showAfterFocus} onBlur={tooltip.hide} onPointerDown={tooltip.pointerStart}><MoreHorizontal aria-hidden /></summary>{tooltip.tooltip}<div role="menu">{children}</div></details>;
}

export function ActionMenuItem({ children, icon, danger = false, className = "", onClick, ...props }: React.ButtonHTMLAttributes<HTMLButtonElement> & { icon: ReactNode; danger?: boolean }) {
  const classes = [danger ? "danger" : "", className].filter(Boolean).join(" ");
  return <button type="button" role="menuitem" className={classes || undefined} {...props} onClick={(event) => { const menu = event.currentTarget.closest("details"); if (menu) menu.open = false; onClick?.(event); }}>{icon}<span>{children}</span></button>;
}

export function OptionMenu({ label, value, options, icon, onChange, className = "", disabled = false }: {
  label: string;
  value: string;
  options: Array<{ value: string; label: string; shortLabel?: string }>;
  icon?: ReactNode;
  onChange: (value: string) => void;
  className?: string;
  disabled?: boolean;
}) {
  const triggerRef = useRef<HTMLButtonElement>(null);
  const listRef = useRef<HTMLDivElement>(null);
  const [open, setOpen] = useState(false);
  const [position, setPosition] = useState<{ left: number; top: number; width: number } | null>(null);
  const selected = options.find((option) => option.value === value) ?? options[0];

  const close = (restoreFocus = false) => {
    setOpen(false);
    setPosition(null);
    if (restoreFocus) triggerRef.current?.focus();
  };

  useLayoutEffect(() => {
    if (!open) return;
    const trigger = triggerRef.current?.getBoundingClientRect();
    const list = listRef.current;
    if (!trigger || !list) return;
    const margin = 8;
    const gap = 6;
    const width = Math.min(Math.max(trigger.width, 220), window.innerWidth - margin * 2);
    const left = Math.max(margin, Math.min(trigger.right - width, window.innerWidth - width - margin));
    const below = trigger.bottom + gap;
    const top = below + list.offsetHeight <= window.innerHeight - margin
      ? below
      : Math.max(margin, trigger.top - list.offsetHeight - gap);
    setPosition({ left, top, width });
  }, [open, options.length]);

  useEffect(() => {
    if (!open) return;
    const selectedOption = listRef.current?.querySelector<HTMLElement>('[role="option"][aria-selected="true"]');
    selectedOption?.focus({ preventScroll: true });
    const onPointerDown = (event: PointerEvent) => {
      const target = event.target as Node;
      if (!triggerRef.current?.contains(target) && !listRef.current?.contains(target)) close();
    };
    const onKeyDown = (event: KeyboardEvent) => {
      if (event.key === "Escape") {
        event.preventDefault();
        close(true);
        return;
      }
      // A listbox is rendered in a portal. Closing it before a surrounding
      // Dialog handles Tab lets the dialog keep focus inside its own subtree
      // instead of leaving focus on a detached portal option.
      if (event.key === "Tab") close();
    };
    const dismiss = () => close();
    const dismissOnWheel = (event: WheelEvent) => {
      const target = event.target;
      if (target instanceof Node && listRef.current?.contains(target)) return;
      close();
    };
    document.addEventListener("pointerdown", onPointerDown);
    // Capture Escape before a containing Dialog's document listener sees it.
    // The Dialog then observes defaultPrevented and stays open while the
    // listbox closes.
    document.addEventListener("keydown", onKeyDown, true);
    window.addEventListener("resize", dismiss);
    window.addEventListener("wheel", dismissOnWheel, true);
    return () => {
      document.removeEventListener("pointerdown", onPointerDown);
      document.removeEventListener("keydown", onKeyDown, true);
      window.removeEventListener("resize", dismiss);
      window.removeEventListener("wheel", dismissOnWheel, true);
    };
  }, [open]);

  const moveFocus = (event: React.KeyboardEvent<HTMLElement>, index: number) => {
    const direction = event.key === "ArrowDown" ? 1 : event.key === "ArrowUp" ? -1 : 0;
    const nextIndex = event.key === "Home" ? 0 : event.key === "End" ? options.length - 1 : direction ? (index + direction + options.length) % options.length : -1;
    if (event.key === "Escape") {
      event.preventDefault();
      close(true);
      return;
    }
    if (nextIndex < 0) return;
    event.preventDefault();
    listRef.current?.querySelectorAll<HTMLElement>('[role="option"]')[nextIndex]?.focus();
  };

  return <div className={`relay-option-menu ${className}`.trim()}>
    <button
      ref={triggerRef}
      type="button"
      className="relay-option-trigger"
      aria-label={`${label}: ${selected?.label ?? ""}`}
      aria-haspopup="listbox"
      aria-expanded={open}
      data-value={value}
      disabled={disabled}
      onClick={() => setOpen((current) => !current)}
      onKeyDown={(event) => {
        if (event.key === "Escape" && open) {
          event.preventDefault();
          close(true);
          return;
        }
        if (event.key !== "ArrowDown" && event.key !== "ArrowUp") return;
        event.preventDefault();
        setOpen(true);
      }}
    >
      {icon}
      <span>{selected?.shortLabel ?? selected?.label}</span>
      <ChevronDown aria-hidden />
    </button>
    {open && typeof document !== "undefined" ? createPortal(
      <div
        ref={listRef}
        className="relay-option-list"
        role="listbox"
        aria-label={label}
        data-positioned={Boolean(position)}
        style={position ? { left: position.left, top: position.top, width: position.width } : undefined}
      >
        {options.map((option, index) => <button
          key={option.value}
          type="button"
          role="option"
          data-value={option.value}
          aria-selected={option.value === value}
          onClick={() => {
            onChange(option.value);
            close(true);
          }}
          onKeyDown={(event) => moveFocus(event, index)}
        >
          <span>{option.label}</span>
          {option.value === value ? <Check aria-hidden /> : null}
        </button>)}
      </div>,
      document.body,
    ) : null}
  </div>;
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

export function Dialog({ title, children, onClose, footer, wide = false, className = "", closeOnBackdrop = false }: { title: string; children: ReactNode; onClose: () => void; footer?: ReactNode; wide?: boolean; className?: string; closeOnBackdrop?: boolean }) {
  const { t } = useTranslation();
  const dialogRef = useRef<HTMLElement>(null);
  const onCloseRef = useRef(onClose);
  // Capture the opener during render, before a descendant with autoFocus can
  // move focus during the commit phase. The cleanup must restore the control
  // that actually opened this dialog, not an input that disappears with it.
  const returnFocusRef = useRef<HTMLElement | null>(null);
  const titleId = useId();
  onCloseRef.current = onClose;
  if (returnFocusRef.current === null && typeof document !== "undefined") {
    returnFocusRef.current = document.activeElement instanceof HTMLElement ? document.activeElement : null;
  }
  useEffect(() => {
    const previouslyFocused = returnFocusRef.current;
    const focusable = () => {
      const dialog = dialogRef.current;
      if (!dialog) return [];
      return Array.from(dialog.querySelectorAll<HTMLElement>([
        "a[href]",
        "button",
        "input",
        "select",
        "textarea",
        "[contenteditable=\"true\"]",
        "[tabindex]",
      ].join(","))).filter((element) => {
        if (element.matches("input[type=\"hidden\"]") || element.hasAttribute("disabled")) return false;
        if (element.tabIndex < 0 || element.hidden || element.closest("[aria-hidden=\"true\"]")) return false;
        const style = window.getComputedStyle(element);
        return style.display !== "none" && style.visibility !== "hidden";
      });
    };
    const isTopmost = () => {
      const dialogs = document.querySelectorAll<HTMLElement>("[data-relay-dialog]");
      return dialogs.length > 0 && dialogs[dialogs.length - 1] === dialogRef.current;
    };
    const focusInitial = () => {
      const dialog = dialogRef.current;
      if (!dialog || dialog.contains(document.activeElement)) return;
      dialog.focus({ preventScroll: true });
    };
    focusInitial();
    const onKey = (event: KeyboardEvent) => {
      const dialog = dialogRef.current;
      if (!dialog || !isTopmost() || event.defaultPrevented) return;
      if (event.key === "Escape") {
        event.preventDefault();
        onCloseRef.current();
        return;
      }
      if (event.key !== "Tab") return;
      const items = focusable();
      if (!items.length) {
        event.preventDefault();
        dialog.focus({ preventScroll: true });
        return;
      }
      const first = items[0];
      const last = items[items.length - 1];
      const active = document.activeElement;
      if (active === dialog || !dialog.contains(active)) {
        event.preventDefault();
        (event.shiftKey ? last : first).focus({ preventScroll: true });
      } else if (event.shiftKey && active === first) {
        event.preventDefault();
        last.focus({ preventScroll: true });
      } else if (!event.shiftKey && active === last) {
        event.preventDefault();
        first.focus({ preventScroll: true });
      }
    };
    document.addEventListener("keydown", onKey);
    return () => {
      document.removeEventListener("keydown", onKey);
      if (
        previouslyFocused?.isConnected
        && !previouslyFocused.hidden
        && !previouslyFocused.closest("[aria-hidden=\"true\"]")
      ) {
        previouslyFocused.focus({ preventScroll: true });
      }
    };
  }, []);
  return <div className="relay-modal-backdrop" role="presentation" onPointerDown={closeOnBackdrop ? (event) => { if (event.target === event.currentTarget) onClose(); } : undefined}><section ref={dialogRef} data-relay-dialog className={`relay-dialog ${wide ? "wide" : ""}${className ? ` ${className}` : ""}`} role="dialog" aria-modal="true" aria-labelledby={titleId} tabIndex={-1}><header><h2 id={titleId}>{title}</h2><IconButton label={t("common.close")} icon={<X aria-hidden />} onClick={onClose} /></header><div className="relay-dialog-body">{children}</div>{footer != null ? <footer>{footer}</footer> : null}</section></div>;
}

export function EmptyState({ title, description, action }: { title: string; description: string; action?: ReactNode }) {
  return <div className="relay-empty"><CircleHelp aria-hidden /><strong>{title}</strong><p>{description}</p>{action}</div>;
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
    <input type="checkbox" checked={checked} disabled={disabled} aria-label={label} onChange={(event) => onChange(event.target.checked)} />
  </label>;
}

export function QuotaMeter({ window, kind, label, nowMs, concise = false }: { window: QuotaWindow | null; kind?: "primary" | "secondary"; label?: string; nowMs?: number; concise?: boolean }) {
  const { i18n, t } = useTranslation();
  const windowKind = kind ?? window?.kind ?? "primary";
  const resolvedLabel = label ?? quotaWindowLabel(window, windowKind, t);
  if (!window?.availableBasisPoints && window?.availableBasisPoints !== 0) {
    const unavailable = window ? t("common.unknown") : t("quota.notReported");
    return <div className="quota-meter unavailable"><div className="quota-meter-heading"><span>{resolvedLabel}</span><small title={unavailable}>{unavailable}</small><strong>-</strong></div><div className="quota-track" aria-label={`${resolvedLabel}: ${unavailable}`} /></div>;
  }
  const percent = Math.round(window.availableBasisPoints / 100);
  const reset = window.resetAtMs
    ? nowMs == null
      ? new Intl.DateTimeFormat(i18n.language, { dateStyle: "short", timeStyle: "short" }).format(new Date(window.resetAtMs))
      : formatDetailedRemainingTime(window.resetAtMs, nowMs, t)
    : t("common.unknown");
  const resetLabel = concise ? reset : t("quota.reset", { value: reset });
  const remainingLabel = concise ? `${percent}%` : t("quota.remainingPercent", { value: percent });
  const level = percent <= 5 ? "critical" : percent <= 20 ? "low" : "normal";
  return <div className="quota-meter" data-level={level}><div className="quota-meter-heading"><span>{resolvedLabel}</span><small title={resetLabel}>{resetLabel}</small><strong>{remainingLabel}</strong></div><div className="quota-track" aria-label={`${resolvedLabel}: ${remainingLabel}`}><span style={{ width: `${percent}%` }} /></div></div>;
}

export function QuotaStack({ snapshot, nowMs, concise = false }: { snapshot: QuotaSnapshot; nowMs?: number; concise?: boolean }) {
  const { t } = useTranslation();
  const coreBlocked = snapshot.limitReached || [snapshot.primary, snapshot.secondary]
    .some((window) => window?.availableBasisPoints === 0);
  const reported = [
    ...(["primary", "secondary"] as const).flatMap((kind) => {
      const window = snapshot[kind];
      if (!window) return [];
      return [{ id: kind, label: "", window: coreBlocked ? { ...window, availableBasisPoints: 0 } : window }];
    }),
    ...(snapshot.supplemental ?? []),
  ];
  if (!reported.length) return <div className="quota-stack"><QuotaMeter window={null} nowMs={nowMs} concise={concise} /></div>;
  return <div className="quota-stack">{reported.map((item) => <QuotaMeter key={item.id} window={item.window} label={item.label ? `${item.label} · ${quotaWindowLabel(item.window, item.window.kind, t)}` : undefined} nowMs={nowMs} concise={concise} />)}</div>;
}

export function SecretField({
  label,
  value,
  onChange,
  placeholder,
  labelAction,
}: {
  label: string;
  value: string;
  onChange: (value: string) => void;
  placeholder?: string;
  labelAction?: ReactNode;
}) {
  const { t } = useTranslation();
  const [visible, setVisible] = useState(false);
  const inputId = useId();
  return <div className="relay-field">
    {labelAction
      ? <div className="relay-field-label-row">
        <label htmlFor={inputId}>{label}</label>
        {labelAction}
      </div>
      : <label htmlFor={inputId}>{label}</label>}
    <div className="secret-field">
      <input id={inputId} type={visible ? "text" : "password"} value={value} onChange={(event) => onChange(event.target.value)} placeholder={placeholder} autoComplete="off" spellCheck={false} />
      <IconButton label={visible ? t("common.hide") : t("common.reveal")} icon={visible ? <EyeOff aria-hidden /> : <Eye aria-hidden />} onClick={() => setVisible((current) => !current)} type="button" />
    </div>
  </div>;
}

export async function copyText(value: string) {
  await navigator.clipboard.writeText(value);
}

export function CopyButton({ value, label }: { value: string; label: string }) {
  const [copied, setCopied] = useState(false);
  return <IconButton label={copied ? `${label}: copied` : label} icon={copied ? <CheckCircle2 aria-hidden /> : <Copy aria-hidden />} onClick={async () => { await copyText(value); setCopied(true); window.setTimeout(() => setCopied(false), 1500); }} />;
}
