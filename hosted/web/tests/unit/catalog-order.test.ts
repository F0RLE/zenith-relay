import { describe, expect, test } from "bun:test";
import { catalogGroups, moveCatalogItem } from "../../src/features/models/catalogOrder";
import type { CustomerModel } from "../../src/features/models/model";
import { projectOwnerCatalog } from "../../src/features/owner/modelRules";

const model = (id: string, publisher?: string): CustomerModel => ({ id, name: id, ...(publisher ? { publisher } : {}), protocols: [], description: "", rates: { input: 10n }, revision: "test" });
const catalog = [model("a1", "OpenAI"), model("b1", "Anthropic"), model("a2", "OpenAI"), model("b2", "Anthropic"), model("g1", "Google")];
const ids = (models: readonly CustomerModel[]) => models.map(model => model.id);

describe("company catalog order", () => {
  test("uses catalog identity, not model names or connection kinds", () => {
    const groups = catalogGroups([
      { ...model("claude-code", "Wrong label"), catalogProvider: " OPENAI " },
      model("gpt-other-company", "Anthropic"), model("gpt-unknown"),
      model("x1", "x-ai"), model("x2", "x_ai"), model("new", "New Company"),
    ]);
    expect(groups.map(group => group.provider)).toEqual(["openai", "anthropic", "other", "xai", "new company"]);
    expect(groups.map(group => group.label)).toEqual(["OpenAI", "Anthropic", "Other", "xAI", "New company"]);
    expect(ids(groups[3]!.models)).toEqual(["x1", "x2"]);
  });

  test("preserves incoming provider and model order without a second sort", () => {
    expect(catalogGroups(catalog).map(group => ids(group.models))).toEqual([["a1", "a2"], ["b1", "b2"], ["g1"]]);
    expect(ids(catalog)).toEqual(["a1", "b1", "a2", "b2", "g1"]);
  });

  test("moving a company moves its entire block, including disabled models", () => {
    const order = moveCatalogItem(catalog, [], { kind: "group", source: "catalog-openai", target: "catalog-google" });
    expect(order).toEqual(["b1", "b2", "g1", "a1", "a2"]);
    const settings = { modelDisplayOrder: order!, rules: { a1: { enabled: false }, a2: { rates: { input: 123456n } } } };
    const retail = projectOwnerCatalog(catalog, settings);
    expect(ids(retail)).toEqual(["b1", "b2", "g1", "a2"]);
    expect(retail[3]!.rates.input).toBe(123456n);
    expect(ids(projectOwnerCatalog(catalog, { ...settings, rules: { ...settings.rules, a1: { enabled: true } } }))).toEqual(order!);
    expect(catalog[2]!.rates.input).toBe(10n);
  });

  test("reorders models only inside the original company", () => {
    const order = moveCatalogItem(catalog, [], { kind: "model", source: "a1", target: "a2" });
    expect(order).toEqual(["a2", "a1", "b1", "b2", "g1"]);
    expect(moveCatalogItem(catalog, order!, { kind: "model", source: "a1", target: "b1" })).toBeNull();
    expect(moveCatalogItem(catalog, [], { kind: "model", source: "a1", target: "a1" })).toBeNull();
    expect(moveCatalogItem(catalog, [], { kind: "group", source: "missing", target: "catalog-openai" })).toBeNull();
  });

  test("new entries append within their company; stale IDs never manufacture models", () => {
    const order = ["missing", " B2 ", "b1", "a2", "a1", "A1"];
    const next = [...catalog, model("a3", "OpenAI"), model("new1", "New Company"), model("A1", "OpenAI")];
    expect(catalogGroups(next, order).map(group => ids(group.models))).toEqual([["b2", "b1"], ["a2", "a1", "a3"], ["g1"], ["new1"]]);
    expect(catalogGroups([], order)).toEqual([]);
  });
});
