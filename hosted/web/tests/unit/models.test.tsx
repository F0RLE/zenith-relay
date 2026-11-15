import { describe, expect, test } from "bun:test";
import { renderToStaticMarkup } from "react-dom/server";
import { LocaleProvider } from "../../src/i18n/LocaleProvider";
import { ModelsPage } from "../../src/features/models/ModelsPage";
import { CatalogChoice } from "../../src/features/models/CatalogChoice";
import { formatModelPrice, modelContextRanges, modelRate, modelTierVariant, pricePresentation, selectedModelVariant } from "../../src/features/models/model";
import type { CustomerModel } from "../../src/features/models/model";
import { previewModels } from "../../src/preview/models";

const base: CustomerModel = {
  id: "test-model", name: "Test model", description: "", protocols: [], revision: "test-v1",
  rates: { input: 400000n, output: 1200000n },
};
const render = (models: CustomerModel[]) => renderToStaticMarkup(<LocaleProvider><ModelsPage models={models} /></LocaleProvider>);

describe("Server-owned model pricing", () => {
  test("missing prices remain unknown and explicit zero remains a price", () => {
    expect(modelRate(undefined)).toBeNull();
    expect(modelRate(null)).toBeNull();
    expect(modelRate(-1n)).toBeNull();
    expect(modelRate(0n)).toBe(0n);
    expect(pricePresentation(base, "cacheRead")).toEqual({ price: null, official: null, discount: null });
    const html = render([{ ...base, rates: { input: 0n, output: null } }]);
    expect(html).toContain("Our price: $0");
    expect(html.match(/Price not published/g)).toHaveLength(3);
    expect(html).not.toContain('class="catalog-saving"');
  });

  test("the server percentage is displayed, never derived from price arithmetic", () => {
    const model: CustomerModel = { ...base, comparison: { officialRates: { input: 2000000n }, savingsPercent: { input: 73.125 } } };
    expect(pricePresentation(model, "input").discount).toBe(73.125);
    expect(render([model])).toContain("−73.125%");
    expect(render([model])).not.toContain("−80%");
  });

  test("a reference price alone never invents a percentage", () => {
    const model: CustomerModel = { ...base, comparison: { officialRates: { input: 2000000n } } };
    expect(pricePresentation(model, "input")).toEqual({ price: 400000n, official: 2000000n, discount: null });
    const html = render([model]);
    expect(html).toContain("Official");
    expect(html).not.toContain('class="catalog-saving"');
  });

  test("a lower input price never claims an output or model-wide discount", () => {
    const model: CustomerModel = { ...base, comparison: { officialRates: { input: 2000000n }, savingsPercent: { input: 80, output: 98 } } };
    expect(pricePresentation(model, "input").discount).toBe(80);
    expect(pricePresentation(model, "output")).toEqual({ price: 1200000n, official: null, discount: null });
    expect(render([model]).match(/class="catalog-saving"/g)).toHaveLength(1);
  });

  test("invalid, negative or misleading comparison values cannot become badges", () => {
    for (const value of [0, -2, 101, Infinity, NaN, 100]) {
      const model: CustomerModel = { ...base, comparison: { officialRates: { input: 2000000n }, savingsPercent: { input: value } } };
      expect(pricePresentation(model, "input").discount).toBeNull();
    }
    for (const official of [null, -1n, 0n, 400000n, 200000n]) {
      const model: CustomerModel = { ...base, comparison: { officialRates: { input: official }, savingsPercent: { input: 98 } } };
      expect(pricePresentation(model, "input").official).toBeNull();
      expect(pricePresentation(model, "input").discount).toBeNull();
    }
    const free: CustomerModel = { ...base, rates: { input: 0n }, comparison: { officialRates: { input: 1n }, savingsPercent: { input: 100 } } };
    expect(pricePresentation(free, "input").discount).toBe(100);
  });

  test("prices stay exact to micro-USD and use the active locale", () => {
    expect(formatModelPrice(null, "en")).toBe("—");
    expect(formatModelPrice(0n, "en")).toBe("$0");
    expect(formatModelPrice(1n, "en")).toBe("$0.000001");
    expect(formatModelPrice(125000n, "en")).toBe("$0.125");
    expect(formatModelPrice(125000n, "ru")).toBe("$0,125");
    expect(formatModelPrice(125000n, "zh-CN")).toBe("$0.125");
    expect(formatModelPrice(9007199254740993123456n, "en")).toBe("$9,007,199,254,740,993.123456");
  });

  test("published variants choose their own prices without inventing tier-context combinations", () => {
    const model = previewModels[0]!;
    expect(selectedModelVariant(model)!.id).toBe("standard-short");
    expect(selectedModelVariant(model, "missing")!.id).toBe("standard-short");
    const long = selectedModelVariant(model, "standard-long")!;
    expect(long.rates.input).toBe(800000n);
    const flex = modelTierVariant(model, long, "flex")!;
    expect(flex.id).toBe("flex-short");
    expect(pricePresentation(flex, "input")).toEqual({ price: 200000n, official: null, discount: null });
    expect(pricePresentation(flex, "cacheWrite")).toEqual({ price: null, official: null, discount: null });
    expect(modelTierVariant(model, long, "unpublished")).toBeUndefined();
    expect(selectedModelVariant(base)).toBeUndefined();
    expect(selectedModelVariant({ ...model, pricingVariants: [] })).toBeUndefined();
    expect(selectedModelVariant({ ...model, defaultPricingVariantId: "standard-long" })!.id).toBe("standard-long");
  });

  test("plain models do not invent selectors and protocol has its own column", () => {
    const html = render([{ ...base, protocols: ["Responses"] }]);
    expect(html).not.toContain('role="combobox"');
    expect(html).toContain('class="catalog-protocol-cell"');
    expect(html).toContain("Responses");
    expect(html).not.toContain('class="catalog-toolbar"');
  });

  test("catalog choices are static for one, inline for two, and menus only above two", () => {
    const options = ["Standard", "Flex", "Priority"].map(value => ({ value, label: value }));
    const choice = (count: number) => renderToStaticMarkup(<LocaleProvider><CatalogChoice label="Pricing tier" value="Standard" options={options.slice(0, count)} onChange={() => {}} /></LocaleProvider>);
    expect(choice(0)).toBe("");
    expect(choice(1)).toContain('class="catalog-static-choice"');
    expect(choice(1)).not.toContain("<button");
    expect(choice(2).match(/aria-pressed=/g)).toHaveLength(2);
    expect(choice(2)).toContain('aria-pressed="true" aria-label="Standard"');
    expect(choice(2)).toContain('aria-pressed="false" aria-label="Flex"');
    expect(choice(2)).not.toContain('role="combobox"');
    expect(choice(3)).toContain('role="combobox"');
    expect(choice(3)).not.toContain('aria-pressed=');
  });

  test("model identity has no tier control and context keeps maximum context only as a hint", () => {
    const model = previewModels[0]!;
    const html = render([{ ...model, pricingVariants: model.pricingVariants!.filter(variant => variant.tier === "standard" || variant.tier === "flex") }]);
    expect(html).toContain('class="catalog-model-heading"><strong class="catalog-model-name">GPT</strong></div>');
    expect(html).not.toContain('class="catalog-tier"');
    expect(html).not.toContain("Pricing tier for");
    expect(html.match(/aria-pressed=/g)).toHaveLength(2);
    expect(html).not.toContain('role="combobox"');
    expect(html).toContain('title="Maximum context window: 128K"');
    expect(html).not.toContain('<span>128K</span>');
    expect(html).toContain('aria-label="Short · ≤ 100K"');
    expect(html).toContain('aria-label="Long · &gt; 100K"');
    expect(render([{ ...base, contextTokens: 128000 }])).toContain('<span>128K</span>');
  });

  test("core bands are ordered independently of maximum context and preserve multiple thresholds", () => {
    const variant = previewModels[0]!.pricingVariants![0]!;
    const model: CustomerModel = { ...base, contextTokens: 1000000, pricingVariants: [
      { ...variant, id: "last", contextBand: "above272k" },
      { ...variant, id: "base", contextBand: "base" },
      { ...variant, id: "middle", contextBand: "above128k" },
      { ...variant, id: "flex", tier: "flex", contextBand: "base" },
    ] };
    expect(modelContextRanges(model, "standard")).toEqual([
      { band: "base", min: undefined, max: 128000 },
      { band: "above128k", min: 128000, max: 272000 },
      { band: "above272k", min: 272000, max: undefined },
    ]);
    expect(modelContextRanges(model, "flex")).toEqual([{ band: "base", min: undefined, max: undefined }]);
    expect(modelContextRanges(model, "ultrafast")).toEqual([]);
    expect(modelContextRanges(base, "standard")).toEqual([]);
    expect(render([model])).toContain("&gt; 272K");
    expect(render([{ ...model, pricingVariants: [model.pricingVariants![0]!] }])).toContain("Long · &gt; 272K");
  });

  test("ultrafast is a published core tier, not a frontend-invented multiplier", () => {
    const model = previewModels[0]!;
    const selected = modelTierVariant(model, selectedModelVariant(model)!, "ultrafast")!;
    expect(selected.rates.input).toBe(1000000n);
    expect(pricePresentation(selected, "cacheWrite").price).toBeNull();
    expect(pricePresentation(selected, "input").discount).toBeNull();
    const html = render([{ ...model, defaultPricingVariantId: selected.id }]);
    expect(html).toContain('data-variant-id="ultrafast-short"');
    expect(html).toContain("Our price: $1");
    expect(html).not.toContain("Ultrafast");
    expect(html).not.toContain('class="catalog-tier"');
  });

  test("core cache-write TTL rates stay distinct and explicit zero is preserved", () => {
    const html = render([{ ...base, rates: { ...base.rates, cacheWrite5m: 125000n, cacheWrite1h: 0n } }]);
    expect(html).toContain('data-component="cacheWrite5m"');
    expect(html).toContain('data-component="cacheWrite1h"');
    expect(html).toContain("5 min");
    expect(html).toContain("1 hour");
    expect(html).toContain("Our price: $0.125");
    expect(html).toContain("Our price: $0");
    expect(html).not.toContain('data-component="cacheWrite"');
  });

  test("catalog renders each model once without introductory cards or a metadata strip", () => {
    const models = Array.from({ length: 4 }, (_, index) => ({ ...base, id: `model-${index}`, featured: true }));
    const html = render(models);
    expect(html).not.toContain('class="catalog-offer"');
    expect(html).not.toContain('class="catalog-intro"');
    expect(html).not.toContain('class="catalog-list-heading"');
    expect(html.match(/class="catalog-row"/g)).toHaveLength(4);
    expect(html).not.toContain('class="catalog-unit"');
    expect(html).toContain('class="table-shell catalog-frame"');
    expect(html).not.toContain('class="catalog-meta"');
    expect(html).not.toContain('class="catalog-controls"');
  });

  test("empty catalogs never fall back to a sample model, price or action", () => {
    const html = render([]);
    expect(html).toContain("No published models yet");
    expect(html).not.toContain("GPT");
    expect(html).not.toContain("Get an API key");
    expect(html).not.toContain('class="catalog-offer"');
    expect(html).not.toContain('class="catalog-price"');
  });

  test("catalog keeps price comparisons without demo or unit header labels", () => {
    const html = render(previewModels);
    expect(html).not.toContain("Illustrative data");
    expect(html).not.toContain("USD per 1M tokens");
    expect(html).not.toContain('class="catalog-demo-note"');
    expect(html).toContain("−98%");
    expect(html).toContain("−80%");
    expect(html).toContain("−60%");
  });

  test("server copy is escaped, missing context is not zero, and owner display has no key action", () => {
    const html = render([{ ...base, name: "<script>test</script>", description: "<img onerror=bad>", featured: true, contextTokens: null }]);
    expect(html).toContain("&lt;script&gt;");
    expect(html).not.toContain("<script>");
    expect(html).not.toContain("Get an API key");
    expect(html).not.toContain("Primary API");
    expect(html).not.toContain('role="dialog"');
  });
});
