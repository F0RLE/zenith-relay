import { describe, expect, test } from "bun:test";
import { renderToStaticMarkup } from "react-dom/server";
import { LocaleProvider } from "../../src/i18n/LocaleProvider";
import { OwnerModelRulesView } from "../../src/features/owner/OwnerModelRules";
import { applyOwnerModelRule, modelRateInput, parseModelRate, poolRuleModels, setOwnerModelRate } from "../../src/features/owner/modelRules";
import type { OwnerModelRules } from "../../src/features/owner/modelRules";
import type { OwnerConnection } from "../../src/features/owner/model";
import { previewModels } from "../../src/preview/models";
import type { CustomerModel } from "../../src/features/models/model";

const model = previewModels[0]!;
const member: OwnerConnection = { id: "test-account", name: "Test account", family: "common.account", kind: "account", models: [model.id], inPool: true, paused: false, trashed: false };

describe("pool-wide model rules", () => {
  test("uses current catalog and pool inventory, not route health or hardcoded models", () => {
    const added = { ...model, id: "new-provider-model", name: "New model" };
    const connections = [{ ...member, paused: true, models: [model.id, added.id, "not-in-catalog"] }];
    expect(poolRuleModels([model, added, model], connections)).toEqual([model, added]);
    expect(poolRuleModels([model, added], [{ ...member, inPool: false }])).toEqual([]);
    expect(poolRuleModels([model], [{ ...member, trashed: true }])).toEqual([]);
    expect(poolRuleModels([model], [{ ...member, modelPolicy: { allowedModels: [], excludedModels: [model.id] } }])).toEqual([model]);
    expect(poolRuleModels([], connections)).toEqual([]);
  });

  test("parses exact microUSD and distinguishes zero from unpublished", () => {
    expect(parseModelRate("0")).toBe(0n);
    expect(parseModelRate(" ")).toBeNull();
    expect(parseModelRate("0,123456")).toBe(123456n);
    expect(parseModelRate("1000000")).toBe(1000000000000n);
    expect(parseModelRate("1.")).toBe(1000000n);
    for (const value of ["-1", "1e3", "1.0000001", "1,2.3", "Infinity", "1000000.000001", "9".repeat(64)]) expect(parseModelRate(value)).toBeUndefined();
    for (const value of [0n, 1n, 10n, 1234567n, 999999999999n, 1000000000000n]) expect(parseModelRate(modelRateInput(value))).toBe(value);
    expect(modelRateInput(null)).toBe("");
  });

  test("price edits retain enablement, other components, variants and original inventory", () => {
    const original = structuredClone(model);
    let rules: OwnerModelRules = { [model.id]: { enabled: false }, untouched: { enabled: false } };
    rules = setOwnerModelRate(rules, model, "standard-short", "input", 123456n);
    rules = setOwnerModelRate(rules, model, "flex-short", "output", 0n);
    const result = applyOwnerModelRule(model, rules[model.id]);
    expect(result.rates.input).toBe(123456n);
    expect(result.rates.output).toBe(model.rates.output);
    expect(result.pricingVariants?.find(variant => variant.id === "standard-short")?.rates.input).toBe(123456n);
    expect(result.pricingVariants?.find(variant => variant.id === "flex-short")?.rates.output).toBe(0n);
    expect(result.pricingVariants?.find(variant => variant.id === "standard-long")).toBe(model.pricingVariants?.find(variant => variant.id === "standard-long"));
    expect(result.comparison).toBeUndefined();
    expect(result.pricingVariants?.find(variant => variant.id === "standard-short")?.comparison).toBeUndefined();
    expect(rules[model.id]?.enabled).toBe(false);
    expect(rules.untouched).toEqual({ enabled: false });
    expect(model).toEqual(original);
  });

  test("server-supplied defaults apply to the featured offer without discarding other prices", () => {
    const result = applyOwnerModelRule(model, { rates: { input: null, output: 0n } });
    expect(result.rates.input).toBeNull();
    expect(result.pricingVariants?.[0]?.rates.input).toBeNull();
    expect(result.pricingVariants?.[0]?.rates.output).toBe(0n);
    expect(result.pricingVariants?.[1]).toBe(model.pricingVariants?.[1]);
    expect(applyOwnerModelRule(model, { enabled: false })).toBe(model);
    const rules = {};
    expect(setOwnerModelRate(rules, model, "unknown", "input", 1n)).toBe(rules);
    expect(setOwnerModelRate(rules, model, undefined, "input", -1n)).toBe(rules);
    expect(setOwnerModelRate(rules, model, undefined, "input", 1000000000001n)).toBe(rules);
  });

  test("company groups start expanded with inline prices and separate drag handles", () => {
    const html = renderToStaticMarkup(<LocaleProvider><OwnerModelRulesView models={[model]} connections={[member]} rules={{ [model.id]: { enabled: false } }} onChange={() => {}} /></LocaleProvider>);
    expect(html).toContain('data-enabled="false"');
    expect(html).toContain('role="switch"');
    expect(html).toContain('aria-label="Input price for GPT"');
    expect(html.match(/type="text"/g)).toHaveLength(4);
    expect(html).not.toContain("owner-model-price-button");
    expect(html).not.toContain("owner-model-price-editor");
    expect(html).toContain('data-group-id="catalog-openai"');
    expect(html).toContain('scope="rowgroup"');
    expect(html).toContain('aria-label="Move company OpenAI"');
    expect(html).toContain('aria-label="Collapse OpenAI models"');
    expect(html).toContain('aria-expanded="true"');
    expect(html).toContain('aria-controls=');
    expect(html).toContain("owner-model-offer");
    expect(html).toContain("OpenAI");
    expect(html).not.toContain(" hidden");
    expect(html).not.toContain('role="dialog"');
    expect(html).not.toContain('role="radiogroup"');
    expect(html).not.toContain("draggable");
    const empty = renderToStaticMarkup(<LocaleProvider><OwnerModelRulesView models={[]} connections={[member]} rules={{}} onChange={() => {}} /></LocaleProvider>);
    expect(empty).toContain("No models in the pool");
  });

  test("standard offers show both supplied context bounds without a tier prefix", () => {
    const variant = model.pricingVariants![0]!;
    const bounded: CustomerModel = { ...model, defaultPricingVariantId: "middle", contextTokens: 1000000, pricingVariants: [
      { ...variant, id: "base", contextBand: "base" },
      { ...variant, id: "middle", contextBand: "above100k" },
      { ...variant, id: "last", contextBand: "above200k" },
    ] };
    const html = renderToStaticMarkup(<LocaleProvider><OwnerModelRulesView models={[bounded]} connections={[member]} rules={{}} onChange={() => {}} /></LocaleProvider>);
    expect(html).toContain("&gt; 100,000 – ≤ 200,000");
    expect(html).not.toContain("Standard");
    expect(html).not.toContain("1,000,000");
  });

  test("missing context stays blank instead of using Standard, a dash or maximum context", () => {
    for (const pricingVariants of [undefined, [model.pricingVariants![0]!]]) {
      const noBounds: CustomerModel = { ...model, pricingVariants, contextTokens: 1000000 };
      const html = renderToStaticMarkup(<LocaleProvider><OwnerModelRulesView models={[noBounds]} connections={[member]} rules={{}} onChange={() => {}} /></LocaleProvider>);
      expect(html).toContain('<span class="owner-model-offer-value"></span>');
      expect(html).toContain('class="owner-model-offer" data-available="false"');
      expect(html).not.toContain("Standard");
      expect(html).not.toContain('role="combobox"');
      expect(html).not.toContain("1,000,000");
    }
  });

  test("cache write durations stay visible and independently labeled", () => {
    const ttlModel = { ...model, pricingVariants: undefined, rates: { input: 1n, output: 2n, cacheRead: 3n, cacheWrite5m: 4n, cacheWrite1h: 5n } };
    const html = renderToStaticMarkup(<LocaleProvider><OwnerModelRulesView models={[ttlModel]} connections={[member]} rules={{}} onChange={() => {}} /></LocaleProvider>);
    expect(html.match(/type="text"/g)).toHaveLength(5);
    expect(html).toContain('aria-label="Cache write · 5 min price for GPT"');
    expect(html).toContain('aria-label="Cache write · 1 hour price for GPT"');
    expect(html).not.toContain('aria-label="Cache write price for GPT"');
  });
});
