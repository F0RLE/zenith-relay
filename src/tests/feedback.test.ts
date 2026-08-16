import { describe, expect, test } from "bun:test";
import { redactFeedbackText, sanitizeFeedbackError } from "../src/features/relay/state/feedback";

describe("feedback diagnostics", () => {
  test("redacts account identities while preserving safe diagnostics", () => {
    const diagnostic = sanitizeFeedbackError({
      error: {
        code: "upstream_invalid",
        message: "request for alice@example.test failed: accountId=account_private_123; api_key=sk-live-secret",
        diagnostic: {
          identity: '"Alice Example"',
          source: "Zenith API",
          requestId: "relay-123",
        },
      },
    });

    const text = JSON.stringify(diagnostic);
    expect(text).not.toContain("alice@example.test");
    expect(text).not.toContain("account_private_123");
    expect(text).not.toContain("Alice Example");
    expect(text).not.toContain("sk-live-secret");
    expect(diagnostic.source).toBe("Zenith API");
    expect(diagnostic.requestId).toBe("relay-123");
  });

  test("redacts standalone local account identifiers", () => {
    expect(redactFeedbackText("candidate account_local_private failed")).toBe("candidate [redacted identity] failed");
  });
});
