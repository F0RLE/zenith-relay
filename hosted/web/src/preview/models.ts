import type { CustomerModel } from "../features/models/model";

// Synthetic presentation fixtures, not current provider prices or real offers.
// Keep today's catalog separate from historical request billing snapshots.
// Comparisons intentionally exercise different server-supplied percentages.
const gptStandardShort = {
  id: "standard-short",
  tier: "standard" as const,
  contextBand: "base" as const,
  rates: { input: 400000n, output: 1200000n, cacheRead: 100000n, cacheWrite: 500000n },
  comparison: {
    officialRates: { input: 2000000n, output: 6000000n, cacheRead: 500000n, cacheWrite: 2500000n },
    savingsPercent: { input: 80, output: 80, cacheRead: 80, cacheWrite: 80 },
  },
};

export const previewModels: CustomerModel[] = [
  {
    id: "demo-gpt", name: "GPT", publisher: "OpenAI", protocols: ["Responses"],
    description: "modelCatalog.exampleCode", contextTokens: 128000, featured: true,
    rates: gptStandardShort.rates,
    revision: "demo-catalog-v1",
    comparison: gptStandardShort.comparison,
    defaultPricingVariantId: "standard-short",
    pricingVariants: [
      gptStandardShort,
      {
        id: "standard-long", tier: "standard", contextBand: "above100k",
        rates: { input: 800000n, output: 2400000n, cacheRead: 200000n, cacheWrite: 1000000n },
        comparison: {
          officialRates: { input: 4000000n, output: 12000000n, cacheRead: 1000000n, cacheWrite: 5000000n },
          savingsPercent: { input: 80, output: 80, cacheRead: 80, cacheWrite: 80 },
        },
      },
      {
        id: "flex-short", tier: "flex", contextBand: "base",
        rates: { input: 200000n, output: 600000n, cacheRead: 50000n, cacheWrite: null },
      },
      {
        id: "priority-short", tier: "priority", contextBand: "base",
        rates: { input: 600000n, output: 1800000n, cacheRead: 150000n, cacheWrite: 750000n },
      },
      {
        id: "ultrafast-short", tier: "ultrafast", contextBand: "base",
        rates: { input: 1000000n, output: 3000000n, cacheRead: 250000n },
      },
    ],
  },
  {
    id: "demo-claude", name: "Claude", publisher: "Anthropic", protocols: ["Messages"],
    description: "modelCatalog.exampleAnalysis", contextTokens: 200000, featured: true,
    rates: { input: 600000n, output: 2000000n, cacheRead: 120000n, cacheWrite: 750000n },
    revision: "demo-catalog-v1",
    comparison: {
      officialRates: { input: 30000000n, output: 100000000n, cacheRead: 6000000n, cacheWrite: 37500000n },
      savingsPercent: { input: 98, output: 98, cacheRead: 98, cacheWrite: 98 },
    },
  },
  {
    id: "demo-gemini", name: "Gemini", publisher: "Google", protocols: ["Chat Completions"],
    description: "modelCatalog.exampleContext", contextTokens: 1000000, featured: true,
    rates: { input: 200000n, output: 800000n, cacheRead: 50000n, cacheWrite: null },
    revision: "demo-catalog-v1",
    comparison: {
      officialRates: { input: 500000n, output: 2000000n, cacheRead: 125000n },
      savingsPercent: { input: 60, output: 60, cacheRead: 60 },
    },
  },
];
