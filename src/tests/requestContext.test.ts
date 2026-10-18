import { describe, expect, test } from "bun:test";
import type { ToolUseDiagnostics } from "../src/features/relay/api/types";
import { formatContextBytes, missingRequiredToolCall } from "../src/features/relay/pages/usage/usageReportFormat";

const greeting: ToolUseDiagnostics = {
  clientToolCount: 291, forwardedToolCount: 291, toolChoice: "auto",
  toolCallCount: 0, textOutput: true, terminalOutput: "text",
};

describe("request context diagnostics", () => {
  test("ordinary greetings do not require tool calls even with a large catalog", () => {
    expect(missingRequiredToolCall(greeting)).toBe(false);
    expect(missingRequiredToolCall({ ...greeting, toolChoice: "none" })).toBe(false);
    expect(missingRequiredToolCall({ ...greeting, toolChoice: "allowed_tools" })).toBe(false);
  });

  test("required tool calls remain distinguishable from an ordinary text reply", () => {
    expect(missingRequiredToolCall({ ...greeting, toolChoice: "required" })).toBe(true);
    expect(missingRequiredToolCall({ ...greeting, toolChoice: "specific" })).toBe(true);
    expect(missingRequiredToolCall({ ...greeting, toolChoice: "required", toolCallCount: 1 })).toBe(false);
    expect(missingRequiredToolCall(undefined)).toBe(false);
  });

  test("JSON byte sizes stay localized sizes rather than invented token counts", () => {
    expect(formatContextBytes(346145, "en")).toContain("346.1");
    expect(formatContextBytes(346145, "ru")).toContain("346,1");
    expect(formatContextBytes(0, "en")).toStartWith("0");
    expect(formatContextBytes(20, "en")).toContain("20");
    for (const value of [undefined, -1, NaN, Infinity]) expect(formatContextBytes(value, "en")).toBe("—");
  });
});
