import { expect, test } from "bun:test";
import { positionTooltip } from "../src/features/relay/components/ui/tooltipPosition";
import type { TooltipPlacement } from "../src/features/relay/components/ui/tooltipPosition";

const viewport = { width: 800, height: 600 };
const tooltip = { width: 160, height: 32 };

test("tooltip points up at a centered trigger and flips above at the bottom edge", () => {
  const below = positionTooltip({ left: 300, top: 100, width: 40, height: 32 }, tooltip, viewport);
  expect(below).toEqual({ left: 240, top: 140, placement: "bottom", arrowLeft: 80, arrowTop: 12 });
  const above = positionTooltip({ left: 300, top: 550, width: 40, height: 32 }, tooltip, viewport);
  expect(above.placement).toBe("top");
  expect(above.top + tooltip.height).toBe(542);
  expect(above.left + above.arrowLeft).toBe(320);
});

test("sidebar tooltip sits beside the control and flips left near the right edge", () => {
  const right = positionTooltip({ left: 8, top: 220, width: 32, height: 32 }, tooltip, viewport, "right");
  expect(right.placement).toBe("right");
  expect(right.left).toBe(48);
  expect(right.top + right.arrowTop).toBe(236);
  const left = positionTooltip({ left: 760, top: 220, width: 32, height: 32 }, tooltip, viewport, "right");
  expect(left.placement).toBe("left");
  expect(left.left + tooltip.width).toBe(752);
  expect(left.top + left.arrowTop).toBe(236);
});

test("clamping the bubble does not center its pointer away from the trigger", () => {
  const result = positionTooltip({ left: 8, top: 100, width: 32, height: 32 }, tooltip, viewport);
  expect(result.left).toBe(8);
  expect(result.arrowLeft).toBe(16);
  expect(result.left + result.arrowLeft).toBe(24);
});

test("all placements stay inside narrow and desktop viewports with safe pointer insets", () => {
  for (const screen of [{ width: 320, height: 740 }, { width: 1160, height: 760 }]) {
    for (const x of [8, screen.width / 2, screen.width - 40]) {
      for (const y of [8, screen.height / 2, screen.height - 40]) {
        for (const side of ["top", "bottom", "left", "right"] satisfies TooltipPlacement[]) {
          const result = positionTooltip({ left: x, top: y, width: 32, height: 32 }, tooltip, screen, side);
          expect(result.left).toBeGreaterThanOrEqual(8);
          expect(result.top).toBeGreaterThanOrEqual(8);
          expect(result.left + tooltip.width).toBeLessThanOrEqual(screen.width - 8);
          expect(result.top + tooltip.height).toBeLessThanOrEqual(screen.height - 8);
          expect(result.arrowLeft).toBeGreaterThanOrEqual(12);
          expect(result.arrowLeft).toBeLessThanOrEqual(tooltip.width - 12);
          expect(result.arrowTop).toBeGreaterThanOrEqual(12);
          expect(result.arrowTop).toBeLessThanOrEqual(tooltip.height - 12);
        }
      }
    }
  }
});
