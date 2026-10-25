import { describe, expect, test } from "bun:test";
import type { LocalUsage, LocalUsagePage } from "../src/features/relay/api/types";
import { createDisplaySnapshotCache } from "../src/features/relay/state/displaySnapshotCache";
import { shareUsageFilterChoices } from "../src/features/relay/state/usagePageSharing";
import { rememberUsagePreview, usagePreview } from "../src/features/relay/state/usagePreview";
import { emptyUsageTotals } from "../src/features/relay/usageTotals";

function report(fields: Partial<LocalUsagePage> = {}): LocalUsagePage {
  return {
    events: [], total: 0, page: 1, pageSize: 50, totalPages: 0,
    totals: emptyUsageTotals(), models: [], poolMembers: [], ...fields,
  };
}

const usageEvent = (fields: Partial<LocalUsage> = {}): LocalUsage => ({
  id: 1, requestId: "synthetic-request", attempt: 1, createdAt: "2026-10-25T00:00:00Z",
  sourceId: "synthetic-source", requestedModel: "synthetic-model", resolvedModel: "synthetic-model",
  wireApi: "responses", transport: "http", success: true, httpStatus: 200, errorCategory: null,
  latencyMs: 100, ttftMs: null, generationMs: null, inputTokens: 1, cachedInputTokens: null,
  reasoningTokens: null, outputTokens: 1, totalTokens: 2, ...fields,
});

describe("usage preview memory", () => {
  test("unchanged IPC copies reuse the preview while changed facts replace it", async () => {
    const valid = (value: unknown): value is LocalUsagePage => value != null && typeof value === "object";
    const preview = createDisplaySnapshotCache("usage", 8, valid, 1);
    const original = report({ events: [usageEvent()] });
    try {
      preview.remember("selected", original);
      preview.remember("selected", structuredClone(original));
      expect(preview.peek("selected")).toBe(original);
      const changed = structuredClone(original);
      changed.events[0].totalTokens = 3;
      preview.remember("selected", changed);
      expect(preview.peek("selected")).toBe(changed);
      preview.remember("selected", { ...changed, events: [] });
      expect(preview.peek("selected")?.events).toEqual([]);
    } finally { await preview.clear(); }
  });

  test("keeps one hot preview without changing the default cache limit", async () => {
    const valid = (value: unknown): value is { page: number } => value != null && typeof value === "object" && "page" in value;
    const preview = createDisplaySnapshotCache("usage", 8, valid, 1);
    const defaults = createDisplaySnapshotCache("runtime", 2, valid);
    try {
      for (let page = 1; page <= 8; page += 1) {
        const data = { page };
        preview.remember(String(page), data);
        defaults.remember(String(page), data);
      }
      expect(Array.from({ length: 8 }, (_, index) => preview.peek(String(index + 1))).filter(Boolean))
        .toEqual([{ page: 8 }]);
      expect(defaults.peek("6")).toBeNull();
      expect(defaults.peek("7")).toEqual({ page: 7 });
      expect(defaults.peek("8")).toEqual({ page: 8 });
    } finally {
      await preview.clear();
      await defaults.clear();
    }
  });

  test("shares reports without provider errors instead of copying every row", async () => {
    const current = report({ events: [usageEvent(), usageEvent({ id: 2, upstreamError: null })] });
    Object.freeze(current.events[0]);
    Object.freeze(current.events[1]);
    Object.freeze(current.events);
    Object.freeze(current);
    try {
      rememberUsagePreview("no-errors", current);
      expect(usagePreview.peek("no-errors")).toBe(current);
      expect(usagePreview.peek("no-errors")?.events).toBe(current.events);
      rememberUsagePreview("latest", report({ page: 2 }));
      expect(usagePreview.peek("no-errors")).toBeNull();
    } finally {
      await usagePreview.clear();
    }
  });

  test("redacts provider errors while sharing unaffected rows and leaving native data intact", async () => {
    const clean = usageEvent();
    const failed = usageEvent({ id: 2, success: false, httpStatus: 502, upstreamError: {
      httpStatus: 502, code: "synthetic_error", errorType: null,
      message: "Synthetic provider diagnostic", redacted: false, truncated: false,
    } });
    const current = report({ events: [clean, failed] });
    const before = structuredClone(current);
    try {
      rememberUsagePreview("with-error", current);
      const preview = usagePreview.peek("with-error");
      expect(preview).not.toBe(current);
      expect(preview?.events[0]).toBe(clean);
      expect(preview?.events[1]).toEqual({ ...failed, upstreamError: null });
      expect(preview?.events[1]).not.toBe(failed);
      expect(preview?.totals).toBe(current.totals);
      expect(current).toEqual(before);
    } finally {
      await usagePreview.clear();
    }
  });
});

