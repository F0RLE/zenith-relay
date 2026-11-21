export type UsageBreakdownInput = {
  inputTokens: number | null;
  cachedInputTokens: number | null;
  cacheWriteInputTokens: number | null;
  outputTokens: number | null;
  reasoningTokens: number | null;
  totalTokens: number | null;
};

export type UsageBreakdown = {
  inputTotal: number | null;
  uncachedInput: number | null;
  cacheRead: number | null;
  cacheWrite: number | null;
  outputTotal: number | null;
  reasoning: number | null;
  visibleOutput: number | null;
  total: number | null;
};

function nonNegativeTokenCount(tokenCount: number | null): number | null {
  return tokenCount != null && Number.isFinite(tokenCount) && tokenCount >= 0 ? tokenCount : null;
}

/** Projects provider usage into nested input/output components for display. */
export function usageBreakdown(input: UsageBreakdownInput): UsageBreakdown {
  const inputTotal = nonNegativeTokenCount(input.inputTokens);
  const reportedCacheRead = nonNegativeTokenCount(input.cachedInputTokens);
  const reportedCacheWrite = nonNegativeTokenCount(input.cacheWriteInputTokens);
  const cacheRead = inputTotal != null && reportedCacheRead != null && reportedCacheRead > inputTotal ? null : reportedCacheRead;
  const cacheWrite = inputTotal != null && reportedCacheWrite != null && reportedCacheWrite > inputTotal ? null : reportedCacheWrite;
  const cacheSum = (cacheRead ?? 0) + (cacheWrite ?? 0);
  const uncachedInput = inputTotal != null && cacheRead != null && cacheWrite != null && cacheSum <= inputTotal
    ? inputTotal - cacheSum : null;
  const outputTotal = nonNegativeTokenCount(input.outputTokens);
  const reasoning = outputTotal == null || input.reasoningTokens == null
    ? nonNegativeTokenCount(input.reasoningTokens)
    : Math.min(outputTotal, nonNegativeTokenCount(input.reasoningTokens) ?? 0);

  return {
    inputTotal,
    uncachedInput,
    cacheRead,
    cacheWrite,
    outputTotal,
    reasoning,
    visibleOutput: outputTotal == null ? null : outputTotal - (reasoning ?? 0),
    total: nonNegativeTokenCount(input.totalTokens) ?? (
      inputTotal == null || outputTotal == null ? null : inputTotal + outputTotal
    ),
  };
}
