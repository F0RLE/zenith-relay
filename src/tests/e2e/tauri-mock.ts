import type { Page } from "@playwright/test";

export type MockOptions = {
  locale?: "en" | "ru";
  onboarding?: boolean;
  mode?: "local" | "remote" | "zenith";
  theme?: "system" | "light" | "dark";
  compact?: boolean;
  populated?: boolean;
  accountCount?: number;
  accountAuthReason?: "invalid_grant" | "reused_refresh_token" | "expired_refresh_token" | "invalidated_refresh_token";
  codexBindings?: boolean;
  codexBoundOauthAccountId?: string | null;
  profileRepairRecommended?: boolean;
  profileSwitchError?: boolean;
  profileSnapshotsEmpty?: boolean;
  historyRepairChanges?: boolean;
  historyRepairError?: boolean;
  supplementalQuota?: boolean;
  subscriptionExpiresInMs?: number;
  exhaustedQuotaWindow?: "primary" | "secondary";
  freeAccountHealthy?: boolean;
  gatewayRunning?: boolean;
  poolKeyPresent?: boolean;
  poolMembers?: boolean;
  importResult?: "success" | "item_failure" | "not_found";
  importFailureCode?: string;
  remoteConnected?: boolean;
  remoteFeatures?: string[];
  legacyRemoteRouting?: boolean;
  oauthCallbackBeforeStartReturns?: boolean;
  updateVersion?: string;
  updateBody?: string;
  updateDate?: string;
};

export async function emitTauriEvent(page: Page, event: string, payload: unknown) {
  await page.evaluate(([name, value]) => {
    (window as unknown as { __TAURI_TEST_EMIT__: (event: string, payload: unknown) => void }).__TAURI_TEST_EMIT__(name, value);
  }, [event, payload] as const);
}

