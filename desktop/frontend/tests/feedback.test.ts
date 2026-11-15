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

  test("redacts quoted JSON fields including escaped quotes in secret values", () => {
    const text = redactFeedbackText(JSON.stringify({
      api_key: 'synthetic-key-"quoted"-tail',
      password: "synthetic-password",
      token: "synthetic-token",
      message: "upstream rejected request",
    }));
    expect(text).not.toContain("synthetic");
    expect(text).not.toContain("quoted");
    expect(text).not.toContain("tail");
    expect(text).toContain("upstream rejected request");
  });

  test("redacts URL credentials without removing the diagnostic host", () => {
    const text = redactFeedbackText("connect https://synthetic-name:synthetic-password@proxy.example.test:8443/v1 failed");
    expect(text).not.toContain("synthetic-name");
    expect(text).not.toContain("synthetic-password");
    expect(text).toContain("proxy.example.test:8443/v1");
  });

  test("redacts a JWT before truncating the diagnostic", () => {
    const text = redactFeedbackText(`${".".repeat(580)} eyJhbGciOiJub25lIn0.eyJzdWIiOiIxIn0.signature`);
    expect(text).not.toContain("eyJ");
    expect(text.length).toBeLessThanOrEqual(600);
  });
});
