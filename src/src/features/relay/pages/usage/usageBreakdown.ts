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
  return tokenCount == null ? null : Math.max(0, tokenCount);
}

/** Projects provider usage into nested input/output components for display. */
export function usageBreakdown(input: UsageBreakdownInput): UsageBreakdown {
  const reportedInput = nonNegativeTokenCount(input.inputTokens);
  const reportedCacheRead = nonNegativeTokenCount(input.cachedInputTokens);
  const reportedCacheWrite = nonNegativeTokenCount(input.cacheWriteInputTokens);
  const inputTotal = reportedInput ?? (
    reportedCacheRead == null && reportedCacheWrite == null
      ? null
      : (reportedCacheRead ?? 0) + (reportedCacheWrite ?? 0)
  );
  const cacheRead = inputTotal == null || input.cachedInputTokens == null
    ? reportedCacheRead
    : Math.min(inputTotal, reportedCacheRead ?? 0);
  const inputAfterCacheRead = inputTotal == null ? null : inputTotal - (cacheRead ?? 0);
  const cacheWrite = inputAfterCacheRead == null || input.cacheWriteInputTokens == null
    ? reportedCacheWrite
    : Math.min(inputAfterCacheRead, reportedCacheWrite ?? 0);
  const uncachedInput = inputTotal == null
    ? null
    : inputTotal - (cacheRead ?? 0) - (cacheWrite ?? 0);
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