describe("usage choice sharing", () => {
  test("copies choices only when it can actually share an older array", () => {
    let copies = 0;
    const choices = new Proxy({ models: ["new"] }, {
      ownKeys(target) { copies += 1; return Reflect.ownKeys(target); },
    });
    const next = report({ filterOptions: choices });
    expect(shareUsageFilterChoices(next, { models: ["old"] })).toBe(next);
    expect(copies).toBe(0);
    const older = { models: ["new"] };
    const shared = shareUsageFilterChoices(next, older);
    expect(copies).toBe(1);
    expect(shared.filterOptions?.models).toBe(older.models);
    expect(next.filterOptions?.models).toBe(choices.models);
  });

  test("repeated reports share every unchanged choice array, not request facts", () => {
    const original = report({ filterOptions: {
      models: Array.from({ length: 1_000 }, (_, index) => `model-${index}`),
      poolMembers: ["synthetic-member"], wireApis: ["responses"],
      transports: ["http"], errorCategories: ["upstream_failure"],
    } });
    const unsharedReports = [original];
    const sharedReports = [original];
    for (let page = 2; page <= 8; page += 1) {
      const next = structuredClone(original);
      next.page = page;
      next.totals.requests = page;
      const shared = shareUsageFilterChoices(next, original.filterOptions);
      unsharedReports.push(next);
      sharedReports.push(shared);
      expect(shared.filterOptions?.models).toBe(original.filterOptions?.models);
      expect(shared.filterOptions?.poolMembers).toBe(original.filterOptions?.poolMembers);
      expect(shared.filterOptions?.wireApis).toBe(original.filterOptions?.wireApis);
      expect(shared.filterOptions?.transports).toBe(original.filterOptions?.transports);
      expect(shared.filterOptions?.errorCategories).toBe(original.filterOptions?.errorCategories);
      expect(shared.events).toBe(next.events);
      expect(shared.totals).toBe(next.totals);
      expect(shared.page).toBe(page);
      expect(original.totals.requests).toBe(0);
    }
    expect(new Set(unsharedReports.map((page) => page.filterOptions?.models)).size).toBe(8);
    expect(new Set(sharedReports.map((page) => page.filterOptions?.models)).size).toBe(1);
  });

  test("new or reordered choices remain authoritative and do not mutate either report", () => {
    const original = report({ filterOptions: { models: ["a", "b"], poolMembers: ["old"] } });
    for (const models of [["a", "c"], ["b", "a"], []]) {
      const next = report({ filterOptions: { models, poolMembers: ["new"] } });
      const shared = shareUsageFilterChoices(next, original.filterOptions);
      expect(shared.filterOptions?.models).toBe(models);
      expect(shared.filterOptions?.poolMembers).toEqual(["new"]);
      expect(original.filterOptions).toEqual({ models: ["a", "b"], poolMembers: ["old"] });
    }
  });

  test("missing legacy fields remain missing, rather than borrowing old choices", () => {
    const older = report({ filterOptions: { models: ["a"], wireApis: ["responses"] } });
    const next = report({ filterOptions: { wireApis: ["responses"] } });
    const shared = shareUsageFilterChoices(next, older.filterOptions);
    expect(shared.filterOptions).not.toHaveProperty("models");
    expect(shared.filterOptions?.wireApis).toBe(older.filterOptions?.wireApis);
    const absent = report();
    expect(shareUsageFilterChoices(absent, older.filterOptions)).toBe(absent);
    expect(shareUsageFilterChoices(next, undefined)).toBe(next);
  });
});