export async function installTauriMock(page: Page, options: MockOptions = {}) {
  await page.addInitScript((input) => {
    const locale = input.locale ?? "en";
    const populated = input.populated ?? true;
    const dayMs = 24 * 60 * 60_000;
    localStorage.setItem("relay.onboarding", input.onboarding === false ? "0" : "1");
    localStorage.setItem("relay.mode", input.mode ?? "local");
    localStorage.setItem("relay.theme", input.theme ?? "light");
    localStorage.setItem("relay.compact", input.compact ? "1" : "0");

    type MockQuotaWindow = { kind: "primary" | "secondary"; availableBasisPoints: number; explicitlyFull: boolean; resetAtMs: number; windowMinutes: number; observedAtMs: number };
    const exhaustedQuotaWindow = input.exhaustedQuotaWindow ?? "primary";
    const quota: { primary: MockQuotaWindow | null; secondary: MockQuotaWindow | null; supplemental: Array<{ id: string; label: string; window: MockQuotaWindow }>; resetCreditsAvailable: number; updatedAtMs: number; error: null } = {
      primary: { kind: "primary", availableBasisPoints: exhaustedQuotaWindow === "primary" ? 0 : 7200, explicitlyFull: false, resetAtMs: Date.now() + 90 * 60_000, windowMinutes: 300, observedAtMs: Date.now() },
      secondary: { kind: "secondary", availableBasisPoints: exhaustedQuotaWindow === "secondary" ? 0 : 6400, explicitlyFull: false, resetAtMs: Date.now() + 3 * 24 * 60 * 60_000, windowMinutes: 10_080, observedAtMs: Date.now() },
      supplemental: input.supplementalQuota ? [
        { id: "code_review:primary", label: "Code Review", window: { kind: "primary", availableBasisPoints: 7200, explicitlyFull: false, resetAtMs: Date.now() + 2 * 60 * 60_000, windowMinutes: 300, observedAtMs: Date.now() } },
        { id: "code_review:secondary", label: "Code Review", window: { kind: "secondary", availableBasisPoints: 8600, explicitlyFull: false, resetAtMs: Date.now() + 5 * 24 * 60 * 60_000, windowMinutes: 10_080, observedAtMs: Date.now() } },
        { id: "additional:0:primary", label: "GPT-5.4 priority", window: { kind: "primary", availableBasisPoints: 4100, explicitlyFull: false, resetAtMs: Date.now() + 12 * 60 * 60_000, windowMinutes: 1_440, observedAtMs: Date.now() } },
      ] : [],
      resetCreditsAvailable: 1,
      updatedAtMs: Date.now(),
      error: null as { code: string; observedAtMs: number } | null,
    };
    const source = {
      id: "source_synthetic",
      name: "Example compatible API",
      enabled: true,
      inPool: input.poolMembers ?? true,
      draining: false,
      baseUrl: "https://example.invalid/v1",
      wireApi: "responses",
      models: ["gpt-5.4", "gpt-5.4-mini"],
      allowedModels: [],
      excludedModels: [],
      priority: 10,
      weight: 100,
      apiEquivalent: { microUsd: 8_500, pricedTokens: 1_400, unpricedTokens: 0 },
      secretAvailable: true,
      lastErrorCode: null,
    };
    const account = {
      id: "account_synthetic",
      label: "Personal Plus",
      identityHint: "p***@example.test",
      enabled: true,
      inPool: input.poolMembers ?? true,
      draining: false,
      authState: input.accountAuthReason ? { state: "requires_reauth", reason: input.accountAuthReason } : "active",
      health: "healthy" as string,
      models: ["gpt-5.4", "gpt-5.4-mini"],
      allowedModels: [],
      excludedModels: [],
      priority: 20,
      weight: 100,
      apiEquivalent: { microUsd: 170, pricedTokens: 28, unpricedTokens: 0 },
      subscription: { planType: input.supplementalQuota ? "pro" : "plus", activeUntilMs: Date.now() + (input.subscriptionExpiresInMs ?? 37 * dayMs), status: "active", updatedAtMs: Date.now() },
      quota,
      secretAvailable: true,
      proxyMode: "common",
      proxyAvailable: true,
      routingExclusion: null as "free_plan_policy" | null,
      lastErrorCode: input.accountAuthReason ? "quota_token_prepare" : null as string | null,
    };
    const accountCount = Math.max(1, Math.min(input.accountCount ?? 1, 6));
    const accountVariants = [
      { label: "Personal Plus", plan: "plus", activeUntilMs: Date.now() + 37 * dayMs, proxyMode: "common", models: ["gpt-5.4", "gpt-5.4-mini"], primary: 0, primaryMinutes: 300, secondary: 6400, priority: 20, health: "healthy", error: null },
      { label: "Business Workspace", plan: "team", activeUntilMs: Date.now() + 203 * dayMs, proxyMode: "account", models: ["gpt-5.4", "gpt-5.4-mini", "o3"], primary: 3800, primaryMinutes: 50_400, secondary: null, priority: 30, health: "healthy", error: null },
      { label: "Backup account", plan: "free", activeUntilMs: null, proxyMode: "direct", models: ["gpt-5.4-mini"], primary: 9500, primaryMinutes: 43_200, secondary: null, priority: 10, health: "degraded", error: "quota_transport" },
      { label: "Pro account", plan: "pro", activeUntilMs: Date.now() + 172 * dayMs, proxyMode: "common", models: ["gpt-5.4", "gpt-5.4-mini", "o3"], primary: 7600, primaryMinutes: 300, secondary: 8200, priority: 25, health: "healthy", error: null },
    ] as const;
    const accounts = Array.from({ length: accountCount }, (_, index) => {
      if (index === 0) return account;
      const variant = accountVariants[index % accountVariants.length];
      const item = structuredClone(account);
      item.id = `account_synthetic_${index + 1}`;
      item.label = variant.label;
      item.identityHint = ["p***@example.test", "b***@example.test", "r***@example.test", "q***@example.test", "s***@example.test", "t***@example.test"][index % 6];
      item.authState = "active";
      item.subscription.planType = variant.plan;
      item.subscription.activeUntilMs = variant.activeUntilMs;
      item.proxyMode = variant.proxyMode;
      item.models = [...variant.models];
      item.priority = variant.priority;
      item.apiEquivalent.microUsd = 170 * (index + 1);
      item.apiEquivalent.pricedTokens = 28 * (index + 1);
      const healthyFree = variant.plan === "free" && input.freeAccountHealthy;
      item.health = healthyFree ? "healthy" : variant.health;
      item.routingExclusion = variant.plan === "free" ? "free_plan_policy" : null;
      item.lastErrorCode = healthyFree ? null : variant.error;
      item.quota.error = healthyFree ? null : variant.error ? { code: variant.error, observedAtMs: Date.now() } : null;
      if (item.quota.primary) {
        item.quota.primary.availableBasisPoints = variant.primary;
        item.quota.primary.windowMinutes = variant.primaryMinutes;
      }
      if (variant.secondary === null) item.quota.secondary = null;
      else if (item.quota.secondary) item.quota.secondary.availableBasisPoints = variant.secondary;
      return item;
    });
    const key = {
      id: "key_synthetic",
      label: "ChatGPT",
      enabled: true,
      sourceIds: null,
      accountIds: null,
      allowedModels: [],
      excludedModels: [],
      modelPrefix: null,
      createdAtMs: Date.now() - 86_400_000,
      lastUsedAtMs: Date.now() - 60_000,
    };
    let profileSnapshots = input.profileSnapshotsEmpty ? [] : [{
      id: "11111111-1111-4111-8111-111111111111",
      name: locale === "ru" ? "Исходный профиль" : "Original profile",
      profileDir: "C:\\Users\\Test\\.codex",
      createdAtMs: Date.now() - 3_600_000,
      configAvailable: true,
      authAvailable: true,
    }];
    type MockModelSummary = { id: string; enabled: boolean; memberCount: number; catalogRank: number | null; inputMicroUsdPerMillion: number | null; outputMicroUsdPerMillion: number | null };
    const modelPrices: Record<string, Pick<MockModelSummary, "catalogRank" | "inputMicroUsdPerMillion" | "outputMicroUsdPerMillion">> = {
      "gpt-5.4": { catalogRank: 5, inputMicroUsdPerMillion: 2_500_000, outputMicroUsdPerMillion: 15_000_000 },
      "gpt-5.4-mini": { catalogRank: 6, inputMicroUsdPerMillion: 750_000, outputMicroUsdPerMillion: 4_500_000 },
    };
    const automation = {
      id: "wake_synthetic",
      name: "Start quota countdown",
      enabled: true,
      accountSelector: { kind: "all_eligible" },
      windowKinds: ["primary", "secondary"],
      modelPolicy: { kind: "lightest_supported" },
      trigger: { kind: "quota_full" },
      executionPolicy: "automatic",
      jitterSeconds: 0,
      maxAttemptsPerCycle: 1,
      createdAtMs: Date.now() - 86_400_000,
      updatedAtMs: Date.now() - 60_000,
    };
    const localRuntime = {
      schemaVersion: 12,
      runtimeTarget: { kind: "local", connected: true, origin: "http://127.0.0.1:14998", serverId: null, version: "1.0.5" },
      gateway: { running: input.gatewayRunning ?? true, baseUrl: "http://127.0.0.1:14998/v1", candidateCount: 0, visibleModelIds: [] as string[], maxRetryCandidates: 3, routingStrategy: "adaptive" as "adaptive" | "oldest_account", defaultServiceTier: "standard" as "standard" | "fast", sessionAffinity: false, sessionAffinityTtlSeconds: 3_600, models: [] as MockModelSummary[], commonProxyConfigured: true, commonProxyAvailable: true, accountProxyRequired: false, quotaRefreshIntervalSeconds: 300, quotaRequestTimeoutSeconds: 20, useFreeAccounts: false },
      platform: "windows",
      capabilities: { features: ["sources", "oauth_accounts", "quota_wake", "profiles", "account_proxies", "account_export", "account_identity_reveal", "free_account_policy"] },
      sources: populated ? [source] : [],
      accounts: populated ? accounts : [],
      keys: populated && input.poolKeyPresent !== false ? [key] : [],
      automations: populated ? [automation] : [],
      wakeHistory: populated ? [{ taskId: automation.id, accountId: account.id, windowKind: "primary", modelId: "gpt-5.4-mini", outcome: "confirmed", startedAtMs: Date.now() - 120_000, completedAtMs: Date.now() - 118_000, errorCode: null }] : [],
      warnings: [],
    };
    refreshGatewayModels(localRuntime);
    const remoteRuntime = structuredClone(localRuntime);
    remoteRuntime.schemaVersion = 14;
    remoteRuntime.runtimeTarget = { kind: "remote", connected: true, origin: "https://relay.example.invalid", serverId: "server_synthetic", version: "1.0.5" };
    remoteRuntime.gateway.baseUrl = "https://relay.example.invalid/v1";
    if (input.legacyRemoteRouting) {
      delete (remoteRuntime.gateway as { routingStrategy?: "adaptive" | "oldest_account" }).routingStrategy;
      delete (remoteRuntime.gateway as { defaultServiceTier?: "standard" | "fast" }).defaultServiceTier;
    }
    remoteRuntime.platform = "linux";
    remoteRuntime.capabilities = { features: input.remoteFeatures ?? ["sources", "accounts", "account_batch_import", "account_import_to_pool", "account_export", "account_identity_reveal", "quota", "models", "usage", "local_gateway", "keys", "diagnostics", "wake_tasks", "account_proxies", "free_account_policy"] };

    function sourceFromPayload(payload: Record<string, unknown>, id: string) {
      return {
        ...structuredClone(source),
        id,
        name: String(payload.name ?? source.name),
        baseUrl: String(payload.baseUrl ?? source.baseUrl),
        wireApi: String(payload.wireApi ?? source.wireApi),
        models: payload.models as string[] ?? [],
        allowedModels: payload.allowedModels as string[] ?? [],
        excludedModels: payload.excludedModels as string[] ?? [],
        priority: Number(payload.priority ?? 0),
        weight: Number(payload.weight ?? 100),
        draining: Boolean(payload.draining),
        inPool: false,
      };
    }

    const routing = { reason: "quota_headroom", eligibleCandidates: 4, quotaRemainingBasisPoints: 6300, effectiveWeight: 6300, inFlightBefore: 0, dispatchesBefore: 3 };
    let localUsage = populated ? [{ id: 1, createdAt: new Date().toISOString(), requestId: "req_synthetic_local", attempt: 1, localKeyId: key.id, sourceId: source.id, accountId: account.id, requestedModel: "gpt-5.4", resolvedModel: "gpt-5.4", wireApi: "responses", success: true, httpStatus: 200, errorCategory: null, latencyMs: 428, ttftMs: 128, inputTokens: 20, cachedInputTokens: 12, reasoningTokens: 5, outputTokens: 8, totalTokens: 28, routing }] : [];
    let remoteUsage = populated ? [{ id: 2, requestId: "req_synthetic_remote", localKeyId: key.id, candidateKind: "account", candidateHint: "a1b2c3d4e5f6", candidateLabel: account.label, requestedModel: "gpt-5.4", resolvedModel: "gpt-5.4", wireApi: "responses", success: true, httpStatus: 200, errorCategory: null, latencyMs: 512, ttftMs: 184, inputTokens: 18, cachedInputTokens: 10, reasoningTokens: 3, outputTokens: 7, totalTokens: 25, createdAtMs: Date.now(), routing }] : [];
    function usageTotals(events: Array<{ success: boolean; latencyMs: number; ttftMs?: number | null; inputTokens: number | null; cachedInputTokens: number | null; reasoningTokens: number | null; outputTokens: number | null; totalTokens: number | null }>) {
      return events.reduce((totals, item) => {
        const visible = Math.max(0, (item.outputTokens ?? 0) - Math.min(item.reasoningTokens ?? 0, item.outputTokens ?? 0));
        const duration = item.ttftMs != null && item.latencyMs > item.ttftMs ? item.latencyMs - item.ttftMs : item.latencyMs;
        totals.requests += 1; totals.successfulRequests += Number(item.success); totals.latencyMs += item.latencyMs;
        if (item.ttftMs != null) { totals.ttftMs += item.ttftMs; totals.ttftSamples += 1; }
        totals.inputTokens += item.inputTokens ?? 0; totals.cachedInputTokens += item.cachedInputTokens ?? 0;
        totals.reasoningTokens += item.reasoningTokens ?? 0; totals.outputTokens += item.outputTokens ?? 0; totals.totalTokens += item.totalTokens ?? 0;
        if (item.success && visible && duration) { totals.speedOutputTokens += visible; totals.speedDurationMs += duration; }
        totals.apiEquivalent.microUsd += 148; totals.apiEquivalent.pricedTokens += item.totalTokens ?? 0;
        return totals;
      }, { requests: 0, successfulRequests: 0, latencyMs: 0, ttftMs: 0, ttftSamples: 0, inputTokens: 0, cachedInputTokens: 0, reasoningTokens: 0, outputTokens: 0, totalTokens: 0, speedOutputTokens: 0, speedDurationMs: 0, apiEquivalent: { microUsd: 0, pricedTokens: 0, unpricedTokens: 0 } });
    }
    let readyKey = "zrk_synthetic_ready_key";
    const invocations: Array<{ command: string; args: Record<string, unknown> }> = [];
    const callbacks = new Map<number, (...args: unknown[]) => unknown>();
    let nextCallback = 1;
    const eventListeners = new Map<number, { event: string; handler: number }>();
    let nextEventListener = 1;

    const tauri = {
      metadata: {
        currentWindow: { label: "main" },
        currentWebview: { windowLabel: "main", label: "main" },
      },
      transformCallback(callback: (...args: unknown[]) => unknown, once = false) {
        const id = nextCallback++;
        callbacks.set(id, (...args: unknown[]) => { const result = callback(...args); if (once) callbacks.delete(id); return result; });
        return id;
      },
      unregisterCallback(id: number) { callbacks.delete(id); },
      convertFileSrc(path: string) { return path; },
      async invoke(command: string, args: Record<string, unknown> = {}) {
        const recordedArgs = command === "plugin:updater|download_and_install"
          ? JSON.parse(JSON.stringify(args)) as Record<string, unknown>
          : structuredClone(args);
        invocations.push({ command, args: recordedArgs });
        switch (command) {
          case "get_system_locale": return locale;
          case "get_platform": return "windows";
          case "get_state": return { providerActive: Boolean(readyKey), codexRunning: false, hasSavedApiKey: Boolean(readyKey) };
          case "get_key_stats": return { balance: 42.5, spent: 7.5, requests: 18, totalTokens: 2500, inputTokens: 1700, cachedTokens: 300, reasoningTokens: 100, outputTokens: 400 };
          case "get_saved_key_stats": return { balance: 42.5, spent: 7.5, requests: 18, totalTokens: 2500, inputTokens: 1700, cachedTokens: 300, reasoningTokens: 100, outputTokens: 400 };
          case "get_saved_key_models": return ["gpt-5.4", "gpt-5.4-mini"];
          case "get_key_usage_history": return { usage: populated ? [{ id: 3, createdAt: new Date().toISOString(), status: "success", model: "gpt-5.4", modelDisplay: "gpt-5.4", streamDurationMs: 390, timeToFirstByteMs: 120, inputTokens: 20, cachedInputTokens: 10, reasoningTokens: 4, outputTokens: 10, totalTokens: 30, requestId: "req_synthetic_ready", responseTimeDisplay: "390 ms" }] : [], limit: 100, sinceId: null };
          case "get_saved_key_usage_history": return { usage: populated ? [{ id: 3, createdAt: new Date().toISOString(), status: "success", model: "gpt-5.4", modelDisplay: "gpt-5.4", streamDurationMs: 390, timeToFirstByteMs: 120, inputTokens: 20, cachedInputTokens: 10, reasoningTokens: 4, outputTokens: 10, totalTokens: 30, requestId: "req_synthetic_ready", responseTimeDisplay: "390 ms" }] : [], limit: 100, sinceId: null };
          case "create_saved_top_up_intent_and_open": return null;
          case "save_key": readyKey = String(args.apiKey ?? ""); return readyKey;
          case "reset_key": readyKey = ""; return "reset";
          case "prepare_top_up_amount": return { amountCents: 1000, amountUsd: 10, valid: true };
          case "get_local_runtime_state": return structuredClone(localRuntime);
          case "get_remote_server_state": return input.remoteConnected === false ? null : structuredClone(remoteRuntime);
          case "get_local_usage": return structuredClone(localUsage);
          case "get_local_usage_page": {
            const query = (args.input ?? {}) as { page?: number; pageSize?: number; success?: boolean; modelQuery?: string; sourceOrAccountQuery?: string; localKeyQuery?: string; wireApi?: string; errorCategory?: string; requestIdQuery?: string };
            const events = localUsage.filter((item) => (query.success === undefined || item.success === query.success) && (!query.modelQuery || item.resolvedModel.includes(query.modelQuery)) && (!query.sourceOrAccountQuery || item.accountId.includes(query.sourceOrAccountQuery) || item.sourceId.includes(query.sourceOrAccountQuery)) && (!query.localKeyQuery || item.localKeyId.includes(query.localKeyQuery)) && (!query.wireApi || item.wireApi === query.wireApi) && (!query.errorCategory || item.errorCategory === query.errorCategory) && (!query.requestIdQuery || item.requestId.includes(query.requestIdQuery)));
            const totals = usageTotals(events);
            return { events: structuredClone(events), total: events.length, page: query.page ?? 1, pageSize: query.pageSize ?? 50, totalPages: events.length ? 1 : 0, totals, models: events.length ? [{ key: "gpt-5.4", totals }] : [], poolMembers: events.length ? [{ key: account.id, label: account.label, totals }] : [] };
          }
          case "get_remote_server_usage": {
            const query = (args.input ?? {}) as { page?: number; pageSize?: number; success?: boolean; modelQuery?: string; sourceOrAccountQuery?: string; localKeyQuery?: string; wireApi?: string; errorCategory?: string; requestIdQuery?: string };
            const events = remoteUsage.filter((item) => (query.success === undefined || item.success === query.success) && (!query.modelQuery || item.resolvedModel.includes(query.modelQuery)) && (!query.sourceOrAccountQuery || item.candidateHint.includes(query.sourceOrAccountQuery)) && (!query.localKeyQuery || item.localKeyId.includes(query.localKeyQuery)) && (!query.wireApi || item.wireApi === query.wireApi) && (!query.errorCategory || item.errorCategory === query.errorCategory) && (!query.requestIdQuery || item.requestId.includes(query.requestIdQuery)));
            const totals = usageTotals(events);
            return { events: structuredClone(events), total: events.length, page: query.page ?? 1, pageSize: query.pageSize ?? 50, totalPages: events.length ? 1 : 0, totals, models: events.length ? [{ key: "gpt-5.4", totals }] : [], poolMembers: events.length ? [{ key: "a1b2c3d4e5f6", label: account.label, totals }] : [] };
          }
          case "create_local_source": {
            const created = sourceFromPayload(args.input as Record<string, unknown>, `source_created_${localRuntime.sources.length + 1}`);
            localRuntime.sources = [...localRuntime.sources, created];
            return structuredClone(created);
          }
          case "update_local_source": {
            const request = args.input as Record<string, unknown> & { sourceId?: string };
            const target = localRuntime.sources.find((item) => item.id === request.sourceId);
            if (target) Object.assign(target, request);
            refreshGatewayModels(localRuntime);
            return structuredClone(localRuntime);
          }
          case "rotate_local_source_key": return structuredClone(localRuntime);
          case "set_local_source_enabled": source.enabled = Boolean(args.enabled); return structuredClone(localRuntime);
          case "delete_local_source": localRuntime.sources = []; return structuredClone(localRuntime);
          case "test_local_source": return structuredClone(source);
          case "start_local_account_import": return importSession("11111111-2222-4333-8444-555555555555");
          case "preview_local_account_import_files": return importSession("11111111-2222-4333-8444-555555555555");
          case "preview_remote_account_import_files": return importSession("remote_import");
          case "resume_local_account_import": return importSession(String(args.sessionId ?? "11111111-2222-4333-8444-555555555555"));
          case "prepare_local_account_import": return importSession(String((args.input as { sessionId?: string })?.sessionId ?? "11111111-2222-4333-8444-555555555555"));
          case "confirm_local_account_import": {
            if (input.importResult === "not_found") throw { code: "not_found" };
            const request = args.input as { sessionId?: string; selectedItemIds?: string[] };
            const itemIds = request.selectedItemIds ?? [];
            return importConfirmation(request.sessionId ?? "11111111-2222-4333-8444-555555555555", itemIds);
          }
          case "cancel_local_account_import": return null;
          case "refresh_local_account_quota": return structuredClone(localRuntime);
          case "refresh_all_local_account_quotas": return structuredClone(localRuntime);
          case "refresh_local_pool_account_quotas": return [];
          case "update_local_account": {
            const request = args.input as { accountId?: string; priority?: number; weight?: number; draining?: boolean };
            const target = localRuntime.accounts.find((item) => item.id === request.accountId);
            if (target && typeof request.priority === "number") target.priority = request.priority;
            if (target && typeof request.weight === "number") target.weight = request.weight;
            if (target && typeof request.draining === "boolean") target.draining = request.draining;
            return structuredClone(localRuntime);
          }
          case "set_local_account_enabled": {
            const target = localRuntime.accounts.find((item) => item.id === args.accountId);
            if (target) target.enabled = Boolean(args.enabled);
            return structuredClone(localRuntime);
          }
          case "set_local_account_draining": {
            const target = localRuntime.accounts.find((item) => item.id === args.accountId);
            if (target) target.draining = Boolean(args.draining);
            return structuredClone(localRuntime);
          }
          case "set_local_pool_membership": {
            const request = args.input as { accountIds: string[]; sourceIds: string[]; inPool: boolean };
            for (const item of localRuntime.accounts) if (request.accountIds.includes(item.id)) item.inPool = request.inPool;
            for (const item of localRuntime.sources) if (request.sourceIds.includes(item.id)) item.inPool = request.inPool;
            localRuntime.gateway.candidateCount = [...localRuntime.accounts, ...localRuntime.sources].filter((item) => item.enabled && item.inPool && !item.draining).length;
            refreshGatewayModels(localRuntime);
            return structuredClone(localRuntime);
          }
          case "set_local_model_enabled": {
            const request = args.input as { modelId: string; enabled: boolean };
            const target = localRuntime.gateway.models.find((model) => model.id === request.modelId);
            if (target) target.enabled = request.enabled;
            localRuntime.gateway.visibleModelIds = localRuntime.gateway.models.filter((model) => model.enabled).map((model) => model.id);
            return structuredClone(localRuntime);
          }
          case "update_local_quota_policy": {
            const request = args.input as { refreshIntervalSeconds: number; requestTimeoutSeconds: number; useFreeAccounts: boolean };
            localRuntime.gateway.quotaRefreshIntervalSeconds = request.refreshIntervalSeconds;
            localRuntime.gateway.quotaRequestTimeoutSeconds = request.requestTimeoutSeconds;
            localRuntime.gateway.useFreeAccounts = request.useFreeAccounts;
            applyFreeRoutingPolicy(localRuntime);
            return structuredClone(localRuntime);
          }
          case "update_local_routing": {
            const request = args.input as { maxRetryCandidates: number; routingStrategy: "adaptive" | "oldest_account"; defaultServiceTier: "standard" | "fast"; sessionAffinity: boolean; sessionAffinityTtlSeconds: number };
            localRuntime.gateway.maxRetryCandidates = request.maxRetryCandidates;
            localRuntime.gateway.routingStrategy = request.routingStrategy;
            localRuntime.gateway.defaultServiceTier = request.defaultServiceTier;
            localRuntime.gateway.sessionAffinity = request.sessionAffinity;
            localRuntime.gateway.sessionAffinityTtlSeconds = request.sessionAffinityTtlSeconds;
            return structuredClone(localRuntime);
          }
          case "sync_codex_default_service_tier": return null;
          case "delete_local_account": {
            localRuntime.accounts = localRuntime.accounts.filter((item) => item.id !== args.accountId);
            refreshGatewayModels(localRuntime);
            return structuredClone(localRuntime);
          }
          case "set_local_account_proxy": {
            const request = args.input as { accountId: string; proxyUrl: string | null };
            account.proxyMode = request.proxyUrl ? "account" : localRuntime.gateway.commonProxyConfigured ? "common" : "direct";
            account.proxyAvailable = true;
            return structuredClone(localRuntime);
          }
          case "assign_local_account_proxies": {
            const request = args.input as { accountIds: string[]; proxyUrls: string[] };
            account.proxyMode = "account";
            account.proxyAvailable = true;
            return { assigned: request.accountIds.length, unused: request.proxyUrls.length - request.accountIds.length };
          }
          case "export_local_accounts":
          case "export_remote_accounts": {
            const request = args.input as { accountIds: string[]; format: string; destination: "copy" | "download" };
            const result = {
              format: request.format,
              accountCount: request.accountIds.length,
              fileName: `${request.accountIds.length === 1 ? "account" : "accounts"}-${request.format}.json`,
            };
            return request.destination === "copy"
              ? { ...result, content: JSON.stringify({ access_token: "synthetic-export-token", account_ids: request.accountIds }) }
              : { ...result, path: `C:\\Temp\\${result.fileName}` };
          }
          case "reveal_local_account_identity":
          case "reveal_remote_account_identity": return { accountId: String(args.accountId), identity: "person@example.test" };
          case "start_codex_oauth": {
            const flow = { loginId: "oauth_synthetic", authorizationUrl: "https://auth.example.invalid/authorize", redirectUri: "http://localhost:1455/auth/callback", expiresAtMs: Date.now() + 600_000, status: "pending" };
            if (input.oauthCallbackBeforeStartReturns) {
              for (const [id, listener] of eventListeners) {
                if (listener.event === "relay-oauth-status") callbacks.get(listener.handler)?.({ event: listener.event, id, payload: { loginId: flow.loginId, status: "callback_received" } });
              }
            }
            return flow;
          }
          case "resume_codex_oauth": return { loginId: String(args.loginId ?? "oauth_synthetic"), authorizationUrl: "https://auth.example.invalid/authorize", redirectUri: "http://localhost:1455/auth/callback", expiresAtMs: Date.now() + 600_000, status: "pending" };
          case "get_codex_oauth_status": return { loginId: "oauth_synthetic", authorizationUrl: "https://auth.example.invalid/authorize", redirectUri: "http://localhost:1455/auth/callback", expiresAtMs: Date.now() + 600_000, status: "callback_received" };
          case "submit_codex_oauth_callback":
          case "cancel_codex_oauth": return null;
          case "complete_codex_oauth": return { account: { id: "account_synthetic" } };
          case "create_local_gateway_key": localRuntime.keys = [key]; return { key: structuredClone(key), secret: "zlr_synthetic_local_key" };
          case "update_local_gateway_key": {
            const request = args.input as Record<string, unknown> & { keyId?: string };
            const target = localRuntime.keys.find((item) => item.id === request.keyId);
            if (target) Object.assign(target, request);
            return structuredClone(localRuntime);
          }
          case "rotate_local_gateway_key": return { key: structuredClone(key), secret: "zlr_synthetic_rotated_key" };
          case "set_local_gateway_key_enabled": key.enabled = Boolean(args.enabled); return structuredClone(localRuntime);
          case "delete_local_gateway_key": localRuntime.keys = []; return structuredClone(localRuntime);
          case "start_local_gateway": localRuntime.gateway.running = true; return structuredClone(localRuntime);
          case "stop_local_gateway": localRuntime.gateway.running = false; return structuredClone(localRuntime);
          case "restart_local_gateway": return structuredClone(localRuntime);
          case "update_local_gateway_port": {
            const port = Number(args.port);
            localRuntime.gateway.baseUrl = `http://127.0.0.1:${port}/v1`;
            localRuntime.runtimeTarget.origin = `http://127.0.0.1:${port}`;
            return structuredClone(localRuntime);
          }
          case "set_local_common_proxy": {
            const request = args.input as { proxyUrl: string | null };
            localRuntime.gateway.commonProxyConfigured = Boolean(request.proxyUrl);
            localRuntime.gateway.commonProxyAvailable = Boolean(request.proxyUrl);
            for (const item of localRuntime.accounts) {
              if (item.proxyMode === "account") continue;
              item.proxyMode = request.proxyUrl ? "common" : "direct";
              item.proxyAvailable = Boolean(request.proxyUrl) || !localRuntime.gateway.accountProxyRequired;
            }
            return structuredClone(localRuntime);
          }
          case "set_local_account_proxy_required": {
            const request = args.input as { required: boolean };
            localRuntime.gateway.accountProxyRequired = request.required;
            for (const item of localRuntime.accounts) {
              if (item.proxyMode === "direct") item.proxyAvailable = !request.required;
            }
            return structuredClone(localRuntime);
          }
          case "diagnose_local_gateway": return { stream: Boolean(args.stream), model: "gpt-5.4-mini", latencyMs: 321, bytesReceived: 64 };
          case "diagnose_remote_gateway": return { stream: Boolean(args.stream), model: "gpt-5.4-mini", latencyMs: 345, bytesReceived: 72 };
          case "create_quota_wake_automation": Object.assign(automation, args.input); localRuntime.automations = [automation]; return structuredClone(localRuntime);
          case "update_quota_wake_automation": Object.assign(automation, args.input); return structuredClone(localRuntime);
          case "set_quota_wake_automation_enabled": automation.enabled = Boolean(args.enabled); return structuredClone(localRuntime);
          case "delete_quota_wake_automation": localRuntime.automations = []; return structuredClone(localRuntime);
          case "run_due_quota_wake_confirmations": return 1;
          case "test_quota_wake_automation": return { taskId: String(args.taskId), status: "ready", eligibleAccounts: 1 };
          case "launch_managed_codex_profile":
          case "launch_saved_codex":
          case "restore_codex_profile":
          case "restore_codex_account_profile": return null;
          case "list_codex_profile_snapshots": return structuredClone(profileSnapshots);
          case "create_codex_profile_snapshot": {
            const snapshot = { id: `22222222-2222-4222-8222-${String(profileSnapshots.length + 1).padStart(12, "0")}`, name: String(args.name), profileDir: "C:\\Users\\Test\\.codex", createdAtMs: Date.now(), configAvailable: true, authAvailable: true };
            profileSnapshots = [snapshot, ...profileSnapshots];
            return structuredClone(snapshot);
          }
          case "restore_codex_profile_snapshot": {
            const safety = { id: `33333333-3333-4333-8333-${String(profileSnapshots.length + 1).padStart(12, "0")}`, name: String(args.safetyName), profileDir: "C:\\Users\\Test\\.codex", createdAtMs: Date.now(), configAvailable: true, authAvailable: true };
            profileSnapshots = [safety, ...profileSnapshots];
            return structuredClone(safety);
          }
          case "delete_codex_profile_snapshot": profileSnapshots = profileSnapshots.filter((snapshot) => snapshot.id !== String(args.snapshotId)); return null;
          case "stop_managed_codex_profile": return true;
          case "attach_codex_to_local_gateway": if (input.profileSwitchError) throw { code: "profile_restore_blocked", message: "Synthetic profile conflict" }; return { binding: { profileDir: "C:\\Users\\Test\\.codex", credentialKind: "local_gateway", credentialId: String(args.keyId), boundOauthAccountId: args.boundOauthAccountId ? String(args.boundOauthAccountId) : null }, previousCredentialKind: input.profileRepairRecommended ? "oauth_account" : null, repairRecommended: input.profileRepairRecommended ?? false, stoppedRunningClient: true };
          case "attach_codex_to_account":
          case "launch_codex_account": return { binding: { profileDir: "C:\\Users\\Test\\.codex", credentialKind: "oauth_account", credentialId: String(args.accountId), boundOauthAccountId: null }, previousCredentialKind: input.profileRepairRecommended === false ? "oauth_account" : "local_gateway", repairRecommended: input.profileRepairRecommended ?? true, stoppedRunningClient: true };
          case "preview_codex_history_repair": { if (input.historyRepairError) throw { code: "recovery_required", message: "Synthetic history preview failure" }; const changes = input.historyRepairChanges ?? true; const request = args.input as { targetProvider: "openai" | "zenith_relay_local" }; return { sessionId: "repair_0123456789abcdef0123456789abcdef", targetProvider: request.targetProvider, profileCount: 1, rolloutFileCount: changes ? 2 : 0, rolloutRecordCount: changes ? 2 : 0, sqliteRowCount: changes ? 1 : 0, codexRunning: false, expiresAtMs: Date.now() + 60_000 }; }
          case "apply_codex_history_repair": return { backupId: "history_repair_0123456789abcdef0123456789abcdef", backupPath: "C:\\Temp\\history-repair-backup", rolloutRecordsChanged: 2, sqliteRowsChanged: 1 };
          case "rollback_codex_history_repair": return { backupId: String(args.backupId), filesRestored: 3 };
          case "get_relay_storage_info": return { rootPath: "C:\\Users\\Test\\AppData\\Local\\Zenith Relay", dataPath: "C:\\Users\\Test\\AppData\\Local\\Zenith Relay\\data", recoveryPath: "C:\\Users\\Test\\AppData\\Local\\Zenith Relay\\recovery", cachePath: "C:\\Users\\Test\\AppData\\Local\\Zenith Relay\\cache", logsPath: "C:\\Users\\Test\\AppData\\Local\\Zenith Relay\\logs", chatgptProfilePath: "C:\\Users\\Test\\.codex", legacyDataPath: null };
          case "open_relay_folder": return null;
          case "reset_local_pool_data": localRuntime.sources = []; localRuntime.accounts = []; localRuntime.keys = []; localRuntime.automations = []; localUsage = []; return null;
          case "clear_local_usage": localUsage = []; return null;
          case "export_usage": return "C:\\Temp\\usage.json";
          case "preview_support_bundle": return { bundle: { generatedAt: new Date().toISOString(), appVersion: "1.0.5", platform: "windows", mode: "local", schemaVersion: 10, gatewayRunning: true, sourceCount: 1, accountCount: 1, keyCount: 1, automationCount: 1, usageCount: localUsage.length, warningCount: 0 }, excluded: ["secrets", "prompts", "responses", "raw_identities", "raw_headers"] };
          case "export_support_bundle": return "C:\\Temp\\support.json";
          case "list_codex_account_bindings": return populated && input.codexBindings !== false ? [{ profileDir: "C:\\Users\\Test\\.codex", credentialKind: "local_gateway", credentialId: key.id, boundOauthAccountId: input.codexBoundOauthAccountId ?? null }] : [];
          case "connect_remote_server": return { target: { origin: remoteRuntime.runtimeTarget.origin, serverId: remoteRuntime.runtimeTarget.serverId, identityFingerprint: "synthetic-fingerprint", serverVersion: "1.0.5", protocolVersion: 1, allowInsecureHttp: false, connectedAtMs: Date.now() } };
          case "disconnect_remote_server": return null;
          case "refresh_remote_server_capabilities": return { target: remoteRuntime.runtimeTarget };
          case "prepare_remote_server_deployment": return { directory: "C:\\Temp\\zenith-relay-deploy", publicBaseUrl: "https://relay.example.invalid", managementToken: "synthetic-management-token-000000", vaultKey: "c3ludGhldGljLXZhdWx0LWtleS0wMDAwMDAwMDA=", composeCommand: "docker compose up -d" };
          case "execute_remote_server_action": return remoteAction(args);
          case "plugin:event|listen": {
            const eventId = nextEventListener++;
            eventListeners.set(eventId, { event: String(args.event), handler: Number(args.handler) });
            return eventId;
          }
          case "plugin:event|unlisten": eventListeners.delete(Number(args.eventId)); return null;
          case "plugin:updater|check": return input.updateVersion ? { rid: 901, currentVersion: "1.0.5", version: input.updateVersion, date: input.updateDate ?? "2026-07-15T12:00:00Z", body: input.updateBody ?? "Faster routing\nImproved settings", rawJson: {} } : null;
          case "plugin:updater|download_and_install":
          case "plugin:resources|close":
          case "plugin:process|restart":
          case "close_window":
          case "minimize_window":
          case "toggle_maximize_window": return null;
          default: throw new Error(`Unexpected Tauri command: ${command}`);
        }
      },
    };

    function importSession(sessionId: string) {
      return { sessionId, prepared: true, preview: { format: "portable", rows: [
        { itemId: "import_0123456789abcdef", label: "Imported account", identity: "im••••ed", authMode: "oauth", sourceName: "OpenAI", quotaStatus: "available", status: "ready", plan: "Plus", defaultSelected: true, selectable: true, existing: false, warnings: [] },
        { itemId: "import_1111222233334444", label: "Second imported account", identity: "se••••nd", authMode: "oauth", sourceName: "OpenAI", quotaStatus: "available", status: "ready", plan: "Plus", defaultSelected: true, selectable: true, existing: false, warnings: [] },
        { itemId: "import_fedcba9876543210", label: "Existing account", identity: "ex••••ng", authMode: "oauth", sourceName: "OpenAI", quotaStatus: "available", status: "existing", plan: "Plus", defaultSelected: false, selectable: true, existing: true, warnings: [] },
      ], warnings: [] } };
    }

    function importConfirmation(sessionId: string, itemIds: string[]) {
      return {
        sessionId,
        results: itemIds.map((itemId, index) => input.importResult === "item_failure" && index === 0
          ? { itemId, status: "failed", error: { code: input.importFailureCode ?? "provider_account_id_missing", message: "secret=synthetic-access-token provider=raw-provider-id" } }
          : { itemId, status: "succeeded" }),
      };
    }

    function refreshGatewayModels(runtime: typeof localRuntime) {
      const enabled = new Map(runtime.gateway.models.map((model) => [model.id.toLowerCase(), model.enabled]));
      const members: Array<{ enabled: boolean; inPool: boolean; draining: boolean; secretAvailable: boolean; models: string[]; proxyAvailable?: boolean; routingExclusion?: string | null }> = [...runtime.sources, ...runtime.accounts];
      const eligible = members.filter((member) => member.enabled && member.inPool && !member.draining && member.secretAvailable && member.proxyAvailable !== false && member.routingExclusion == null);
      runtime.gateway.candidateCount = eligible.length;
      const ids = [...new Map(eligible.flatMap((member) => member.models).map((id) => [id.toLowerCase(), id])).values()];
      runtime.gateway.models = ids.map((id) => ({
        id,
        enabled: enabled.get(id.toLowerCase()) ?? true,
        memberCount: eligible.filter((member) => member.models.some((model) => model.toLowerCase() === id.toLowerCase())).length,
        ...(modelPrices[id.toLowerCase()] ?? { catalogRank: null, inputMicroUsdPerMillion: null, outputMicroUsdPerMillion: null }),
      })).sort((left, right) => (left.catalogRank ?? Number.MAX_SAFE_INTEGER) - (right.catalogRank ?? Number.MAX_SAFE_INTEGER) || left.id.localeCompare(right.id));
      runtime.gateway.visibleModelIds = runtime.gateway.models.filter((model) => model.enabled).map((model) => model.id);
    }

    function applyFreeRoutingPolicy(runtime: typeof localRuntime) {
      for (const item of runtime.accounts) {
        const free = item.subscription.planType?.toLowerCase().includes("free") ?? false;
        item.routingExclusion = free && !runtime.gateway.useFreeAccounts ? "free_plan_policy" : null;
      }
      refreshGatewayModels(runtime);
    }

    function remoteAction(args: Record<string, unknown>) {
      const input = args.input as { action?: { type?: string; id?: string }; payload?: Record<string, unknown> };
      const type = input?.action?.type;
      if (type === "rotate_key") return { key, secret: "zlr_synthetic_remote_rotated_key" };
      if (type === "create_key") return { key, secret: "zlr_synthetic_remote_key" };
      if (type === "create_source") {
        const created = sourceFromPayload(input.payload ?? {}, `source_remote_created_${remoteRuntime.sources.length + 1}`);
        remoteRuntime.sources = [...remoteRuntime.sources, created];
        return structuredClone(created);
      }
      if (type === "update_source") {
        const target = remoteRuntime.sources.find((item) => item.id === input.action?.id);
        if (target) Object.assign(target, input.payload);
        refreshGatewayModels(remoteRuntime);
        return structuredClone(target ?? null);
      }
      if (type === "update_key") {
        const target = remoteRuntime.keys.find((item) => item.id === input.action?.id);
        if (target) Object.assign(target, input.payload);
        return structuredClone(target ?? null);
      }
      if (type === "delete_key") {
        remoteRuntime.keys = remoteRuntime.keys.filter((item) => item.id !== input.action?.id);
        return null;
      }
      if (type === "preview_account_batch_import") return importSession("remote_import");
      if (type === "confirm_account_batch_import") return importConfirmation("remote_import", input.payload?.selectedItemIds as string[] ?? []);
      if (type === "update_account") {
        const target = remoteRuntime.accounts.find((item) => item.id === input.action?.id);
        if (target && typeof input.payload?.enabled === "boolean") target.enabled = input.payload.enabled;
        if (target && typeof input.payload?.draining === "boolean") target.draining = input.payload.draining;
        if (target && typeof input.payload?.priority === "number") target.priority = input.payload.priority;
        if (target && typeof input.payload?.weight === "number") target.weight = input.payload.weight;
        return structuredClone(target ?? null);
      }
      if (type === "delete_account") {
        remoteRuntime.accounts = remoteRuntime.accounts.filter((item) => item.id !== input.action?.id);
        refreshGatewayModels(remoteRuntime);
        return null;
      }
      if (type === "test_source") return structuredClone(source);
      if (type === "test_wake_task") return { taskId: String(input.action?.id), status: "ready", eligibleAccounts: 1 };
      if (type === "set_common_proxy") {
        const proxyUrl = input.payload?.proxyUrl;
        remoteRuntime.gateway.commonProxyConfigured = Boolean(proxyUrl);
        remoteRuntime.gateway.commonProxyAvailable = Boolean(proxyUrl);
        for (const item of remoteRuntime.accounts) {
          if (item.proxyMode === "account") continue;
          item.proxyMode = proxyUrl ? "common" : "direct";
          item.proxyAvailable = Boolean(proxyUrl) || !remoteRuntime.gateway.accountProxyRequired;
        }
        return structuredClone(remoteRuntime);
      }
      if (type === "set_account_proxy_required") {
        remoteRuntime.gateway.accountProxyRequired = Boolean(input.payload?.required);
        for (const item of remoteRuntime.accounts) {
          if (item.proxyMode === "direct") item.proxyAvailable = !remoteRuntime.gateway.accountProxyRequired;
        }
        return structuredClone(remoteRuntime);
      }
      if (type === "set_account_proxy") {
        account.proxyMode = input.payload?.proxyUrl ? "account" : remoteRuntime.gateway.commonProxyConfigured ? "common" : "direct";
        account.proxyAvailable = true;
        return structuredClone(account);
      }
      if (type === "assign_account_proxies") {
        const accountIds = input.payload?.accountIds as string[] ?? [];
        const proxyUrls = input.payload?.proxyUrls as string[] ?? [];
        account.proxyMode = "account";
        account.proxyAvailable = true;
        return { assigned: accountIds.length, unused: proxyUrls.length - accountIds.length };
      }
      if (type === "set_pool_membership") {
        const accountIds = input.payload?.accountIds as string[] ?? [];
        const sourceIds = input.payload?.sourceIds as string[] ?? [];
        const inPool = Boolean(input.payload?.inPool);
        for (const item of remoteRuntime.accounts) if (accountIds.includes(item.id)) item.inPool = inPool;
        for (const item of remoteRuntime.sources) if (sourceIds.includes(item.id)) item.inPool = inPool;
        remoteRuntime.gateway.candidateCount = [...remoteRuntime.accounts, ...remoteRuntime.sources].filter((item) => item.enabled && item.inPool && !item.draining).length;
        refreshGatewayModels(remoteRuntime);
        return structuredClone(remoteRuntime);
      }
      if (type === "set_model_enabled") {
        const modelId = String(input.payload?.modelId ?? "");
        const target = remoteRuntime.gateway.models.find((model) => model.id === modelId);
        if (target) target.enabled = Boolean(input.payload?.enabled);
        remoteRuntime.gateway.visibleModelIds = remoteRuntime.gateway.models.filter((model) => model.enabled).map((model) => model.id);
        return structuredClone(remoteRuntime);
      }
      if (type === "set_quota_policy") {
        remoteRuntime.gateway.quotaRefreshIntervalSeconds = Number(input.payload?.refreshIntervalSeconds);
        remoteRuntime.gateway.quotaRequestTimeoutSeconds = Number(input.payload?.requestTimeoutSeconds);
        remoteRuntime.gateway.useFreeAccounts = Boolean(input.payload?.useFreeAccounts);
        applyFreeRoutingPolicy(remoteRuntime);
        return structuredClone(remoteRuntime);
      }
      if (type === "set_routing_policy") {
        remoteRuntime.gateway.maxRetryCandidates = Number(input.payload?.maxRetryCandidates);
        if (input.payload?.routingStrategy) remoteRuntime.gateway.routingStrategy = input.payload.routingStrategy as "adaptive" | "oldest_account";
        if (input.payload?.defaultServiceTier) remoteRuntime.gateway.defaultServiceTier = input.payload.defaultServiceTier as "standard" | "fast";
        remoteRuntime.gateway.sessionAffinity = Boolean(input.payload?.sessionAffinity);
        remoteRuntime.gateway.sessionAffinityTtlSeconds = Number(input.payload?.sessionAffinityTtlSeconds);
        return structuredClone(remoteRuntime);
      }
      if (type === "refresh_pool_quotas") return { refreshed: remoteRuntime.accounts.filter((item) => item.inPool && item.enabled).length, failed: 0, snapshot: structuredClone(remoteRuntime) };
      if (type === "clear_usage") { remoteUsage = []; return null; }
      return null;
    }

    Object.defineProperty(window, "__TAURI_INTERNALS__", { configurable: true, value: tauri });
    Object.defineProperty(window, "__TAURI_EVENT_PLUGIN_INTERNALS__", {
      configurable: true,
      value: { unregisterListener: (_event: string, id: number) => callbacks.delete(id) },
    });
    Object.defineProperty(window, "__TAURI_TEST_INVOKES__", { configurable: true, value: invocations });
    Object.defineProperty(window, "__TAURI_TEST_EMIT__", {
      configurable: true,
      value: (event: string, payload: unknown) => {
        for (const [id, listener] of eventListeners) {
          if (listener.event === event) callbacks.get(listener.handler)?.({ event, id, payload });
        }
      },
    });
  }, options);
}
