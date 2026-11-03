import type { HostedSnapshot } from "../features/app/data";
import { previewRequests } from "./fixtures";
import { previewOwnerAccount } from "./ownerAccounts";
import { previewModels } from "./models";

export const previewSnapshot: HostedSnapshot = {
  asOf: "2026-10-20T14:32:00Z",
  requests: previewRequests,
  models: previewModels,
  modelPricingIllustrative: true,
  balance: 42800000n,
  reserved: 180000n,
  owner: {
    // Synthetic active-request evidence, not a live request or a routing simulation.
    poolActivity: { members: [{ id: "demo_account_1", kind: "account", activeRequests: 1, models: [{ id: "demo-gpt", requests: 1 }] }], next: { id: "demo_account_2", kind: "account" } },
    connections: [
      { id: "demo_account_1", name: "Code account", family: "common.account", kind: "account", models: ["demo-gpt"], inPool: true, paused: false, trashed: false, account: previewOwnerAccount(Date.parse("2026-10-20T14:32:00Z"), "normal") },
      { id: "demo_account_2", name: "Text account", family: "common.account", kind: "account", models: ["demo-gpt"], inPool: true, paused: false, trashed: false, account: previewOwnerAccount(Date.parse("2026-10-20T14:32:00Z"), "low") },
      { id: "demo_provider_1", name: "Primary API", family: "connections.apiProvider", kind: "api", models: ["demo-gpt", "demo-claude", "demo-gemini"], inPool: true, paused: false, trashed: false, source: {
        baseUrl: "https://api.example.com/v1", wireApi: "responses",
        resolvedProtocolBindings: [{ wireApi: "responses", modelIds: ["demo-gpt"] }, { wireApi: "messages", modelIds: ["demo-claude"] }, { wireApi: "chat_completions", modelIds: ["demo-gemini"] }],
        // Synthetic provider observations, unrelated to customer API charges.
        providerStats: { provider: "billing", status: "available", balanceMicroUsd: 125800000, spentMicroUsd: 15400000, requests: null, totalTokens: null, asOfMs: Date.parse("2026-10-20T14:32:00Z") },
      } },
      { id: "demo_provider_2", name: "Backup API", family: "connections.apiProvider", kind: "api", models: ["demo-gemini"], inPool: false, paused: false, trashed: false, source: { baseUrl: "https://backup.example.com/v1", wireApi: "chat_completions", resolvedProtocolBindings: [], providerStats: null } },
    ],
    customers: [{ id: "demo_customer_1", name: "Personal project", status: "status.active", keys: 2 }, { id: "demo_customer_2", name: "Test bot", status: "status.active", keys: 1 }],
    events: [{ id: "demo_event_1", at: "2026-10-20T14:00:00Z", action: "Configuration created", target: "Primary pool" }],
  },
  keys: [...new Map(previewRequests.map(record => [record.key.id, {
    id: record.key.id, name: record.key.name, mask: record.key.mask,
    models: [...new Set(previewRequests.filter(request => request.key.id === record.key.id).map(request => request.model.id))],
    dailyLimit: null, expires: null, revoked: false,
  }])).values()],
  paymentMethods: [
    { id: "card", label: "billing.bankCard" },
    { id: "crypto", label: "billing.cryptocurrency" },
    { id: "bank_transfer", label: "billing.bankTransfer" },
  ],
  payments: [
    { id: "demo_payment_0003", at: "2026-10-20T14:05:00Z", kind: "billing.topUpOperation", amount: 25000000n, status: "billing.credited" },
    { id: "demo_payment_0002", at: "2026-10-19T11:40:00Z", kind: "billing.paymentRefund", amount: -5000000n, status: "billing.refunded" },
    { id: "demo_payment_0001", at: "2026-10-17T10:05:00Z", kind: "billing.topUpOperation", amount: 20000000n, status: "billing.credited" },
  ],
};
