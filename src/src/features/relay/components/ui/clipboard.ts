/** User-gesture copy avoids WebView2 clipboard-permission prompts. */
export async function copyText(text: string) {
  const active = document.activeElement;
  const field = active instanceof HTMLInputElement || active instanceof HTMLTextAreaElement ? active : null;
  const start = field?.selectionStart;
  const end = field?.selectionEnd;
  const direction = field?.selectionDirection;
  const selection = window.getSelection();
  const ranges = selection ? Array.from({ length: selection.rangeCount }, (_, index) => selection.getRangeAt(index).cloneRange()) : [];
  const temporary = document.createElement("textarea");
  temporary.value = text;
  temporary.readOnly = true;
  temporary.tabIndex = -1;
  temporary.setAttribute("aria-hidden", "true");
  Object.assign(temporary.style, { position: "fixed", left: "-10000px", top: "0", opacity: "0" });
  document.body.appendChild(temporary);
  try {
    temporary.select();
    if (!document.execCommand("copy")) throw new Error("Clipboard copy failed");
  } finally {
    temporary.remove();
    if (active instanceof HTMLElement && active.isConnected) {
      active.focus({ preventScroll: true });
      if (field && start != null && end != null) field.setSelectionRange(start, end, direction ?? undefined);
    }
    if (!field && selection) {
      selection.removeAllRanges();
      for (const range of ranges) selection.addRange(range);
    }
  }
}
