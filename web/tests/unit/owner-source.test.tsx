import { describe, expect, test } from "bun:test";
import { renderToStaticMarkup } from "react-dom/server";
import { LocaleProvider } from "../../src/i18n/LocaleProvider";
import { OwnerSourceBody } from "../../src/features/owner/OwnerSourceBody";
import { sourceEndpoint, sourceModelProtocols, validSourceConfiguration, validSourceEndpoint } from "../../src/features/owner/sourcePresentation";
import type { OwnerSourceConfiguration } from "../../src/features/owner/sourcePresentation";
import { updateConnections } from "../../src/features/owner/model";
import { previewSnapshot } from "../../src/preview/snapshot";
import type { SourceStats } from "../../../src/src/features/relay/api/types/source";

const source = previewSnapshot.owner.connections.find(item => item.kind === "api")!;
const configuration: OwnerSourceConfiguration = { name: source.name, baseUrl: source.source!.baseUrl, wireApi: source.source!.wireApi, models: source.models };
const renderStats = (stats?: SourceStats | null) => renderToStaticMarkup(<LocaleProvider><OwnerSourceBody source={{ ...source.source!, providerStats: stats ?? null }} modelCount={3} valuesVisible /></LocaleProvider>);

describe("Hosted API source presentation and local configuration", () => {
  test("provider amounts reuse Desktop currency semantics without invented balances or spend", () => {
    expect(renderStats(null)).toContain("—");
    expect(renderStats(null)).not.toContain("$0.00");
    const stats = { ...source.source!.providerStats!, balanceMicroUsd: 0, spentMicroUsd: 1250000 };
    expect(renderStats(stats)).toContain("$0.00");
    expect(renderStats(stats)).toContain("$1.25");
    expect(renderStats({ ...stats, status: "unauthorized" })).not.toContain("$1.25");
    expect(renderStats({ ...stats, stale: true })).toContain('data-stale="true"');
    expect(renderStats({ ...stats, amounts: [{ currency: "CNY", balanceMicros: 3000000, spentMicros: null }, { currency: "CREDITS", balanceMicros: 2500000, spentMicros: 0 }] })).toContain("credits");
    expect(renderStats({ ...stats, balanceUnlimited: true, balanceKind: "key_quota" })).toContain("Unlimited");
    expect(renderStats({ ...stats, balanceUnlimited: true, balanceKind: "key_quota" })).toContain("Key allowance");
  });

  test("source protocols use resolved core bindings and never infer support from a model name", () => {
    expect(sourceModelProtocols(source.source, source.models, "demo-gpt")).toEqual(["Responses"]);
    expect(sourceModelProtocols(source.source, source.models, "demo-claude")).toEqual(["Messages"]);
    expect(sourceModelProtocols({ ...source.source!, resolvedProtocolBindings: [] }, source.models, "demo-gpt")).toEqual([]);
    expect(sourceModelProtocols(undefined, ["gpt-example"], "gpt-example")).toEqual([]);
  });

  test("endpoint display strips credentials while forms reject them", () => {
    expect(sourceEndpoint("https://user:password@example.com/v1?token=example#private")).toBe("https://example.com/v1");
    for (const endpoint of ["not a url", "javascript:alert(1)", "file:///private", "https://user:password@example.com", "https://example.com?key=example", "https://example.com#private"]) {
      expect(validSourceEndpoint(endpoint)).toBe(false);
      expect(validSourceConfiguration({ ...configuration, baseUrl: endpoint })).toBe(false);
    }
    expect(validSourceEndpoint("http://localhost:8080/v1")).toBe(true);
  });

  test("configuration rejects duplicate or invalid IDs before updating state", () => {
    for (const models of [["one", "ONE"], [""], ["has space"], ["x".repeat(201)], Array.from({ length: 257 }, (_, i) => `model-${i}`)]) {
      expect(validSourceConfiguration({ ...configuration, models })).toBe(false);
    }
    const current = [source];
    expect(updateConnections(current, { type: "saveSource", id: source.id, configuration: { ...configuration, name: " " } })).toBe(current);
  });

  test("rename preserves observations and membership, target edits invalidate old evidence", () => {
    const renamed = updateConnections([source], { type: "saveSource", id: source.id, configuration: { ...configuration, name: "Renamed" } })[0]!;
    expect(renamed.name).toBe("Renamed");
    expect(renamed.inPool).toBe(source.inPool);
    expect(renamed.source!.providerStats).toEqual(source.source!.providerStats);
    expect(renamed.source!.resolvedProtocolBindings).toEqual(source.source!.resolvedProtocolBindings);
    const expanded = updateConnections([source], { type: "saveSource", id: source.id, configuration: { ...configuration, models: [...source.models, "new-model"] } })[0]!;
    expect(sourceModelProtocols(expanded.source, expanded.models, "demo-gpt")).toEqual(["Responses"]);
    expect(sourceModelProtocols(expanded.source, expanded.models, "new-model")).toEqual([]);
    const removed = updateConnections([source], { type: "saveSource", id: source.id, configuration: { ...configuration, models: ["demo-gpt"] } })[0]!;
    expect(sourceModelProtocols(removed.source, removed.models, "demo-gpt")).toEqual(["Responses"]);
    expect(sourceModelProtocols(removed.source, removed.models, "demo-claude")).toEqual([]);
    const edited = updateConnections([source], { type: "saveSource", id: source.id, configuration: { ...configuration, baseUrl: "https://new.example.com/v1" } })[0]!;
    expect(edited.source!.providerStats).toBeNull();
    expect(edited.source!.resolvedProtocolBindings).toEqual([]);
    const added = updateConnections([], { type: "saveSource", id: "new", configuration })[0]!;
    expect(added.inPool).toBe(false);
    expect(added.source!.resolvedProtocolBindings).toEqual([]);
    expect(added.source!.providerStats).toBeNull();
    expect(updateConnections([added], { type: "saveSource", id: "new", configuration })).toHaveLength(1);
  });

  test("source edits cannot overwrite accounts or restore trash", () => {
    const account = previewSnapshot.owner.connections[0]!;
    const current = [account, { ...source, trashed: true }];
    for (const item of current) expect(updateConnections(current, { type: "saveSource", id: item.id, configuration })).toBe(current);
  });
});
