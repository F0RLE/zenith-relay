import { normalizeReasoningEffort, sortReasoningEfforts } from "../../poolFormatting";

export function initialReasoningLevels(allowedLevels: string[] | undefined, detectedLevels: string[]) {
  return sortReasoningEfforts(allowedLevels ?? detectedLevels);
}

export function toggleReasoningLevel(levels: string[], level: string) {
  const normalized = normalizeReasoningEffort(level);
  if (!normalized) return levels;
  return levels.includes(normalized)
    ? levels.filter((current) => current !== normalized)
    : sortReasoningEfforts([...levels, normalized]);
}
