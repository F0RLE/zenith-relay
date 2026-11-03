import { expect, test } from "bun:test";
import { focusWithoutTooltip, isTooltipFocusSuppressed } from "../src/features/relay/components/ui/focus";

test("passive focus suppresses hints only during the synchronous focus event", () => {
  const calls: FocusOptions[] = [];
  expect(isTooltipFocusSuppressed()).toBe(false);
  focusWithoutTooltip({ focus(options: FocusOptions) {
    expect(isTooltipFocusSuppressed()).toBe(true);
    calls.push(options);
  } } as unknown as HTMLElement);
  expect(calls).toEqual([{ preventScroll: true }]);
  expect(isTooltipFocusSuppressed()).toBe(false);
});

test("nested passive focus cannot end an outer suppression scope", () => {
  focusWithoutTooltip({ focus() {
    focusWithoutTooltip({ focus() { expect(isTooltipFocusSuppressed()).toBe(true); } } as unknown as HTMLElement);
    expect(isTooltipFocusSuppressed()).toBe(true);
  } } as unknown as HTMLElement);
  expect(isTooltipFocusSuppressed()).toBe(false);
});

test("failed focus never disables future keyboard tooltips", () => {
  expect(() => focusWithoutTooltip({ focus() { throw new Error("Synthetic focus failure"); } } as unknown as HTMLElement)).toThrow("Synthetic focus failure");
  expect(isTooltipFocusSuppressed()).toBe(false);
});
