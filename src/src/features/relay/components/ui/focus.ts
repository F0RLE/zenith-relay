let suppressedTooltipFocus = 0;

export function isTooltipFocusSuppressed() {
  return suppressedTooltipFocus > 0;
}

/** Restore keyboard focus while suppressing tooltip activation. */
export function focusWithoutTooltip(element: HTMLElement) {
  suppressedTooltipFocus += 1;
  try {
    element.focus({ preventScroll: true });
  } finally {
    suppressedTooltipFocus -= 1;
  }
}
