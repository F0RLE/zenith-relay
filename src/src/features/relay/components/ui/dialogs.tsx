import { useEffect, useId, useRef } from "react";
import type { ReactNode } from "react";
import { Check, CircleAlert, Copy, X } from "lucide-react";
import { useTranslation } from "react-i18next";
import type { FeedbackError } from "../../state/feedback";
import { useTransientFlag } from "../../hooks/useTransientFlag";
import { Button, IconButton } from "./buttons";
import { copyText } from "./secret";

export function Dialog({
  title,
  children,
  onClose,
  footer,
  wide = false,
  className = "",
  layer = "default",
}: {
  title: string;
  children: ReactNode;
  onClose: () => void;
  footer?: ReactNode;
  wide?: boolean;
  className?: string;
  layer?: "default" | "top";
}) {
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
      if (
        previouslyFocused?.isConnected
        && !previouslyFocused.hidden
        && !previouslyFocused.closest("[aria-hidden=\"true\"]")
      ) {
        previouslyFocused.focus({ preventScroll: true });
      }
    };
  }, []);
  return (
    <div
      className={`relay-modal-backdrop${layer === "top" ? " relay-modal-backdrop-top" : ""}`}
      role="presentation"
      onPointerDown={(event) => { if (event.target === event.currentTarget) onClose(); }}
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
          <h2 id={titleId}>{title}</h2>
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
  const details = JSON.stringify(error, null, 2);
  const copyError = async () => {
    try {
      await copyText(details);
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
    <div className="global-feedback-dialog-summary"><CircleAlert aria-hidden /><div><strong>{message}</strong><code>{error.code}</code></div></div>
    <div className="config-preview global-feedback-error-json"><pre><code>{details}</code></pre></div>
    <p className="form-note">{t("feedback.detailsHint")}</p>
  </Dialog>;
}

