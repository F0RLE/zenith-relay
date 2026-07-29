import { CheckCheck, ClipboardPaste, Copy, Scissors } from "lucide-react";
import { useEffect, useLayoutEffect, useRef, useState } from "react";
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
  const next = `${field.value.slice(0, start)}${value}${field.value.slice(end)}`;
  const setter = Object.getOwnPropertyDescriptor(Object.getPrototypeOf(field), "value")?.set;
  setter?.call(field, next);
  const caret = start + value.length;
  field.focus();
  field.setSelectionRange(caret, caret);
  field.dispatchEvent(new Event("input", { bubbles: true }));
}

export function AppContextMenu() {
  const { t } = useTranslation();
  const menuRef = useRef<HTMLDivElement>(null);
  const [context, setContext] = useState<ContextState | null>(null);
  const [position, setPosition] = useState<{ left: number; top: number } | null>(null);

  const close = (restoreFocus = false) => {
    if (restoreFocus) context?.field?.focus();
    setContext(null);
    setPosition(null);
  };

  useEffect(() => {
    const open = (event: MouseEvent) => {
      event.preventDefault();
      const field = textFieldFrom(event.target);
      const selectionStart = field?.selectionStart ?? 0;
      const selectionEnd = field?.selectionEnd ?? selectionStart;
      const protectsPassword = field instanceof HTMLInputElement && field.type === "password";
      const text = field
        ? protectsPassword ? "" : field.value.slice(selectionStart, selectionEnd)
        : window.getSelection()?.toString() ?? "";
      if (!field && !text) {
        close();
        return;
      }
      const targetRect = event.target instanceof Element ? event.target.getBoundingClientRect() : null;
      setPosition(null);
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

  useLayoutEffect(() => {
    if (!context || !menuRef.current) return;
    const margin = 8;
    const rect = menuRef.current.getBoundingClientRect();
    setPosition({
      left: Math.max(margin, Math.min(context.x, window.innerWidth - rect.width - margin)),
      top: Math.max(margin + 36, Math.min(context.y, window.innerHeight - rect.height - margin)),
    });
    menuRef.current.querySelector<HTMLButtonElement>("button:not(:disabled)")?.focus();
  }, [context]);

  useEffect(() => {
    if (!context) return;
    const outside = (event: PointerEvent) => {
      if (!menuRef.current?.contains(event.target as Node)) close();
    };
    const onKeyDown = (event: KeyboardEvent) => {
      if (event.key !== "Escape") return;
      event.preventDefault();
      close(true);
    };
    const dismiss = () => close();
    document.addEventListener("pointerdown", outside);
    document.addEventListener("keydown", onKeyDown);
    window.addEventListener("resize", dismiss);
    window.addEventListener("scroll", dismiss, true);
    window.addEventListener("blur", dismiss);
    return () => {
      document.removeEventListener("pointerdown", outside);
      document.removeEventListener("keydown", onKeyDown);
      window.removeEventListener("resize", dismiss);
      window.removeEventListener("scroll", dismiss, true);
      window.removeEventListener("blur", dismiss);
    };
  }, [context]);

  if (!context) return null;
  const hasSelection = Boolean(context.text);
  const focusMenuItem = (event: React.KeyboardEvent<HTMLDivElement>) => {
    const items = Array.from(menuRef.current?.querySelectorAll<HTMLButtonElement>("button:not(:disabled)") ?? []);
    const index = items.indexOf(document.activeElement as HTMLButtonElement);
    const direction = event.key === "ArrowDown" ? 1 : event.key === "ArrowUp" ? -1 : 0;
    const next = event.key === "Home" ? 0 : event.key === "End" ? items.length - 1 : direction ? (index + direction + items.length) % items.length : -1;
    if (event.key === "Escape") {
      event.preventDefault();
      close(true);
    } else if (event.key === "Tab") {
      close();
    } else if (next >= 0) {
      event.preventDefault();
      items[next]?.focus();
    }
  };

  return <div
    ref={menuRef}
    className="app-context-menu"
    role="menu"
    aria-label={t("common.contextMenu")}
    data-positioned={Boolean(position)}
    style={position ?? undefined}
    onKeyDown={focusMenuItem}
  >
    {context.field && context.writable ? <button type="button" role="menuitem" disabled={!hasSelection} onClick={async () => {
      if (!context.text || !await writeClipboard(context.text)) return;
      replaceSelection(context.field!, context.selectionStart, context.selectionEnd, "");
      close();
    }}><Scissors aria-hidden /><span>{t("common.cut")}</span></button> : null}
    <button type="button" role="menuitem" disabled={!hasSelection} onClick={async () => {
      if (context.text) await writeClipboard(context.text);
      close(true);
    }}><Copy aria-hidden /><span>{t("common.copy")}</span></button>
    {context.field && context.writable ? <button type="button" role="menuitem" onClick={async () => {
      try {
        const text = await navigator.clipboard.readText();
        replaceSelection(context.field!, context.selectionStart, context.selectionEnd, text);
      } catch {
        context.field?.focus();
      }
      close();
    }}><ClipboardPaste aria-hidden /><span>{t("common.paste")}</span></button> : null}
    {context.field ? <button type="button" role="menuitem" className="context-menu-select-all" onClick={() => {
      context.field?.focus();
      context.field?.select();
      close();
    }}><CheckCheck aria-hidden /><span>{t("common.selectAllText")}</span></button> : null}
  </div>;
}
