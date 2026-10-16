import { CheckCheck, ClipboardPaste, Copy, Scissors } from "lucide-react";
import { useEffect, useLayoutEffect, useRef, useState } from "react";
import type { ReactNode } from "react";
import { createPortal } from "react-dom";
import { useTranslation } from "react-i18next";

type TextField = HTMLInputElement | HTMLTextAreaElement;

type ContextState = {
  x: number;
  y: number;
  field: TextField | null;
  selectionStart: number;
  selectionEnd: number;
  text: string;
  writable: boolean;
};

const textInputTypes = new Set(["email", "password", "search", "tel", "text", "url"]);

function textFieldFrom(target: EventTarget | null): TextField | null {
  if (!(target instanceof Element)) return null;
  const field = target.closest("input, textarea");
  if (field instanceof HTMLTextAreaElement) return field;
  return field instanceof HTMLInputElement && textInputTypes.has(field.type) ? field : null;
}

async function writeClipboard(text: string) {
  try {
    await navigator.clipboard.writeText(text);
    return true;
  } catch {
    const fallback = document.createElement("textarea");
    fallback.value = text;
    fallback.style.position = "fixed";
    fallback.style.opacity = "0";
    document.body.appendChild(fallback);
    fallback.select();
    const copied = document.execCommand("copy");
    fallback.remove();
    return copied;
  }
}

function replaceSelection(field: TextField, start: number, end: number, value: string) {
  const updatedText = `${field.value.slice(0, start)}${value}${field.value.slice(end)}`;
  const setter = Object.getOwnPropertyDescriptor(Object.getPrototypeOf(field), "value")?.set;
  setter?.call(field, updatedText);
  const caret = start + value.length;
  field.focus();
  field.setSelectionRange(caret, caret);
  field.dispatchEvent(new Event("input", { bubbles: true }));
}

export function ContextMenu({ position, children, onClose }: {
  position: { x: number; y: number };
  children: ReactNode;
  onClose: (restoreFocus?: boolean) => void;
}) {
  const { t } = useTranslation();
  const menuRef = useRef<HTMLDivElement>(null);
  const [placement, setPlacement] = useState<{ left: number; top: number } | null>(null);

  useLayoutEffect(() => {
    if (!menuRef.current) return;
    const margin = 8;
    const rect = menuRef.current.getBoundingClientRect();
    setPlacement({
      left: Math.max(margin, Math.min(position.x, window.innerWidth - rect.width - margin)),
      top: Math.max(margin + 36, Math.min(position.y, window.innerHeight - rect.height - margin)),
    });
    menuRef.current.querySelector<HTMLButtonElement>("button:not(:disabled)")?.focus({ preventScroll: true });
  }, [position]);

  useEffect(() => {
    const outside = (event: PointerEvent) => {
      if (!menuRef.current?.contains(event.target as Node)) onClose();
    };
    const onKeyDown = (event: KeyboardEvent) => {
      if (event.key === "Escape") {
        event.preventDefault();
        event.stopPropagation();
        onClose(true);
      } else if (event.key === "Tab") {
        onClose(true);
      }
    };
    const dismiss = () => onClose();
    document.addEventListener("pointerdown", outside);
    document.addEventListener("keydown", onKeyDown, true);
    window.addEventListener("resize", dismiss);
    window.addEventListener("scroll", dismiss, true);
    window.addEventListener("blur", dismiss);
    return () => {
      document.removeEventListener("pointerdown", outside);
      document.removeEventListener("keydown", onKeyDown, true);
      window.removeEventListener("resize", dismiss);
      window.removeEventListener("scroll", dismiss, true);
      window.removeEventListener("blur", dismiss);
    };
  }, [onClose]);

  const focusMenuItem = (event: React.KeyboardEvent<HTMLDivElement>) => {
    const menuItems = Array.from(menuRef.current?.querySelectorAll<HTMLButtonElement>("button:not(:disabled)") ?? []);
    const activeIndex = menuItems.indexOf(document.activeElement as HTMLButtonElement);
    const direction = event.key === "ArrowDown" ? 1 : event.key === "ArrowUp" ? -1 : 0;
    const targetIndex = event.key === "Home" ? 0 : event.key === "End" ? menuItems.length - 1 : direction ? (activeIndex + direction + menuItems.length) % menuItems.length : -1;
    if (targetIndex >= 0) {
      event.preventDefault();
      menuItems[targetIndex]?.focus();
    }
  };

  return createPortal(<div
    ref={menuRef}
    className="app-context-menu relay-popover-panel"
    role="menu"
    aria-label={t("common.contextMenu")}
    data-positioned={Boolean(placement)}
    style={placement ?? undefined}
    onKeyDown={focusMenuItem}
    onContextMenu={(event) => event.preventDefault()}
  >{children}</div>, document.body);
}

export function AppContextMenu() {
  const { t } = useTranslation();
  const [context, setContext] = useState<ContextState | null>(null);

  const close = (restoreFocus = false) => {
    if (restoreFocus) context?.field?.focus();
    setContext(null);
  };

  useEffect(() => {
    const open = (event: MouseEvent) => {
      if (event.defaultPrevented || (event.target instanceof Element && event.target.closest("[data-relay-context-action]"))) return;
      event.preventDefault();
      const field = textFieldFrom(event.target);
      const selectionStart = field?.selectionStart ?? 0;
      const selectionEnd = field?.selectionEnd ?? selectionStart;
      const protectsPassword = field instanceof HTMLInputElement && field.type === "password";
      const text = field
        ? protectsPassword ? "" : field.value.slice(selectionStart, selectionEnd)
        : window.getSelection()?.toString() ?? "";
      if (!field && !text) {
        setContext(null);
        return;
      }
      const targetRect = event.target instanceof Element ? event.target.getBoundingClientRect() : null;
      setContext({
        x: event.clientX || (targetRect?.left ?? 0) + 8,
        y: event.clientY || (targetRect?.top ?? 0) + 8,
        field,
        selectionStart,
        selectionEnd,
        text,
        writable: Boolean(field && !field.disabled && !field.readOnly),
      });
    };
    document.addEventListener("contextmenu", open);
    return () => document.removeEventListener("contextmenu", open);
  }, []);

  if (!context) return null;
  const hasSelection = Boolean(context.text);

  return <ContextMenu position={context} onClose={close}>
    {context.field && context.writable ? <button className="relay-popover-item" type="button" role="menuitem" disabled={!hasSelection} onClick={async () => {
      if (!context.text || !await writeClipboard(context.text)) return;
      replaceSelection(context.field!, context.selectionStart, context.selectionEnd, "");
      close();
    }}><Scissors aria-hidden /><span>{t("common.cut")}</span></button> : null}
    <button className="relay-popover-item" type="button" role="menuitem" disabled={!hasSelection} onClick={async () => {
      if (context.text) await writeClipboard(context.text);
      close(true);
    }}><Copy aria-hidden /><span>{t("common.copy")}</span></button>
    {context.field && context.writable ? <button className="relay-popover-item" type="button" role="menuitem" onClick={async () => {
      try {
        const text = await navigator.clipboard.readText();
        replaceSelection(context.field!, context.selectionStart, context.selectionEnd, text);
      } catch {
        context.field?.focus();
      }
      close();
    }}><ClipboardPaste aria-hidden /><span>{t("common.paste")}</span></button> : null}
    {context.field ? <button type="button" role="menuitem" className="relay-popover-item context-menu-select-all" onClick={() => {
      context.field?.focus();
      context.field?.select();
      close();
    }}><CheckCheck aria-hidden /><span>{t("common.selectAllText")}</span></button> : null}
  </ContextMenu>;
}
