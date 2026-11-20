import type { CustomerModel } from "../features/models/model";

// Synthetic presentation fixtures, not current provider prices or real offers.
// Keep today's catalog separate from historical request billing snapshots.
// Comparisons intentionally exercise different server-supplied percentages.
const gptStandardShort = {
  id: "standard-short",
  tier: "standard" as const,
  contextBand: "base" as const,
  rates: { input: "0.4", output: "1.2", cacheRead: "0.1", cacheWrite: "0.5" },
  comparison: {
    officialRates: { input: "2", output: "6", cacheRead: "0.5", cacheWrite: "2.5" },
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
        rates: { input: "0.8", output: "2.4", cacheRead: "0.2", cacheWrite: "1" },
        comparison: {
          officialRates: { input: "4", output: "12", cacheRead: "1", cacheWrite: "5" },
          savingsPercent: { input: 80, output: 80, cacheRead: 80, cacheWrite: 80 },
        },
      },
      {
        id: "flex-short", tier: "flex", contextBand: "base",
        rates: { input: "0.2", output: "0.6", cacheRead: "0.05", cacheWrite: null },
      },
      {
        id: "priority-short", tier: "priority", contextBand: "base",
        rates: { input: "0.6", output: "1.8", cacheRead: "0.15", cacheWrite: "0.75" },
      },
      {
        id: "ultrafast-short", tier: "ultrafast", contextBand: "base",
        rates: { input: "1", output: "3", cacheRead: "0.25" },
      },
    ],
  },
  {
    id: "demo-claude", name: "Claude", publisher: "Anthropic", protocols: ["Messages"],
    description: "modelCatalog.exampleAnalysis", contextTokens: 200000, featured: true,
    rates: { input: "0.6", output: "2", cacheRead: "0.12", cacheWrite: "0.75" },
    revision: "demo-catalog-v1",
    comparison: {
      officialRates: { input: "30", output: "100", cacheRead: "6", cacheWrite: "37.5" },
      savingsPercent: { input: 98, output: 98, cacheRead: 98, cacheWrite: 98 },
    },
  },
  {
    id: "demo-gemini", name: "Gemini", publisher: "Google", protocols: ["Chat Completions"],
    description: "modelCatalog.exampleContext", contextTokens: 1000000, featured: true,
    rates: { input: "0.2", output: "0.8", cacheRead: "0.05", cacheWrite: null },
    revision: "demo-catalog-v1",
    comparison: {
      officialRates: { input: "0.5", output: "2", cacheRead: "0.125" },
      savingsPercent: { input: 60, output: 60, cacheRead: 60 },
    },
  },
];
