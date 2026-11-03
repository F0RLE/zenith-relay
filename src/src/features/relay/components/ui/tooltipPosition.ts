export type TooltipPlacement = "top" | "bottom" | "left" | "right";
export interface TooltipPosition {
  left: number;
  top: number;
  placement: TooltipPlacement;
  arrowLeft: number;
  arrowTop: number;
}
type Rect = { left: number; top: number; width: number; height: number };
const clamp = (value: number, minimum: number, maximum: number) => Math.max(minimum, Math.min(value, maximum));

/** Browser-only geometry shared by Desktop and Hosted. All coordinates are viewport-relative. */
export function positionTooltip(anchor: Rect, tooltip: { width: number; height: number }, viewport: { width: number; height: number }, preferred: TooltipPlacement = "bottom"): TooltipPosition {
  const margin = 8, gap = 8, arrowInset = 12;
  const right = anchor.left + anchor.width, bottom = anchor.top + anchor.height;
  const room = { top: anchor.top - margin - gap, bottom: viewport.height - bottom - margin - gap, left: anchor.left - margin - gap, right: viewport.width - right - margin - gap };
  const opposite: Record<TooltipPlacement, TooltipPlacement> = { top: "bottom", bottom: "top", left: "right", right: "left" };
  const candidates: TooltipPlacement[] = [preferred, opposite[preferred], ...(preferred === "left" || preferred === "right" ? ["bottom", "top"] as const : ["right", "left"] as const)];
  const required = (side: TooltipPlacement) => side === "left" || side === "right" ? tooltip.width : tooltip.height;
  const placement = candidates.find(side => room[side] >= required(side))
    ?? candidates.reduce((best, side) => room[side] - required(side) > room[best] - required(best) ? side : best);
  const centerX = anchor.left + anchor.width / 2, centerY = anchor.top + anchor.height / 2;
  const left = clamp(placement === "right" ? right + gap : placement === "left" ? anchor.left - tooltip.width - gap : centerX - tooltip.width / 2, margin, viewport.width - tooltip.width - margin);
  const top = clamp(placement === "bottom" ? bottom + gap : placement === "top" ? anchor.top - tooltip.height - gap : centerY - tooltip.height / 2, margin, viewport.height - tooltip.height - margin);
  return { left, top, placement,
    arrowLeft: clamp(centerX - left, Math.min(arrowInset, tooltip.width / 2), tooltip.width - Math.min(arrowInset, tooltip.width / 2)),
    arrowTop: clamp(centerY - top, Math.min(arrowInset, tooltip.height / 2), tooltip.height - Math.min(arrowInset, tooltip.height / 2)),
  };
}
