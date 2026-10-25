import type { ModelContextPolicy } from "../../api/types";

// Mirrors relay-core/catalog/context_policy.rs; never substitute a guessed maximum.
export const SHORT_CONTEXT_PERCENT = 25;
export const MIN_CUSTOM_CONTEXT = 1_024;
export const MAX_CUSTOM_CONTEXT = 2_000_000;

export function sameModelContextPolicy(left: ModelContextPolicy | undefined, right: ModelContextPolicy | undefined): boolean {
  if (!left || !right) return left === right;
  return left.mode === right.mode
    && (left.mode !== "custom" || (right.mode === "custom" && left.tokens === right.tokens));
}

export function knownContextMaximum(tokens: number | null | undefined): number | undefined {
  return tokens != null && Number.isSafeInteger(tokens) && tokens > 0 ? tokens : undefined;
}

export function contextWindowTokens(policy: ModelContextPolicy, maximum?: number, defaultTokens?: number): number | undefined {
  const limit = knownContextMaximum(maximum);
  const publishedDefault = knownContextMaximum(defaultTokens);
  if (policy.mode === "custom") return limit === undefined ? policy.tokens : Math.min(policy.tokens, limit);
  if (policy.mode === "short" && publishedDefault !== undefined) return limit === undefined ? publishedDefault : Math.min(publishedDefault, limit);
  if (limit === undefined) return publishedDefault;
  return policy.mode === "short"
    ? Math.min(limit, Math.max(MIN_CUSTOM_CONTEXT, Math.floor(limit * SHORT_CONTEXT_PERCENT / 100)))
    : limit;
}

export function customContextMaximum(maximum?: number): number {
  return Math.min(MAX_CUSTOM_CONTEXT, knownContextMaximum(maximum) ?? MAX_CUSTOM_CONTEXT);
}
