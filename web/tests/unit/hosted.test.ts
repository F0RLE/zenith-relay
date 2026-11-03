import { describe, expect, test } from "bun:test";
import { customerPages, hostedPageUrl, ownerPages, readHostedRoute } from "../../src/features/app/routes";
import { updateConnections } from "../../src/features/owner/model";
import { previewSnapshot } from "../../src/preview/snapshot";

describe("Hosted frontend routes", () => {
  test("customer status follows models and retains the legacy preview route", () => {
    expect(customerPages.map(page => page.id)).toEqual(["overview", "usage", "models", "status", "keys", "billing"]);
    expect(readHostedRoute(new URL("https://example.test/preview.html?page=status"))).toEqual({ area: "customer", page: "status", found: true });
    expect(hostedPageUrl("customer", "status", new URL("https://example.test/app/models")).pathname).toBe("/app/status");
  });
  test("owner navigation includes notifications after usage and no retired route aliases", () => {
    expect(ownerPages.map(page => page.id)).toEqual(["overview", "connections", "pools", "models", "usage", "notifications"]);
    for (const id of ["tariffs", "access", "customers", "payments", "operations", "audit"]) {
      expect(readHostedRoute(new URL(`https://example.test/admin/${id}`)).found).toBe(false);
    }
  });
  test("every canonical destination has a distinct area and page", () => {
    for (const page of customerPages) expect(readHostedRoute(new URL(`https://example.test/app/${page.id}`))).toEqual({ area: "customer", page: page.id, found: true });
    for (const page of ownerPages) expect(readHostedRoute(new URL(`https://example.test/admin/${page.id}`))).toEqual({ area: "owner", page: page.id, found: true });
  });
  test("unknown, local and cross-area destinations are not overview aliases", () => {
    for (const path of ["/app/connections", "/app/local", "/admin/billing", "/admin/status", "/app/overview/private", "/administrator/overview", "/app/profiles", "/app/integrations"]) expect(readHostedRoute(new URL(`https://example.test${path}`)).found).toBe(false);
  });
  test("canonical links drop stale query and hash, legacy preview stays compatible", () => {
    expect(hostedPageUrl("customer", "keys", new URL("https://example.test/app/models?welcome=1#old")).href).toBe("https://example.test/app/keys");
    expect(hostedPageUrl("owner", "pools", new URL("https://example.test/admin/connections?filter=old")).pathname).toBe("/admin/pools");
    expect(hostedPageUrl("customer", "usage", new URL("https://example.test/preview.html")).search).toBe("?page=usage");
  });
  test("removed help routes are not aliases while owner connections remain available", () => {
    for (const path of ["/app/help", "/preview.html?page=help", "/admin/help"]) expect(readHostedRoute(new URL(`https://example.test${path}`)).found).toBe(false);
    expect(readHostedRoute(new URL("https://example.test/admin/connections"))).toEqual({ area: "owner", page: "connections", found: true });
  });
  test("settings are removed from both workspaces and the legacy preview", () => {
    for (const pages of [customerPages, ownerPages]) expect(pages.some(page => String(page.id) === "settings")).toBe(false);
    for (const path of ["/app/settings", "/admin/settings", "/preview.html?page=settings"]) expect(readHostedRoute(new URL(`https://example.test${path}`)).found).toBe(false);
  });
});

describe("Hosted synthetic resource state", () => {
  const connections = previewSnapshot.owner.connections;
  test("saved connections do not automatically enter the pool", () => {
    const next = updateConnections(connections, { type: "add", connection: { ...connections[0]!, id: "new", inPool: true } });
    expect(next.at(-1)?.inPool).toBe(false);
    expect(next.length).toBe(connections.length + 1);
  });
  test("removing membership preserves the saved source", () => {
    const next = updateConnections(connections, { type: "pool", id: connections[0]!.id });
    expect(next[0]?.inPool).toBe(false);
    expect(next[0]?.trashed).toBe(false);
    expect(next.length).toBe(connections.length);
  });
  test("batch admission is explicit, idempotent and preserves pause and inventory", () => {
    const outside = connections.map((item, index) => ({ ...item, inPool: false, paused: index === 0 }));
    const next = updateConnections(outside, { type: "addMembers", ids: [outside[0]!.id, outside[3]!.id, outside[0]!.id] });
    expect(next.filter(item => item.inPool).map(item => item.id)).toEqual([outside[0]!.id, outside[3]!.id]);
    expect(next[0]!.paused).toBe(true);
    expect(next[1]).toBe(outside[1]);
    expect(next[3]!.source).toBe(outside[3]!.source);
    expect(outside.every(item => !item.inPool)).toBe(true);
    expect(updateConnections(next, { type: "addMembers", ids: [outside[0]!.id, outside[3]!.id] })).toBe(next);
  });
  test("batch admission ignores deleted, trashed and already included selections", () => {
    const current = connections.map((item, index) => index === 3 ? { ...item, trashed: true, inPool: false } : item);
    expect(updateConnections(current, { type: "addMembers", ids: [current[0]!.id, current[3]!.id, "removed-id"] })).toBe(current);
    expect(updateConnections(current, { type: "addMembers", ids: [] })).toBe(current);
  });
  test("trash and restore cannot silently reactivate a source", () => {
    const trashed = updateConnections(connections, { type: "trash", id: connections[0]!.id });
    expect(trashed[0]?.inPool).toBe(false);
    const restored = updateConnections(trashed, { type: "restore", id: connections[0]!.id });
    expect(restored[0]?.trashed).toBe(false);
    expect(restored[0]?.inPool).toBe(false);
    expect(updateConnections(trashed, { type: "pool", id: connections[0]!.id })[0]?.inPool).toBe(false);
  });
  test("reordering respects pool membership and does not mutate fixtures", () => {
    const first = connections[0]!.id;
    expect(updateConnections(connections, { type: "move", id: first, direction: -1 })).toBe(connections);
    expect(updateConnections(connections, { type: "move", id: first, direction: 1 })[1]?.id).toBe(first);
    expect(connections[0]?.id).toBe(first);
    expect(updateConnections(connections, { type: "move", id: "missing", direction: 1 })).toBe(connections);
  });
});
