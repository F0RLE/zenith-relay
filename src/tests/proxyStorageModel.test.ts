import { describe, expect, test } from "bun:test";
import { buildProxyBindings, proxyTimezone } from "../src/features/relay/pages/connections/proxyStorageModel";

describe("proxy storage bindings", () => {
  test("fresh timezone results replace or clear storage without losing old-host compatibility", () => {
    const entry = { timezone: "Europe/Amsterdam" };
    const check = { proxyId: "synthetic", checkedAtMs: 1, elapsedMs: 1, ip: "203.0.113.9", countryCode: "NL", errorCode: null };
    expect(proxyTimezone(entry)).toBe("Europe/Amsterdam");
    expect(proxyTimezone(entry, check)).toBe("Europe/Amsterdam");
    expect(proxyTimezone(entry, { ...check, timezone: "Asia/Tokyo" })).toBe("Asia/Tokyo");
    expect(proxyTimezone(entry, { ...check, timezone: null })).toBeNull();
  });
  test("merges explicit and common accounts once while preserving their order", () => {
    const bindings = buildProxyBindings({
      entries: [{ id: "proxy-a", assignedAccountIds: ["a", "missing", "a"] }],
      accounts: [
        { id: "a", label: "Account A", proxyMode: "common" },
        { id: "b", label: "Account B", proxyMode: "common" },
        { id: "c", label: "Account C", proxyMode: "direct" },
      ],
      sources: [], commonProxyId: "proxy-a", unknownAccountLabel: "Unknown",
    });
    expect(bindings.get("proxy-a")).toEqual({
      accountNames: ["Account A", "Unknown", "Account B"], sourceNames: [], isCommon: true,
    });
  });

  test("keeps source bindings scoped and ordered without inventing account bindings", () => {
    const bindings = buildProxyBindings({
      entries: [
        { id: "proxy-a", assignedAccountIds: [] },
        { id: "proxy-b", assignedAccountIds: [] },
      ],
      accounts: [{ id: "a", label: "Account A", proxyMode: "common" }],
      sources: [
        { proxyId: "proxy-b", name: "First" }, { proxyId: null, name: "Direct" },
        { proxyId: "proxy-a", name: "Other" }, { proxyId: "proxy-b", name: "Second" },
      ],
      commonProxyId: null, unknownAccountLabel: "Unknown",
    });
    expect(bindings.get("proxy-a")).toEqual({ accountNames: [], sourceNames: ["Other"], isCommon: false });
    expect(bindings.get("proxy-b")).toEqual({ accountNames: [], sourceNames: ["First", "Second"], isCommon: false });
  });
});
