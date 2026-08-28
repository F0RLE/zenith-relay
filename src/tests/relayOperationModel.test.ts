import { describe, expect, test } from "bun:test";
import type { FeedbackError } from "../src/features/relay/state/feedback";
import { runRelayOperation } from "../src/features/relay/state/relayOperationModel";

const resolvedError = (cause: unknown) => ({
  key: "errors.general",
  error: { code: "general", message: String(cause) } satisfies FeedbackError,
});

describe("relay operation policy", () => {
  test("runs work, refreshes current state, and publishes success in order", async () => {
    const events: string[] = [];
    const result = await runRelayOperation({
      work: async () => { events.push("work"); },
      refresh: async () => { events.push("refresh"); },
      isCurrent: () => true,
      successKey: "feedback.saved",
      resolveError: resolvedError,
      setFeedback: (feedback) => events.push(`${feedback.kind}:${feedback.key}`),
      settle: () => events.push("settled"),
    });
    expect(result).toBeTrue();
    expect(events).toEqual(["work", "refresh", "success:feedback.saved", "settled"]);
  });

  test("does not refresh or settle an operation superseded while work is running", async () => {
    let current = true;
    let refreshes = 0;
    let settlements = 0;
    const result = await runRelayOperation({
      work: async () => { current = false; },
      refresh: async () => { refreshes += 1; },
      isCurrent: () => current,
      resolveError: resolvedError,
      setFeedback: () => { throw new Error("stale feedback"); },
      settle: () => { settlements += 1; },
    });
    expect(result).toBeFalse();
    expect(refreshes).toBe(0);
    expect(settlements).toBe(0);
  });

  test("keeps a handled error local when global reporting is disabled", async () => {
    const local: Array<{ code: string; key: string }> = [];
    let globalFeedback = 0;
    let settlements = 0;
    const result = await runRelayOperation({
      work: async () => { throw new Error("upstream failed"); },
      refresh: async () => { throw new Error("refresh should not run"); },
      isCurrent: () => true,
      options: {
        reportError: false,
        onError: (error, key) => local.push({ code: error.code, key }),
      },
      resolveError: resolvedError,
      setFeedback: () => { globalFeedback += 1; },
      settle: () => { settlements += 1; },
    });
    expect(result).toBeFalse();
    expect(local).toEqual([{ code: "general", key: "errors.general" }]);
    expect(globalFeedback).toBe(0);
    expect(settlements).toBe(1);
  });

  test("reports refresh failures through the same sanitized error path", async () => {
    const feedback: Array<{ kind: string; key: string; message?: string }> = [];
    const result = await runRelayOperation({
      work: async () => undefined,
      refresh: async () => { throw new Error("refresh failed"); },
      isCurrent: () => true,
      resolveError: resolvedError,
      setFeedback: (item) => feedback.push({ kind: item.kind, key: item.key, message: item.error?.message }),
      settle: () => undefined,
    });
    expect(result).toBeFalse();
    expect(feedback).toEqual([{ kind: "error", key: "errors.general", message: "Error: refresh failed" }]);
  });
});
