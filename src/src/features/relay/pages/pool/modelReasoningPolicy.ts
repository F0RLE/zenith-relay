import type { ModelReasoningProbeResult } from "../../api/types";
import { sortReasoningEfforts } from "../../poolFormatting";

export function normalizeReasoningLevel(value: string) {
  return value.trim().toLowerCase();
}

export function initialReasoningLevels(allowedLevels: string[] | undefined, detectedLevels: string[]) {
  return sortReasoningEfforts(allowedLevels ?? detectedLevels);
}

export function toggleReasoningLevel(levels: string[], level: string) {
  const normalized = normalizeReasoningLevel(level);
  if (!normalized) return levels;
  return levels.includes(normalized)
    ? levels.filter((current) => current !== normalized)
    : sortReasoningEfforts([...levels, normalized]);
}

export function addReasoningLevel(levels: string[], level: string) {
  const normalized = normalizeReasoningLevel(level);
  if (!normalized || levels.includes(normalized)) return levels;
  return sortReasoningEfforts([...levels, normalized]);
}

export function mergeSuccessfulProbe(levels: string[], result: ModelReasoningProbeResult) {
  if (!result.appliedToSettings) return levels;
  return addReasoningLevel(levels, result.level);
}
