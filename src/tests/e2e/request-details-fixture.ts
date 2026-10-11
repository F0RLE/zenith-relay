import type { Page } from "../bun-playwright";

export const noOutputUsage = {
  ttftMs: null, generationMs: null, inputTokens: null, outputTokens: null,
  cachedInputTokens: null, cacheWriteInputTokens: null, reasoningTokens: null, totalTokens: null,
  apiEquivalent: { microUsd: 0, pricedTokens: 0, unpricedTokens: 0 },
};

export const completedUsage = {
  latencyMs: 7840, ttftMs: 640, generationMs: 7200,
  inputTokens: 18420, cachedInputTokens: 15360, cacheWriteInputTokens: 0,
  outputTokens: 920, reasoningTokens: 320, totalTokens: 19340,
  requestedReasoningEffort: "max", effectiveReasoningEffort: "max",
  apiEquivalent: { microUsd: 42000, pricedTokens: 19340, unpricedTokens: 0 },
  toolUse: {
    clientToolCount: 3, forwardedToolCount: 3, toolChoice: "auto",
    toolCallCount: 2, textOutput: true, terminalOutput: "mixed",
  },
};

export async function setUsageEventOverrides(page: Page, overrides: Record<string, unknown>) {
  await page.evaluate((fields) => {
    const internals = (window as unknown as {
      __TAURI_INTERNALS__: { invoke: (command: string, args?: unknown, options?: unknown) => Promise<unknown> };
    }).__TAURI_INTERNALS__;
    const invoke = internals.invoke.bind(internals);
    internals.invoke = async (command, args, options) => {
      const result = await invoke(command, args, options);
      if (!result || (command !== "get_local_usage_page" && command !== "get_remote_server_usage")) return result;
      const report = structuredClone(result) as { events: Array<Record<string, unknown>> };
      for (const event of report.events) Object.assign(event, fields);
      return report;
    };
  }, overrides);
}
