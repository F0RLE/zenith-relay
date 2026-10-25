import { useEffect, useId, useRef } from "react";
import type { ReactNode } from "react";
import { Check, CircleAlert, Copy, X } from "lucide-react";
import { useTranslation } from "react-i18next";
import type { FeedbackError } from "../../state/feedback";
import { useTransientFlag } from "../../hooks/useTransientFlag";
import { Button, IconButton } from "./buttons";
import { copyText } from "./secret";
import { focusWithoutTooltip } from "./focus";

let focusRestoreGeneration = 0;

export function Dialog({
  title,
  subtitle,
  children,
  onClose,
  footer,
  headerActions,
  wide = false,
  className = "",
  backdropClassName = "",
  restoreFocus = "immediate",
  layer = "default",
}: {
  title: string;
  subtitle?: ReactNode;
  children: ReactNode;
  onClose: () => void;
  footer?: ReactNode;
  headerActions?: ReactNode;
  wide?: boolean;
  className?: string;
  backdropClassName?: string;
  restoreFocus?: "immediate" | "after-paint";
  layer?: "default" | "top";
}) {
  const { t } = useTranslation();
  const dialogRef = useRef<HTMLElement>(null);
  const onCloseRef = useRef(onClose);
  // Capture the opener before descendant autoFocus runs; restore it on cleanup.
  const returnFocusRef = useRef<HTMLElement | null>(null);
  const titleId = useId();
  onCloseRef.current = onClose;
  if (returnFocusRef.current === null && typeof document !== "undefined") {
    returnFocusRef.current = document.activeElement instanceof HTMLElement ? document.activeElement : null;
  }
  useEffect(() => {
    focusRestoreGeneration += 1;
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
        "summary",
        "[contenteditable=\"true\"]",
        "[tabindex]",
      ].join(","))).filter((element) => {
        if (element.matches("input[type=\"hidden\"]") || element.hasAttribute("disabled")) return false;
        if (element.tabIndex < 0 || element.hidden || element.closest("[aria-hidden=\"true\"]")) return false;
        // Closed details can retain layout boxes. Only their first summary
        // remains focusable, including when disclosures are nested.
        for (let ancestor = element.parentElement; ancestor && ancestor !== dialog; ancestor = ancestor.parentElement) {
          if (ancestor.matches("details:not([open])") && !ancestor.querySelector(":scope > summary")?.contains(element)) return false;
        }
        const style = window.getComputedStyle(element);
        return element.getClientRects().length > 0 && style.display !== "none" && style.visibility !== "hidden";
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
      const focusableElements = focusable();
      if (!focusableElements.length) {
        event.preventDefault();
        dialog.focus({ preventScroll: true });
        return;
      }
      const first = focusableElements[0];
      const last = focusableElements[focusableElements.length - 1];
      if (!first || !last) return;
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
      const generation = ++focusRestoreGeneration;
      const restore = () => {
        if (generation !== focusRestoreGeneration || !previouslyFocused?.isConnected
          || previouslyFocused.hidden || previouslyFocused.closest("[aria-hidden=\"true\"]")) return;
        if (restoreFocus === "after-paint") {
          // A newer dialog, navigation or a user's new focus must always win.
          const active = document.activeElement;
          if (active !== document.body && active !== document.documentElement) return;
          focusWithoutTooltip(previouslyFocused);
        } else {
          previouslyFocused.focus({ preventScroll: true });
        }
      };
      if (restoreFocus === "after-paint") {
        if (document.visibilityState === "visible") {
          // Removing the overlay gets a paint opportunity before focus can
          // force layout of the table or mount a delegated tooltip.
          requestAnimationFrame(() => requestAnimationFrame(restore));
        }
      } else restore();
    };
  }, []);
  return (
    <div
      className={`relay-modal-backdrop${layer === "top" ? " relay-modal-backdrop-top" : ""}${backdropClassName ? ` ${backdropClassName}` : ""}`}
      role="presentation"
      onPointerDown={(event) => {
        if (event.target !== event.currentTarget) return;
        // Do not let the default mouse focus overwrite the restored opener
        // after this backdrop disappears during pointerdown.
        event.preventDefault();
        onClose();
      }}
    >
      <section
        ref={dialogRef}
        data-relay-dialog
        className={`relay-dialog ${wide ? "wide" : ""}${className ? ` ${className}` : ""}`}
        role="dialog"
        aria-modal="true"
        aria-labelledby={titleId}
        tabIndex={-1}
      >
        <header>
          {subtitle ? (
            <div className="relay-dialog-heading">
              <h2 id={titleId}>{title}</h2>
              {subtitle}
            </div>
          ) : <h2 id={titleId}>{title}</h2>}
          {headerActions ? <div className="relay-dialog-header-actions">{headerActions}</div> : null}
          <IconButton label={t("common.close")} icon={<X aria-hidden />} onClick={onClose} />
        </header>
        <div className="relay-dialog-body">{children}</div>
        {footer != null ? <footer>{footer}</footer> : null}
      </section>
    </div>
  );
}

export function ErrorDetailsDialog({ error, message, onClose }: { error: FeedbackError; message: string; onClose: () => void }) {
  const { t } = useTranslation();
  const [copied, showCopied, clearCopied] = useTransientFlag(1_500);
  const errorDetailsJson = JSON.stringify(error, null, 2);
  const copyError = async () => {
    try {
      await copyText(errorDetailsJson);
      showCopied();
    } catch {
      clearCopied();
    }
  };
  return <Dialog
    title={t("feedback.errorDetails")}
    onClose={onClose}
    layer="top"
    className="global-feedback-error-dialog"
    footer={(
      <div className="global-feedback-dialog-actions">
        <span className="global-feedback-dialog-copy-state" role="status" aria-live="polite">
          {copied ? t("feedback.copied") : ""}
        </span>
        <Button
          variant="secondary"
          icon={copied ? <Check aria-hidden /> : <Copy aria-hidden />}
          onClick={() => void copyError()}
        >
          {copied ? t("feedback.copied") : t("feedback.copyError")}
        </Button>
        <Button variant="primary" onClick={onClose}>{t("common.close")}</Button>
      </div>
    )}
  >
    <div className="global-feedback-dialog-summary"><CircleAlert aria-hidden /><div><strong>{message}</strong></div></div>
    <div className="config-preview global-feedback-error-json"><pre><code>{errorDetailsJson}</code></pre></div>
  </Dialog>;
}

