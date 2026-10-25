let suppressedTooltipFocus = 0;

export function isTooltipFocusSuppressed() {
  return suppressedTooltipFocus > 0;
}

/** Restoring keyboard position is not a request to show/measure another hint. */
export function focusWithoutTooltip(element: HTMLElement) {
  suppressedTooltipFocus += 1;
  try {
    element.focus({ preventScroll: true });
  } finally {
    suppressedTooltipFocus -= 1;
  }
}
