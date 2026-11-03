import { describe, expect, test } from "bun:test";
import { renderToStaticMarkup } from "react-dom/server";
import { projectPoolActivity, poolActivityKey } from "../../src/features/owner/poolActivity";
import type { OwnerPoolActivity } from "../../src/features/owner/poolActivity";
import type { OwnerConnection } from "../../src/features/owner/model";
import { OwnerPoolRuntime } from "../../src/features/owner/OwnerPoolRuntime";
import { OwnerResourceCard } from "../../src/features/owner/OwnerResourceCard";
import { LocaleProvider } from "../../src/i18n/LocaleProvider";

const account: OwnerConnection = { id: "account", kind: "account", name: "Test account", family: "test", inPool: true, paused: false, trashed: false, models: ["model-a"] };
const source: OwnerConnection = { ...account, id: "source", kind: "api", name: "Test API" };
const live: OwnerPoolActivity = { members: [{ id: account.id, kind: account.kind, activeRequests: 2, models: [{ id: "model-a", requests: 2 }] }], next: { id: source.id, kind: source.kind } };

describe("pool activity presentation", () => {
  test("only explicit in-flight evidence lights a member, never selection or ordering", () => {
    const projected = projectPoolActivity([account, source], live);
    expect(projected.total).toBe(2);
    expect(projected.current).toEqual([account]);
    expect(projected.memberRequests.get(poolActivityKey(source))).toBeUndefined();
    expect(projected.next).toBe(source);
    expect(projectPoolActivity([source, account], live).current).toEqual([account]);
    expect(projectPoolActivity([account], { members: [] }).memberRequests.size).toBe(0);
  });

  test("unknown telemetry, observed idle and no model attribution are distinct", () => {
    expect(projectPoolActivity([account]).observed).toBe(false);
    expect(projectPoolActivity([account], { members: [] }).observed).toBe(true);
    const activity = projectPoolActivity([account], { members: [{ id: account.id, kind: "account", activeRequests: 1, models: [] }] });
    expect(activity.total).toBe(1);
    expect(activity.models).toEqual([]);
  });

  test("aggregates simultaneous accounts and model counts without modifying inventory", () => {
    const activity = { ...live, members: [...live.members, { id: source.id, kind: source.kind, activeRequests: 3, models: [{ id: "MODEL-A", requests: 1 }, { id: "runtime-only", requests: 2 }] }] };
    const projected = projectPoolActivity([account, source], activity);
    expect(projected.total).toBe(5);
    expect(projected.current).toEqual([account, source]);
    expect(projected.models).toEqual([{ id: "model-a", requests: 3 }, { id: "runtime-only", requests: 2 }]);
    expect(account.models).toEqual(["model-a"]);
  });

  test("ignores stale membership, invalid counts and duplicate rows; uses qualified identity", () => {
    const bad = { ...live.members[0]!, activeRequests: 0 };
    for (const activeRequests of [0, -1, NaN, Infinity, 0.5]) expect(projectPoolActivity([account], { members: [{ ...bad, activeRequests }] }).total).toBe(0);
    expect(projectPoolActivity([{ ...account, inPool: false }], live).total).toBe(0);
    expect(projectPoolActivity([{ ...account, trashed: true }], live).total).toBe(0);
    expect(projectPoolActivity([{ ...account, kind: "api" }], live).total).toBe(0);
    expect(projectPoolActivity([account], { members: [...live.members, ...live.members] }).total).toBe(2);
    expect(projectPoolActivity([account], live).next).toBeUndefined();
  });

  test("pause does not pretend that already running requests have completed", () => {
    const paused = { ...account, paused: true };
    expect(projectPoolActivity([paused], live).total).toBe(2);
    expect(projectPoolActivity([paused], { members: [] }).total).toBe(0);
  });

  test("activity summary and card highlight consume the same explicit count", () => {
    const projected = projectPoolActivity([account, source], live);
    const summary = renderToStaticMarkup(<LocaleProvider><OwnerPoolRuntime activity={projected} mode="automatic" /></LocaleProvider>);
    expect(summary).toContain("Working now: Test account");
    expect(summary).toContain("In progress (2): model-a × 2");
    expect(summary).not.toContain("Next:");
    expect(summary).not.toContain(source.name);
    const card = (count: number, selected = false) => renderToStaticMarkup(<LocaleProvider><OwnerResourceCard connection={account} currentRequests={count} selected={selected} onSelect={() => {}} change={() => {}} nowMs={0} active models={[]} position={0} /></LocaleProvider>);
    expect(card(2)).toContain('data-current="true"');
    expect(card(0, true)).toContain('data-current="false"');
    expect(card(2)).toContain("Requests in progress: 2");
  });

  test("an idle pool never presents a next candidate supplied by telemetry", () => {
    const projected = projectPoolActivity([account, source], { ...live, members: [] });
    const summary = renderToStaticMarkup(<LocaleProvider><OwnerPoolRuntime activity={projected} mode="automatic" /></LocaleProvider>);
    expect(summary).toContain("Waiting for requests");
    expect(summary).not.toContain("Next:");
    expect(summary).not.toContain(source.name);
    expect(projected.current).toEqual([]);
  });
});
