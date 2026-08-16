import type { Page } from "../bun-playwright";

export type MockOptions = {
  locale?: "en" | "ru";
  onboarding?: boolean;
  mode?: "local" | "remote" | "zenith";
  theme?: "system" | "light" | "dark";
  populated?: boolean;
  readyConnected?: boolean;
  readyActive?: boolean;
  sourceCount?: number;
  accountCount?: number;
  accountHealth?: string;
  staleAccountError?: boolean;
  accountCooldown?: boolean;
  usageAccountIndex?: number;
  usagePresent?: boolean;
  usageActive?: boolean;
  usageFailure?: boolean;
  usageUnpricedTokens?: number;
  usageCandidateKind?: "account" | "source";
  usageRequestedModel?: string;
  usageResolvedModel?: string;
  activeModelCounts?: Array<{ model: string; requestCount: number }>;
  usageToolDiagnostics?: "forwarded_text_only" | "dropped_text_only";
  usageTotalPages?: number;
  planBenchmark?: boolean;
  accountAuthReason?: "invalid_grant" | "reused_refresh_token" | "expired_refresh_token" | "invalidated_refresh_token";
  codexBindings?: boolean;
  codexBindingActive?: boolean;
  codexBindingKind?: "oauth_account" | "local_gateway";
  codexBoundOauthAccountId?: string | null;
  profileSwitchError?: boolean;
  recoveryLoadError?: boolean;
  canonicalProfilePath?: boolean;
  moveAccountsError?: boolean;
  profileSnapshotsEmpty?: boolean;
  supplementalQuota?: boolean;
  subscriptionExpiresInMs?: number;
  exhaustedQuotaWindow?: "primary" | "secondary";
  quotaAvailable?: boolean;
  quotaRefreshStatus?: "pending" | "refreshing" | "updated" | "failed" | "requires_reauth";
  freeAccountHealthy?: boolean;
  gatewayRunning?: boolean;
  poolMembers?: boolean;
  proxyCount?: number;
  importResult?: "success" | "item_failure" | "not_found";
  importFailureCode?: string;
  importPreviewDelayMs?: number;
  importConfirmDelayMs?: number;
  importDescription?: string;
  currentProfileAvailable?: boolean;
  remoteUsageLabelMissing?: boolean;
  staleAccountReferences?: boolean;
  remoteConnected?: boolean;
  remoteFeatures?: string[];
  oauthCallbackBeforeStartReturns?: boolean;
  updateVersion?: string;
  updateBody?: string;
  updateDate?: string;
  portableUpdateTargetMissing?: boolean;
  updateCheckError?: boolean;
  bundleType?: "nsis" | "msi" | null;
  profileSwitchBackupPrompt?: boolean;
  profileSnapshotBackupBeforeRestore?: boolean;
  mixedModels?: boolean;
  serverModelOrder?: string[];
  sourceDetectedModelPrices?: Record<string, {
    inputMicroUsdPerMillion: number;
    cachedInputMicroUsdPerMillion?: number;
    cacheWrite5mMicroUsdPerMillion?: number;
    cacheWrite1hMicroUsdPerMillion?: number;
    outputMicroUsdPerMillion: number;
  }>;
  modelReasoning?: Record<string, string[]>;
  sourceProtocolBindings?: Array<{
    wireApi: "responses" | "messages" | "chat_completions";
    adapter: "native" | "responses_to_messages";
    reasoningMode: "disabled" | "budget" | "adaptive";
    modelIds: string[];
  }>;
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
    const usagePresent = populated && input.usagePresent !== false;
    const dayMs = 24 * 60 * 60_000;
    localStorage.setItem("relay.onboarding", input.onboarding === false ? "0" : "1");
    localStorage.setItem("relay.mode", input.mode ?? "local");
    localStorage.setItem("relay.theme", input.theme ?? "light");
    // Test fixtures seed the preference once.  A renderer reload must use the
    // persisted WebView value just like the packaged desktop application.
    if (localStorage.getItem("relay.profileSwitchBackupPrompt") === null) {
      localStorage.setItem("relay.profileSwitchBackupPrompt", input.profileSwitchBackupPrompt ? "1" : "0");
    }
    if (localStorage.getItem("relay.profileSnapshotBackupBeforeRestore") === null) {
      localStorage.setItem("relay.profileSnapshotBackupBeforeRestore", input.profileSnapshotBackupBeforeRestore === false ? "0" : "1");
    }

    type MockQuotaWindow = { kind: "primary" | "secondary"; availableBasisPoints: number; explicitlyFull: boolean; resetAtMs: number; windowMinutes: number; observedAtMs: number };
    type MockOperationalStatus = "rotation" | "quotaWait" | "unavailable" | "disabled";
    type MockAuthState = { state: string; reason?: string };
    const exhaustedQuotaWindow = input.quotaAvailable ? null : input.exhaustedQuotaWindow ?? "primary";
    const quotaNowMs = Date.now();
    const primaryResetAtMs = quotaNowMs + 90 * 60_000;
    const secondaryResetAtMs = quotaNowMs + 3 * 24 * 60 * 60_000;
    const quota: { primary: MockQuotaWindow | null; secondary: MockQuotaWindow | null; supplemental: Array<{ id: string; label: string; window: MockQuotaWindow }>; limitReached: boolean; resetCreditsAvailable: number; updatedAtMs: number; error: null } = {
      primary: { kind: "primary", availableBasisPoints: exhaustedQuotaWindow === "primary" ? 0 : 7200, explicitlyFull: false, resetAtMs: primaryResetAtMs, windowMinutes: 300, observedAtMs: quotaNowMs },
      secondary: { kind: "secondary", availableBasisPoints: exhaustedQuotaWindow === "secondary" ? 0 : 6400, explicitlyFull: false, resetAtMs: secondaryResetAtMs, windowMinutes: 10_080, observedAtMs: quotaNowMs },
      supplemental: input.supplementalQuota ? [
        { id: "code_review:primary", label: "Code Review", window: { kind: "primary", availableBasisPoints: 7200, explicitlyFull: false, resetAtMs: Date.now() + 2 * 60 * 60_000, windowMinutes: 300, observedAtMs: Date.now() } },
        { id: "code_review:secondary", label: "Code Review", window: { kind: "secondary", availableBasisPoints: 8600, explicitlyFull: false, resetAtMs: Date.now() + 5 * 24 * 60 * 60_000, windowMinutes: 10_080, observedAtMs: Date.now() } },
        { id: "additional:0:primary", label: "GPT-5.4 priority", window: { kind: "primary", availableBasisPoints: 4100, explicitlyFull: false, resetAtMs: Date.now() + 12 * 60 * 60_000, windowMinutes: 1_440, observedAtMs: Date.now() } },
      ] : [],
      limitReached: false,
      resetCreditsAvailable: 1,
      updatedAtMs: Date.now(),
      error: null as { code: string; occurredAtMs: number } | null,
    };
    const sourceModels = input.serverModelOrder ?? (input.mixedModels
      ? ["gpt-5.4", "claude-opus-4-8", "gemini-3.1-pro-preview", "grok-4.5", "glm-5.2", "private-model"]
      : ["gpt-5.4", "gpt-5.4-mini"]);
    const source = {
      id: "source_synthetic",
      name: "Example compatible API",
      enabled: true,
      inPool: input.poolMembers ?? true,
      draining: false,
      operationalStatus: "rotation" as MockOperationalStatus,
      baseUrl: "https://api.zenithmarket.dev/v1",
      wireApi: "responses" as const,
      protocolBindings: input.sourceProtocolBindings ?? [{
        wireApi: "responses" as const,
        adapter: "native" as const,
        reasoningMode: "disabled" as const,
        modelIds: [...sourceModels],
      }],
      models: sourceModels,
      allowedModels: [],
      excludedModels: [],
      priority: 10,
      weight: 100,
      recoveryDelaySeconds: 0,
      modelPriceOverrides: {},
      detectedModelPrices: input.sourceDetectedModelPrices ?? {},
      apiEquivalent: { microUsd: 8_500, pricedTokens: 1_400, unpricedTokens: 0 },
      secretAvailable: true,
      lastErrorCode: null,
    };
    const sourceCount = Math.max(1, Math.min(8, Math.trunc(input.sourceCount ?? 1)));
    const sources = [source, ...Array.from({ length: sourceCount - 1 }, (_, index) => ({
      ...source,
      id: `source_synthetic_${index + 2}`,
      name: `Backup API ${index + 1}`,
      priority: source.priority - index - 1,
      protocolBindings: structuredClone(source.protocolBindings),
      models: [...source.models],
      allowedModels: [...source.allowedModels],
      excludedModels: [...source.excludedModels],
    }))];
    const account = {
      id: "account_synthetic",
      label: "Personal Plus",
      identityHint: "p***@example.test",
      enabled: true,
      inPool: input.poolMembers ?? true,
      draining: false,
      operationalStatus: "rotation" as MockOperationalStatus,
      authState: (input.accountAuthReason ? { state: "requires_reauth", reason: input.accountAuthReason } : { state: "active" }) as MockAuthState,
      health: input.staleAccountError ? "degraded" : input.accountHealth ?? "healthy" as string,
      models: ["gpt-5.4", "gpt-5.4-mini"],
      allowedModels: [],
      excludedModels: [],
      priority: 20,
      weight: 100,
      apiEquivalent: { microUsd: 14_100_000, pricedTokens: 2_800_000, unpricedTokens: 0 },
      economics: {
        purchaseCostMicroUsd: 18_000_000,
        potentialMicroUsd: 24_000_000,
        potentialLowMicroUsd: 20_000_000,
        potentialHighMicroUsd: 28_000_000,
        potentialRequests: 220,
        potentialTotalTokens: 2_400_000,
        estimateState: "estimated" as const,
        confidence: "medium" as const,
        observedBasisPoints: 340,
        sampleCount: 3,
        windows: [
          { kind: "primary" as const, potentialMicroUsd: 4_200_000, potentialLowMicroUsd: 3_800_000, potentialHighMicroUsd: 4_700_000, potentialRequests: 38, potentialTotalTokens: 420_000, fullWindowMicroUsd: 5_830_000, estimateState: "estimated" as const, confidence: "medium" as const, observedBasisPoints: 340, sampleCount: 3, serviceTiers: [{ serviceTier: "standard" as const, potentialMicroUsd: 4_200_000, potentialRequests: 38, potentialTotalTokens: 420_000, observedBasisPoints: 340, sampleCount: 3 }] },
          { kind: "secondary" as const, potentialMicroUsd: 24_000_000, potentialLowMicroUsd: 20_000_000, potentialHighMicroUsd: 28_000_000, potentialRequests: 220, potentialTotalTokens: 2_400_000, fullWindowMicroUsd: 37_500_000, estimateState: "estimated" as const, confidence: "medium" as const, observedBasisPoints: 340, sampleCount: 3, serviceTiers: [{ serviceTier: "standard" as const, potentialMicroUsd: 24_000_000, potentialRequests: 220, potentialTotalTokens: 2_400_000, observedBasisPoints: 340, sampleCount: 3 }] },
        ],
      },
      subscription: { planType: input.supplementalQuota ? "pro" : "plus", activeUntilMs: Date.now() + (input.subscriptionExpiresInMs ?? 37 * dayMs), status: "active", updatedAtMs: Date.now() },
      quota,
      quotaRefreshStatus: input.quotaRefreshStatus ?? "updated" as "pending" | "refreshing" | "updated" | "failed" | "requires_reauth",
      secretAvailable: true,
      remoteLocation: null as { serverId: string; remoteAccountId: string } | null,
      proxyMode: "common",
      proxyAvailable: true,
      lastErrorCode: input.staleAccountError ? "quota_transport" : input.accountAuthReason ? "quota_token_prepare" : null as string | null,
    };
    const accountCount = Math.max(1, Math.min(input.accountCount ?? 1, 6));
    const accountVariants = [
      { label: "Personal Plus", plan: "plus", activeUntilMs: Date.now() + 37 * dayMs, proxyMode: "common", models: ["gpt-5.4", "gpt-5.4-mini"], primary: 0, primaryMinutes: 300, secondary: 6400, priority: 20, health: "healthy", error: null },
      { label: "Business Workspace", plan: "team", activeUntilMs: Date.now() + 203 * dayMs, proxyMode: "account", models: ["gpt-5.4", "gpt-5.4-mini", "o3"], primary: 3800, primaryMinutes: 50_400, secondary: null, priority: 30, health: "healthy", error: null },
      { label: "Backup account", plan: "free", activeUntilMs: null, proxyMode: "direct", models: ["gpt-5.4-mini"], primary: 9500, primaryMinutes: 43_200, secondary: null, priority: 10, health: "degraded", error: "quota_transport" },
      { label: "Pro account", plan: "pro", activeUntilMs: Date.now() + 172 * dayMs, proxyMode: "common", models: ["gpt-5.4", "gpt-5.4-mini", "o3"], primary: 7600, primaryMinutes: 300, secondary: 8200, priority: 25, health: "healthy", error: null },
      { label: "Quota pending", plan: "plus", activeUntilMs: Date.now() + 46 * dayMs, proxyMode: "common", models: ["gpt-5.4-mini"], primary: null, primaryMinutes: 300, secondary: null, priority: 1, health: "healthy", error: null },
      { label: "Free reserve", plan: "free", activeUntilMs: null, proxyMode: "direct", models: ["gpt-5.4-mini"], primary: 8800, primaryMinutes: 43_200, secondary: null, priority: 1, health: "healthy", error: null },
    ] as const;
    const accounts = Array.from({ length: accountCount }, (_, index) => {
      if (index === 0) return account;
      const variant = accountVariants[index % accountVariants.length];
      const item = structuredClone(account);
      item.id = `account_synthetic_${index + 1}`;
      item.label = variant.label;
      item.identityHint = ["p***@example.test", "b***@example.test", "r***@example.test", "q***@example.test", "s***@example.test", "t***@example.test"][index % 6];
      item.authState = { state: "active" };
      item.subscription.planType = variant.plan;
      item.subscription.activeUntilMs = variant.activeUntilMs;
      item.proxyMode = variant.proxyMode;
      item.models = [...variant.models];
      item.priority = variant.priority;
      const economics = [
        null,
        { used: 31_500_000, purchase: 24_000_000, potential: 62_000_000, low: 57_000_000, high: 68_000_000, state: "estimated", confidence: "high", samples: 4, observed: 420 },
        { used: 3_800_000, purchase: 8_000_000, potential: null, low: null, high: null, state: "stale", confidence: null, samples: 1, observed: 80 },
        { used: 65_000_000, purchase: 70_000_000, potential: 120_000_000, low: 98_000_000, high: 136_000_000, state: "estimated", confidence: "medium", samples: 3, observed: 250 },
        { used: 500_000, purchase: 12_000_000, potential: null, low: null, high: null, state: "collecting", confidence: null, samples: 0, observed: 4 },
        { used: 4_200_000, purchase: null, potential: 9_000_000, low: 5_000_000, high: 14_000_000, state: "estimated", confidence: "low", samples: 2, observed: 40 },
      ][index % 6];
      if (economics) {
        item.apiEquivalent.microUsd = economics.used;
        item.apiEquivalent.pricedTokens = 2_800_000 * (index + 1);
        item.economics = {
          purchaseCostMicroUsd: economics.purchase,
          potentialMicroUsd: economics.potential,
          potentialLowMicroUsd: economics.low,
          potentialHighMicroUsd: economics.high,
          potentialRequests: economics.potential == null ? null : 120,
          potentialTotalTokens: economics.potential == null ? null : 1_200_000,
          estimateState: economics.state as "collecting" | "estimated" | "stale",
          confidence: economics.confidence as "low" | "medium" | "high" | null,
          observedBasisPoints: economics.observed,
          sampleCount: economics.samples,
          windows: [],
        };
      }
      const healthyFree = variant.plan === "free" && input.freeAccountHealthy;
      item.health = healthyFree ? "healthy" : variant.health;
      item.lastErrorCode = healthyFree ? null : variant.error;
      item.quota.error = healthyFree ? null : variant.error ? { code: variant.error, observedAtMs: Date.now() } : null;
      if (variant.primary === null) {
        item.quota.primary = null;
        item.quota.secondary = null;
        item.quota.updatedAtMs = null;
      }
      else if (item.quota.primary) {
        item.quota.primary.availableBasisPoints = variant.primary;
        item.quota.primary.windowMinutes = variant.primaryMinutes;
      }
      if (variant.secondary === null) item.quota.secondary = null;
      else if (item.quota.secondary) item.quota.secondary.availableBasisPoints = variant.secondary;
      item.quotaRefreshStatus = item.quota.error ? "failed" : item.quota.updatedAtMs == null ? "pending" : "updated";
      return item;
    });
    if (input.planBenchmark) {
      for (const [index, label, plan, multiplier] of [
        [1, "Peer Plus", account.subscription.planType, 1.2],
        [2, "Outlier Plus", account.subscription.planType, 20],
        [3, "Peer Business", "team", 100],
      ] as const) {
        const peer = accounts[index];
        if (!peer) continue;
        peer.label = label;
        peer.subscription.planType = plan;
        peer.quota = structuredClone(account.quota);
        peer.economics = structuredClone(account.economics);
        for (const window of peer.economics.windows) {
          window.fullWindowMicroUsd = Math.round(window.fullWindowMicroUsd * multiplier);
        }
      }
      for (const [window, quotaWindow] of [[account.economics.windows[0], account.quota.primary], [account.economics.windows[1], account.quota.secondary]] as const) {
        const fullWindowMicroUsd = Math.round(window.fullWindowMicroUsd * 1.2);
        const availableBasisPoints = quotaWindow?.availableBasisPoints ?? 0;
        Object.assign(window, {
          planBenchmark: {
            provider: "chatgpt",
            plan: "plus",
            windowKind: window.kind,
            windowMinutes: quotaWindow?.windowMinutes ?? 0,
            serviceTier: "standard",
            pricingRevision: "mock-pricing",
            accountCount: 3,
            cycleCount: 9,
            latestCompletedAtMs: Date.now() - dayMs,
            stale: false,
            confidence: "low",
            fullWindowMicroUsd,
            meanFullWindowMicroUsd: Math.round(window.fullWindowMicroUsd * 7.4),
            lowFullWindowMicroUsd: window.fullWindowMicroUsd,
            highFullWindowMicroUsd: window.fullWindowMicroUsd * 20,
            potentialMicroUsd: Math.round(fullWindowMicroUsd * availableBasisPoints / 10_000),
            weeklyEquivalentMicroUsd: null,
          },
        });
      }
    }
    const systemCredentialId = "key_system";
    const profileDir = input.canonicalProfilePath ? "\\\\?\\C:\\Users\\Test\\.codex" : "C:\\Users\\Test\\.codex";
    let profileSnapshots = input.profileSnapshotsEmpty ? [] : [{
      id: "11111111-1111-4111-8111-111111111111",
      name: locale === "ru" ? "Исходный профиль" : "Original profile",
      profileDir,
      createdAtMs: Date.now() - 3_600_000,
      configAvailable: true,
      authAvailable: true,
    }];
    type MockModelSummary = { id: string; enabled: boolean; memberCount: number; codexVisible: boolean; codexDisplayName: string; catalogRank: number | null; inputMicroUsdPerMillion: number | null; cachedInputMicroUsdPerMillion: number | null; cacheWrite5mMicroUsdPerMillion?: number | null; cacheWrite1hMicroUsdPerMillion?: number | null; outputMicroUsdPerMillion: number | null; customPrice: boolean; reasoningLevels: string[]; reasoningAllowedLevels: string[]; reasoningConfigurable: boolean };
    type MockCandidateRuntime = { candidateId: string; kind: "api_source" | "oauth_account"; available: boolean; inFlight: number; activeRequestCount: number; activeModels: Array<{ model: string; requestCount: number }>; lastUsedAtMs: number | null; nextRetryAtMs: number | null; halfOpen: boolean; dispatches: number };
    const modelPrices: Record<string, Pick<MockModelSummary, "catalogRank" | "inputMicroUsdPerMillion" | "cachedInputMicroUsdPerMillion" | "outputMicroUsdPerMillion">> = {
      "gpt-5.4": { catalogRank: 5, inputMicroUsdPerMillion: 2_500_000, cachedInputMicroUsdPerMillion: 250_000, outputMicroUsdPerMillion: 15_000_000 },
      "gpt-5.4-mini": { catalogRank: 6, inputMicroUsdPerMillion: 750_000, cachedInputMicroUsdPerMillion: 75_000, outputMicroUsdPerMillion: 4_500_000 },
    };
    const modelGroupOrder = new Map(["openai", "anthropic", "other"].map((group, index) => [group, index]));
    function modelLeaf(model: string) { return model.trim().toLowerCase().split("/").at(-1) ?? model.trim().toLowerCase(); }
    function isOpenAiModel(model: string) { return /^(gpt-|codex-|o\d|text-|dall-e)/.test(model); }
    function modelProviderGroup(model: string) {
      const leaf = modelLeaf(model);
      if (leaf.startsWith("claude-")) return "anthropic";
      if (isOpenAiModel(leaf)) return "openai";
      return "other";
    }
    function compareModelOrder(left: MockModelSummary, right: MockModelSummary) {
      const leftGroup = modelGroupOrder.get(modelProviderGroup(left.id)) ?? 99;
      const rightGroup = modelGroupOrder.get(modelProviderGroup(right.id)) ?? 99;
      return leftGroup - rightGroup;
    }
    const automation = {
      id: "wake_synthetic",
      name: "Start quota countdown",
      enabled: true,
      accountSelector: { kind: "all_eligible" },
      windowKinds: ["primary"],
      modelPolicy: { kind: "lightest_supported" },
      trigger: { kind: "quota_full" },
      executionPolicy: "automatic",
      jitterSeconds: 0,
      maxAttemptsPerCycle: 1,
      createdAtMs: Date.now() - 86_400_000,
      updatedAtMs: Date.now() - 60_000,
    };
    const localRuntime = {
      schemaVersion: 14,
      configurationRevision: null as string | null,
      runtimeTarget: { kind: "local", connected: true, origin: "http://127.0.0.1:14998", serverId: null, version: "1.1.0" },
      gateway: { running: input.gatewayRunning ?? true, baseUrl: "http://127.0.0.1:14998/v1", candidateCount: 0, visibleModelIds: [] as string[], maxRetryCandidates: 3, cooldownAfterFailures: 3, keepLastCandidateAvailable: true, routingStrategy: "adaptive" as "adaptive" | "quota_highest" | "subscription_expiry" | "subscription_plan", subscriptionPlanOrder: [] as string[], defaultServiceTier: "standard" as "standard" | "fast", models: [] as MockModelSummary[], commonProxyConfigured: true, commonProxyAvailable: true, accountProxyRequired: false, quotaRequestTimeoutSeconds: 20, chatgptInterfaceQuotaReserveBasisPoints: 100, routingOrder: [] as MockCandidateRuntime[] },
      platform: "windows",
      capabilities: { features: ["sources", "oauth_accounts", "quota_wake", "profiles", "account_proxies", "account_export", "account_identity_reveal", "runtime_routing"], supportedWireApis: ["responses", "chat_completions", "messages"] as Array<"responses" | "chat_completions" | "messages"> },
      sources: populated ? sources : [],
      accounts: populated ? accounts : [],
      automations: populated ? [automation] : [],
      wakeHistory: populated ? [{ taskId: automation.id, accountId: account.id, windowKind: "primary", modelId: "gpt-5.4-mini", outcome: "confirmed", startedAtMs: Date.now() - 120_000, completedAtMs: Date.now() - 118_000, errorCode: null }] : [],
      warnings: [],
    };
    const usageAccount = accounts[Math.max(0, Math.min(input.usageAccountIndex ?? 0, accounts.length - 1))];
    const activeModelCounts = usagePresent && input.usageActive !== false
      ? input.activeModelCounts ?? [{ model: input.usageResolvedModel ?? input.usageRequestedModel ?? "gpt-5.4", requestCount: 1 }]
      : [];
    const activeRequestCount = activeModelCounts.reduce((count, item) => count + item.requestCount, 0);
    const orderedMembers = [
      usageAccount,
      accounts.find((item) => item.label === "Business Workspace"),
      ...sources,
      ...accounts,
    ].filter((item, index, items): item is typeof source | typeof account => Boolean(item) && items.findIndex((candidate) => candidate?.id === item?.id) === index);
    localRuntime.gateway.routingOrder = orderedMembers
      .filter((item) => populated && item.inPool)
      .map((item, index) => ({
        candidateId: item.id,
        kind: "baseUrl" in item ? "api_source" as const : "oauth_account" as const,
        available: false,
        inFlight: item.id === usageAccount.id ? activeRequestCount : 0,
        activeRequestCount: item.id === usageAccount.id ? activeRequestCount : 0,
        activeModels: item.id === usageAccount.id ? structuredClone(activeModelCounts) : [],
        lastUsedAtMs: usagePresent && item.id === usageAccount.id ? Date.now() - 1_000 : null,
        nextRetryAtMs: input.accountCooldown && item.id === account.id ? Date.now() + 30 * 60_000 : null,
        halfOpen: false,
        dispatches: index,
      }));
    refreshGatewayModels(localRuntime);
    const remoteRuntime = structuredClone(localRuntime);
    remoteRuntime.schemaVersion = 15;
    remoteRuntime.runtimeTarget = { kind: "remote", connected: true, origin: "https://relay.example.invalid", serverId: "server_synthetic", version: "1.1.0" };
    remoteRuntime.gateway.baseUrl = "https://relay.example.invalid/v1";
    remoteRuntime.platform = "linux";
    remoteRuntime.configurationRevision = "cfg_synthetic_current";
    remoteRuntime.capabilities = { features: input.remoteFeatures ?? ["sources", "accounts", "account_batch_import", "account_batch_import_creation_status", "account_import_to_pool", "account_export", "account_identity_reveal", "quota", "models", "model_pricing", "usage", "local_gateway", "profile_attach", "profile_key_rotation", "diagnostics", "wake_tasks", "account_proxies", "runtime_routing", "configuration_presets", "images"], supportedWireApis: ["responses", "chat_completions", "messages"] };
    const configurationPreset = {
      format: "zenith-relay-configuration",
      schemaVersion: 2,
      settings: {
        sources: remoteRuntime.sources.map((item) => ({ id: item.id, name: item.name, baseUrl: item.baseUrl, wireApi: item.wireApi, protocolBindings: item.protocolBindings, enabled: item.enabled, inPool: item.inPool, allowedModels: item.allowedModels, excludedModels: item.excludedModels, priority: item.priority, weight: item.weight, recoveryDelaySeconds: item.recoveryDelaySeconds, modelPriceOverrides: item.modelPriceOverrides })),
        accounts: remoteRuntime.accounts.map((item) => ({ id: item.id, identityHint: item.identityHint, enabled: item.enabled, inPool: item.inPool, allowedModels: item.allowedModels, excludedModels: item.excludedModels, priority: item.priority, weight: item.weight, proxyId: null })),
        routing: { maxRetryCandidates: 4, routingStrategy: remoteRuntime.gateway.routingStrategy, subscriptionPlanOrder: remoteRuntime.gateway.subscriptionPlanOrder, defaultServiceTier: remoteRuntime.gateway.defaultServiceTier, imageBaseModel: null },
        quota: { requestTimeoutSeconds: remoteRuntime.gateway.quotaRequestTimeoutSeconds, accountProxyRequired: remoteRuntime.gateway.accountProxyRequired, commonProxyId: null },
        hiddenModels: [],
        modelPriceOverrides: {},
      },
    };
    const assignedProxyAccount = localRuntime.accounts.find((item) => item.proxyMode === "account");
    let proxyEntries = Array.from({ length: input.proxyCount ?? (populated ? 3 : 0) }, (_, index) => ({
      id: `proxy_synthetic_${index + 1}`,
      endpoint: `http://proxy-${index + 1}.example.test:${10_000 + index}`,
      assignedAccountIds: index === 0 && input.staleAccountReferences ? ["account_deleted_internal"] : index === 0 && assignedProxyAccount ? [assignedProxyAccount.id] : [],
      countryCode: index === 0 ? "US" : null,
      region: index === 0 ? "Virginia" : null,
      createdAtMs: Date.now() - index * 60_000,
    }));
    const proxyPool = () => ({
      entries: structuredClone(proxyEntries),
      total: proxyEntries.length,
      free: proxyEntries.filter((entry) => !entry.assignedAccountIds.length).length,
      assigned: proxyEntries.filter((entry) => entry.assignedAccountIds.length).length,
    });

    function sourceFromPayload(payload: Record<string, unknown>, id: string) {
      const requestedModels = payload.models as string[] | undefined;
      const models = requestedModels?.length
        ? requestedModels
        : ["gpt-5.4", "gpt-5.4-mini", "o3"];
      const requestedBindings = Array.isArray(payload.protocolBindings)
        ? payload.protocolBindings as Array<{
          wireApi?: string;
          adapter?: string;
          reasoningMode?: string;
          modelIds?: string[];
        }>
        : [];
      const wireApi = String(payload.wireApi ?? requestedBindings[0]?.wireApi ?? source.wireApi);
      const protocolBindings = requestedBindings.length
        ? requestedBindings.map((binding) => ({
          wireApi: String(binding.wireApi ?? wireApi),
          adapter: binding.adapter === "responses_to_messages" && String(binding.wireApi ?? wireApi) === "responses"
            ? "responses_to_messages"
            : "native",
          reasoningMode: binding.reasoningMode === "budget" || binding.reasoningMode === "adaptive"
            ? binding.reasoningMode
            : "disabled",
          modelIds: binding.modelIds?.length
            ? [...binding.modelIds]
            : requestedBindings.length === 1
              ? [...models]
              : [],
        }))
        : [{
          wireApi,
          adapter: "native",
          reasoningMode: "disabled",
          modelIds: [...models],
        }];
      return {
        ...structuredClone(source),
        id,
        name: String(payload.name ?? source.name),
        baseUrl: String(payload.baseUrl ?? source.baseUrl),
        wireApi,
        protocolBindings,
        models,
        allowedModels: payload.allowedModels as string[] ?? [],
        excludedModels: payload.excludedModels as string[] ?? [],
        priority: Number(payload.priority ?? 0),
        weight: Number(payload.weight ?? 100),
        recoveryDelaySeconds: Number(payload.recoveryDelaySeconds ?? 0),
        modelPriceOverrides: payload.modelPriceOverrides as typeof source.modelPriceOverrides ?? {},
        draining: Boolean(payload.draining),
        inPool: false,
        operationalStatus: "disabled" as MockOperationalStatus,
      };
    }

    const sourceUsage = input.usageCandidateKind === "source";
    const routing = { reason: sourceUsage ? "weighted_rotation" : "quota_headroom", eligibleCandidates: 4, quotaRemainingBasisPoints: sourceUsage ? null : 6300, inFlightBefore: 0, dispatchesBefore: 3 };
    const localUnpricedTokens = Math.min(28, Math.max(0, input.usageUnpricedTokens ?? 0));
    const remoteUnpricedTokens = Math.min(25, Math.max(0, input.usageUnpricedTokens ?? 0));
    const requestedUsageModel = input.usageRequestedModel ?? "gpt-5.4";
    const resolvedUsageModel = input.usageResolvedModel ?? "gpt-5.4";
    const toolUse = input.usageToolDiagnostics ? {
      clientToolCount: 3,
      forwardedToolCount: input.usageToolDiagnostics === "forwarded_text_only" ? 3 : 0,
      toolChoice: "auto",
      toolCallCount: 0,
      textOutput: true,
      terminalOutput: "text",
    } : undefined;
    let localUsage = usagePresent ? [{ id: 1, createdAt: new Date().toISOString(), requestId: "req_synthetic_local", attempt: 1, sourceId: source.id, accountId: sourceUsage ? null : input.staleAccountReferences ? "account_deleted_internal" : usageAccount.id, requestedModel: requestedUsageModel, resolvedModel: resolvedUsageModel, requestedReasoningEffort: "max", effectiveReasoningEffort: "low", wireApi: "responses", serviceTier: "standard", success: !input.usageFailure, httpStatus: input.usageFailure ? 502 : 200, errorCategory: input.usageFailure ? "upstream_failure" : null, latencyMs: 428, ttftMs: 128, generationMs: 300, inputTokens: 20, cachedInputTokens: 12, reasoningTokens: 5, outputTokens: 8, totalTokens: 28, apiEquivalent: { microUsd: 148, pricedTokens: 28 - localUnpricedTokens, unpricedTokens: localUnpricedTokens }, toolUse, routing }] : [];
    let remoteUsage = usagePresent ? [{ id: 2, requestId: "req_synthetic_remote", candidateKind: sourceUsage ? "source" : "account", candidateHint: sourceUsage ? source.id : input.remoteUsageLabelMissing ? "4f5c821a909b" : "a1b2c3d4e5f6", candidateLabel: sourceUsage ? source.name : input.remoteUsageLabelMissing ? null : usageAccount.label, requestedModel: requestedUsageModel, resolvedModel: resolvedUsageModel, requestedReasoningEffort: "max", effectiveReasoningEffort: "low", wireApi: "responses", serviceTier: "fast", appliedServiceTier: "standard", success: true, httpStatus: 200, errorCategory: null, latencyMs: 512, ttftMs: 184, generationMs: 328, inputTokens: 18, cachedInputTokens: 10, reasoningTokens: 3, outputTokens: 7, totalTokens: 25, apiEquivalent: { microUsd: 148, pricedTokens: 25 - remoteUnpricedTokens, unpricedTokens: remoteUnpricedTokens }, createdAtMs: Date.now(), routing }] : [];
    function usageTotals(events: Array<{ success: boolean; latencyMs: number; ttftMs?: number | null; generationMs?: number | null; inputTokens: number | null; cachedInputTokens: number | null; reasoningTokens: number | null; outputTokens: number | null; totalTokens: number | null; apiEquivalent?: { microUsd: number; pricedTokens: number; unpricedTokens: number } }>) {
      return events.reduce((totals, item) => {
        const visibleOutputTokens = Math.max(0, (item.outputTokens ?? 0) - (item.reasoningTokens ?? 0));
        totals.requests += 1; totals.successfulRequests += Number(item.success); totals.latencyMs += item.latencyMs;
        if (item.ttftMs != null) { totals.ttftMs += item.ttftMs; totals.ttftSamples += 1; }
        if (item.generationMs != null) { totals.generationMs += item.generationMs; totals.generationSamples += 1; totals.generationOutputTokens += visibleOutputTokens; }
        totals.inputTokens += item.inputTokens ?? 0; totals.cachedInputTokens += item.cachedInputTokens ?? 0; totals.cachedInputSamples += Number(item.cachedInputTokens != null);
        totals.reasoningTokens += item.reasoningTokens ?? 0; totals.outputTokens += item.outputTokens ?? 0; totals.totalTokens += item.totalTokens ?? 0;
        if (item.success && visibleOutputTokens && item.latencyMs) { totals.speedOutputTokens += visibleOutputTokens; totals.speedDurationMs += item.latencyMs; }
        totals.apiEquivalent.microUsd += item.apiEquivalent?.microUsd ?? 0; totals.apiEquivalent.pricedTokens += item.apiEquivalent?.pricedTokens ?? 0; totals.apiEquivalent.unpricedTokens += item.apiEquivalent?.unpricedTokens ?? item.totalTokens ?? 0;
        return totals;
      }, { requests: 0, successfulRequests: 0, latencyMs: 0, ttftMs: 0, ttftSamples: 0, generationMs: 0, generationSamples: 0, generationOutputTokens: 0, inputTokens: 0, cachedInputTokens: 0, cachedInputSamples: 0, reasoningTokens: 0, outputTokens: 0, totalTokens: 0, speedOutputTokens: 0, speedDurationMs: 0, apiEquivalent: { microUsd: 0, pricedTokens: 0, unpricedTokens: 0 } });
    }
    let readyKey = input.readyConnected === false ? "" : "test_zenith_source_key";
    let readyActive = Boolean(readyKey) && (input.readyActive ?? true);
    const invocations: Array<{ command: string; args: Record<string, unknown> }> = [];
    const callbacks = new Map<number, (...args: unknown[]) => unknown>();
    let nextCallback = 1;
    const eventListeners = new Map<number, { event: string; handler: number }>();
    let nextEventListener = 1;
    const emitEvent = (event: string, payload: unknown) => {
      for (const [id, listener] of eventListeners) {
        if (listener.event === event) callbacks.get(listener.handler)?.({ event, id, payload });
      }
    };
    const sendChannel = (channel: unknown, message: unknown, index = 0) => {
      const id = Number((channel as { id?: number } | null)?.id);
      if (Number.isFinite(id)) callbacks.get(id)?.({ index, message });
    };

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
        const recordedArgs = command === "plugin:updater|download_and_install" || command === "install_portable_update"
          ? JSON.parse(JSON.stringify(args)) as Record<string, unknown>
          : structuredClone(args);
        invocations.push({ command, args: recordedArgs });
        switch (command) {
          case "get_system_locale": return locale;
          case "get_platform": return "windows";
          case "get_state": return { providerActive: readyActive, codexRunning: false, hasSavedApiKey: Boolean(readyKey) };
          case "get_saved_key_models": return ["gpt-5.4", "gpt-5.4-mini"];
          case "create_saved_top_up_intent_and_open": return null;
          case "open_api_key_page": return null;
          case "save_key": readyKey = String(args.apiKey ?? ""); if (args.activate !== false) readyActive = true; return readyKey;
          case "activate_ready_api_profile": readyActive = true; return "api";
          case "deactivate_ready_api_profile": readyActive = false; return "chat";
          case "reset_key": readyKey = ""; readyActive = false; return "reset";
          case "prepare_top_up_amount": return { amountCents: 1000, amountUsd: 10, valid: true };
          case "get_local_runtime_state": return structuredClone(localRuntime);
          case "get_local_runtime_order": return structuredClone(localRuntime.gateway.routingOrder);
          case "export_local_configuration_preset": return "C:\\Temp\\zenith-relay-configuration.json";
          case "get_remote_server_state": return input.remoteConnected === false ? null : structuredClone(remoteRuntime);
          case "get_remote_runtime_order": return input.remoteConnected === false ? null : structuredClone(remoteRuntime.gateway.routingOrder);
          case "export_remote_configuration_preset": return "C:\\Temp\\zenith-relay-configuration.json";
          case "preview_remote_configuration_preset": return { baseRevision: "cfg_synthetic_current", preset: structuredClone(configurationPreset), changes: [{ path: "/routing/maxRetryCandidates", before: 3, after: 4 }] };
          case "apply_remote_configuration_preset": remoteRuntime.gateway.maxRetryCandidates = 4; remoteRuntime.configurationRevision = "cfg_synthetic_applied"; return { previousRevision: "cfg_synthetic_current", revision: remoteRuntime.configurationRevision, changes: [{ path: "/routing/maxRetryCandidates", before: 3, after: 4 }] };
          case "get_local_usage": return structuredClone(localUsage);
          case "get_local_usage_page": {
            const query = (args.input ?? {}) as { page?: number; pageSize?: number; fromMs?: number; bucketMs?: number; success?: boolean; modelQuery?: string; sourceOrAccountQuery?: string; wireApi?: string; errorCategory?: string; requestIdQuery?: string };
            const events = localUsage.filter((item) => (query.success === undefined || item.success === query.success) && (!query.modelQuery || item.resolvedModel.includes(query.modelQuery)) && (!query.sourceOrAccountQuery || item.accountId?.includes(query.sourceOrAccountQuery) || item.sourceId.includes(query.sourceOrAccountQuery)) && (!query.wireApi || item.wireApi === query.wireApi) && (!query.errorCategory || item.errorCategory === query.errorCategory) && (!query.requestIdQuery || item.requestId.includes(query.requestIdQuery)));
            const totals = usageTotals(events);
            const createdAtMs = events[0] ? Date.parse(events[0].createdAt) : 0;
            const bucketStart = query.bucketMs && query.fromMs != null ? query.fromMs + Math.floor((createdAtMs - query.fromMs) / query.bucketMs) * query.bucketMs : null;
            const buckets = bucketStart == null ? [] : [{ startMs: bucketStart, totals }];
            return { events: structuredClone(events), total: events.length, page: query.page ?? 1, pageSize: query.pageSize ?? 50, totalPages: events.length ? input.usageTotalPages ?? 1 : 0, totals, buckets, models: events.length ? [{ key: "gpt-5.4", totals }] : [], poolMembers: events.length ? [{ key: sourceUsage ? source.id : account.id, label: sourceUsage ? source.name : account.label, totals }] : [] };
          }
          case "get_remote_server_usage": {
            const query = (args.input ?? {}) as { page?: number; pageSize?: number; fromMs?: number; bucketMs?: number; success?: boolean; modelQuery?: string; sourceOrAccountQuery?: string; wireApi?: string; errorCategory?: string; requestIdQuery?: string };
            const sourceOrAccountQuery = query.sourceOrAccountQuery === account.id ? "a1b2c3d4e5f6" : query.sourceOrAccountQuery;
            const events = remoteUsage.filter((item) => (query.success === undefined || item.success === query.success) && (!query.modelQuery || item.resolvedModel.includes(query.modelQuery)) && (!sourceOrAccountQuery || item.candidateHint.includes(sourceOrAccountQuery)) && (!query.wireApi || item.wireApi === query.wireApi) && (!query.errorCategory || item.errorCategory === query.errorCategory) && (!query.requestIdQuery || item.requestId.includes(query.requestIdQuery)));
            const totals = usageTotals(events);
            const createdAtMs = events[0]?.createdAtMs ?? 0;
            const bucketStart = query.bucketMs && query.fromMs != null ? query.fromMs + Math.floor((createdAtMs - query.fromMs) / query.bucketMs) * query.bucketMs : null;
            const buckets = bucketStart == null ? [] : [{ startMs: bucketStart, totals }];
            return { events: structuredClone(events), total: events.length, page: query.page ?? 1, pageSize: query.pageSize ?? 50, totalPages: events.length ? 1 : 0, totals, buckets, models: events.length ? [{ key: "gpt-5.4", totals }] : [], poolMembers: events.length ? [{ key: sourceUsage ? source.id : "a1b2c3d4e5f6", label: sourceUsage ? source.name : account.label, totals }] : [] };
          }
          case "create_local_source": {
            const created = sourceFromPayload(args.input as Record<string, unknown>, `source_created_${localRuntime.sources.length + 1}`);
            localRuntime.sources = [...localRuntime.sources, created];
            return structuredClone(created);
          }
          case "update_local_source": {
            const request = args.input as Record<string, unknown> & { sourceId?: string; sourcePriorities?: Record<string, number> };
            const target = localRuntime.sources.find((item) => item.id === request.sourceId);
            if (target) Object.assign(target, request);
            for (const [sourceId, priority] of Object.entries(request.sourcePriorities ?? {})) {
              const source = localRuntime.sources.find((item) => item.id === sourceId);
              if (source) source.priority = priority;
            }
            refreshGatewayModels(localRuntime);
            return structuredClone(localRuntime);
          }
          case "rotate_local_source_key": return structuredClone(localRuntime);
          case "set_local_source_enabled": {
            const target = localRuntime.sources.find((item) => item.id === args.sourceId);
            if (target) target.enabled = Boolean(args.enabled);
            refreshGatewayModels(localRuntime);
            return structuredClone(localRuntime);
          }
          case "delete_local_source": localRuntime.sources = []; refreshGatewayModels(localRuntime); return structuredClone(localRuntime);
          case "test_local_source": return structuredClone(source);
          case "get_local_source_stats":
          case "get_remote_source_stats": {
            const selected = localRuntime.sources.find((item) => item.id === String(args.sourceId));
            const host = selected ? new URL(selected.baseUrl).host.toLowerCase() : "";
            if (host === "api.zenithmarket.dev") return { provider: "zenith", balanceMicroUsd: 42_500_000, spentMicroUsd: 7_500_000, requests: 128, totalTokens: 987_654 };
            if (host === "openrouter.ai") return { provider: "openrouter", balanceMicroUsd: 21_250_000, spentMicroUsd: 3_750_000, requests: null, totalTokens: null };
            return { provider: "unsupported", balanceMicroUsd: null, spentMicroUsd: null, requests: null, totalTokens: null };
          }
          case "start_local_account_import": return importSession("11111111-2222-4333-8444-555555555555");
          case "current_chatgpt_profile_available": return input.currentProfileAvailable ?? true;
          case "preview_current_codex_account_import":
            if (input.importPreviewDelayMs) await new Promise((resolve) => setTimeout(resolve, input.importPreviewDelayMs));
            return importSession("current_codex_profile");
          case "preview_local_account_import_files":
            if (input.importPreviewDelayMs) await new Promise((resolve) => setTimeout(resolve, input.importPreviewDelayMs));
            return importSession("11111111-2222-4333-8444-555555555555");
          case "preview_remote_account_import_files":
            if (input.importPreviewDelayMs) await new Promise((resolve) => setTimeout(resolve, input.importPreviewDelayMs));
            return importSession("remote_import");
          case "resume_local_account_import": return importSession(String(args.sessionId ?? "11111111-2222-4333-8444-555555555555"));
          case "prepare_local_account_import": return importSession(String((args.input as { sessionId?: string })?.sessionId ?? "11111111-2222-4333-8444-555555555555"));
          case "confirm_local_account_import": {
            if (input.importResult === "not_found") throw { code: "not_found" };
            const request = args.input as { sessionId?: string; selectedItemIds?: string[] };
            const itemIds = request.selectedItemIds ?? [];
            if (input.importConfirmDelayMs) {
              emitEvent("relay-account-import-progress", { sessionId: request.sessionId, completed: 0, total: itemIds.length, succeeded: 0, failed: 0, currentLabel: "Imported account" });
              await new Promise((resolve) => setTimeout(resolve, Math.max(1, input.importConfirmDelayMs! / 2)));
              emitEvent("relay-account-import-progress", { sessionId: request.sessionId, completed: Math.min(1, itemIds.length), total: itemIds.length, succeeded: Math.min(1, itemIds.length), failed: 0, currentLabel: itemIds.length > 1 ? "Second imported account" : undefined });
              await new Promise((resolve) => setTimeout(resolve, Math.max(1, input.importConfirmDelayMs! / 2)));
            }
            return importConfirmation(request.sessionId ?? "11111111-2222-4333-8444-555555555555", itemIds);
          }
          case "cancel_local_account_import": return null;
          case "refresh_local_account_quota": return structuredClone(localRuntime);
          case "refresh_all_local_account_quotas": return localRuntime.accounts.map((item) => ({ accountId: item.id, status: "succeeded" }));
          case "update_local_account": {
            const request = args.input as { accountId?: string; priority?: number; weight?: number; draining?: boolean; purchaseCostMicroUsd?: number };
            const target = localRuntime.accounts.find((item) => item.id === request.accountId);
            if (target && typeof request.priority === "number") target.priority = request.priority;
            if (target && typeof request.weight === "number") target.weight = request.weight;
            if (target && typeof request.draining === "boolean") target.draining = request.draining;
            if (target && typeof request.purchaseCostMicroUsd === "number" && target.economics) target.economics.purchaseCostMicroUsd = request.purchaseCostMicroUsd || null;
            refreshGatewayModels(localRuntime);
            return structuredClone(localRuntime);
          }
          case "set_local_account_enabled": {
            const target = localRuntime.accounts.find((item) => item.id === args.accountId);
            if (target) target.enabled = Boolean(args.enabled);
            refreshGatewayModels(localRuntime);
            return structuredClone(localRuntime);
          }
          case "set_local_account_draining": {
            const target = localRuntime.accounts.find((item) => item.id === args.accountId);
            if (target) target.draining = Boolean(args.draining);
            refreshGatewayModels(localRuntime);
            return structuredClone(localRuntime);
          }
          case "set_local_pool_membership": {
            const request = args.input as { accountIds: string[]; sourceIds: string[]; inPool: boolean };
            for (const item of localRuntime.accounts) if (request.accountIds.includes(item.id)) item.inPool = request.inPool;
            for (const item of localRuntime.sources) if (request.sourceIds.includes(item.id)) item.inPool = request.inPool;
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
          case "set_local_model_price": {
            const request = args.input as { modelId: string; inputMicroUsdPerMillion: number | null; cachedInputMicroUsdPerMillion: number | null; cacheWrite5mMicroUsdPerMillion: number | null; cacheWrite1hMicroUsdPerMillion: number | null; outputMicroUsdPerMillion: number | null };
            const target = localRuntime.gateway.models.find((model) => model.id === request.modelId);
            if (target) {
              const custom = request.inputMicroUsdPerMillion != null && request.cachedInputMicroUsdPerMillion != null && request.outputMicroUsdPerMillion != null;
              const catalog = modelPrices[target.id.toLowerCase()] ?? { catalogRank: null, inputMicroUsdPerMillion: null, cachedInputMicroUsdPerMillion: null, outputMicroUsdPerMillion: null };
              target.inputMicroUsdPerMillion = custom ? request.inputMicroUsdPerMillion : catalog.inputMicroUsdPerMillion;
              target.cachedInputMicroUsdPerMillion = custom ? request.cachedInputMicroUsdPerMillion : catalog.cachedInputMicroUsdPerMillion;
              target.cacheWrite5mMicroUsdPerMillion = custom ? request.cacheWrite5mMicroUsdPerMillion : null;
              target.cacheWrite1hMicroUsdPerMillion = custom ? request.cacheWrite1hMicroUsdPerMillion : null;
              target.outputMicroUsdPerMillion = custom ? request.outputMicroUsdPerMillion : catalog.outputMicroUsdPerMillion;
              target.customPrice = custom;
            }
            return structuredClone(localRuntime);
          }
          case "set_local_model_reasoning": {
            const request = args.input as { modelId: string; allowedLevels: string[] };
            const target = localRuntime.gateway.models.find((model) => model.id === request.modelId);
            if (target) target.reasoningAllowedLevels = [...request.allowedLevels];
            return structuredClone(localRuntime);
          }
          case "update_chatgpt_interface_quota_reserve": {
            const request = args.input as { reserveBasisPoints: number };
            localRuntime.gateway.chatgptInterfaceQuotaReserveBasisPoints = request.reserveBasisPoints;
            return structuredClone(localRuntime);
          }
          case "update_local_routing": {
            const request = args.input as { maxRetryCandidates: number; cooldownAfterFailures: number; keepLastCandidateAvailable: boolean; routingStrategy: "adaptive" | "quota_highest" | "subscription_expiry" | "subscription_plan"; subscriptionPlanOrder: string[]; defaultServiceTier: "standard" | "fast" };
            localRuntime.gateway.maxRetryCandidates = request.maxRetryCandidates;
            localRuntime.gateway.cooldownAfterFailures = request.cooldownAfterFailures;
            localRuntime.gateway.keepLastCandidateAvailable = request.keepLastCandidateAvailable;
            localRuntime.gateway.routingStrategy = request.routingStrategy;
            localRuntime.gateway.subscriptionPlanOrder = [...request.subscriptionPlanOrder];
            localRuntime.gateway.defaultServiceTier = request.defaultServiceTier;
            return structuredClone(localRuntime);
          }
          case "delete_local_account": {
            for (const entry of proxyEntries) entry.assignedAccountIds = entry.assignedAccountIds.filter((accountId) => accountId !== args.accountId);
            localRuntime.accounts = localRuntime.accounts.filter((item) => item.id !== args.accountId);
            refreshGatewayModels(localRuntime);
            return structuredClone(localRuntime);
          }
          case "delete_local_accounts": {
            const accountIds = new Set((args.accountIds as string[] | undefined) ?? []);
            for (const entry of proxyEntries) entry.assignedAccountIds = entry.assignedAccountIds.filter((accountId) => !accountIds.has(accountId));
            localRuntime.accounts = localRuntime.accounts.filter((item) => !accountIds.has(item.id));
            refreshGatewayModels(localRuntime);
            return structuredClone(localRuntime);
          }
          case "set_local_account_proxy": {
            const request = args.input as { accountId: string; proxyUrl: string | null; bypassCommonProxy?: boolean };
            const target = localRuntime.accounts.find((item) => item.id === request.accountId);
            for (const entry of proxyEntries) entry.assignedAccountIds = entry.assignedAccountIds.filter((accountId) => accountId !== request.accountId);
            if (request.proxyUrl) {
              proxyEntries.push({ id: `proxy_synthetic_${proxyEntries.length + 1}`, endpoint: `http://custom-proxy.example.test:1080`, assignedAccountIds: [request.accountId], countryCode: null, region: null, createdAtMs: Date.now() });
            }
            if (target) {
              target.proxyMode = request.proxyUrl ? "account" : request.bypassCommonProxy ? "direct" : localRuntime.gateway.commonProxyConfigured ? "common" : "direct";
              target.proxyAvailable = target.proxyMode !== "direct" || !localRuntime.gateway.accountProxyRequired;
            }
            refreshGatewayModels(localRuntime);
            return structuredClone(localRuntime);
          }
          case "get_local_proxy_pool": return proxyPool();
          case "import_local_proxy_pool": {
            const values = (args.input as { proxyUrls?: string[] })?.proxyUrls ?? [];
            let added = 0;
            let duplicates = 0;
            for (const value of values) {
              const match = value.match(/^(?:https?:\/\/)?([^:@]+):(\d+)/);
              const endpoint = match ? `http://${match[1]}:${match[2]}` : `http://imported-${proxyEntries.length + 1}.example.test:8080`;
              if (proxyEntries.some((entry) => entry.endpoint === endpoint)) {
                duplicates += 1;
                continue;
              }
              const countryCode = value.match(/__cr[.-]([a-z]{2})/i)?.[1]?.toUpperCase() ?? null;
              proxyEntries.push({ id: `proxy_synthetic_${proxyEntries.length + 1}`, endpoint, assignedAccountIds: [], countryCode, region: null, createdAtMs: Date.now() });
              added += 1;
            }
            return { added, duplicates, pool: proxyPool() };
          }
          case "delete_local_stored_proxy": {
            proxyEntries = proxyEntries.filter((entry) => entry.id !== String(args.proxyId));
            return proxyPool();
          }
          case "delete_local_stored_proxies": {
            const proxyIds = (args.input as { proxyIds?: string[] })?.proxyIds ?? [];
            proxyEntries = proxyEntries.filter((entry) => !proxyIds.includes(entry.id));
            return proxyPool();
          }
          case "assign_local_stored_proxy": {
            const request = args.input as { accountId: string; proxyId: string };
            for (const entry of proxyEntries) entry.assignedAccountIds = entry.assignedAccountIds.filter((accountId) => accountId !== request.accountId);
            const targetProxy = proxyEntries.find((entry) => entry.id === request.proxyId);
            if (targetProxy) targetProxy.assignedAccountIds.push(request.accountId);
            const targetAccount = localRuntime.accounts.find((item) => item.id === request.accountId);
            if (targetAccount && targetProxy) targetAccount.proxyMode = "account";
            refreshGatewayModels(localRuntime);
            return { assigned: targetProxy ? 1 : 0, unchanged: 0, unavailable: targetProxy ? 0 : 1, pool: proxyPool() };
          }
          case "set_local_stored_proxy_accounts": {
            const request = args.input as { proxyId: string; accountIds: string[] };
            const targetProxy = proxyEntries.find((entry) => entry.id === request.proxyId);
            const previous = targetProxy?.assignedAccountIds ?? [];
            for (const accountId of request.accountIds) {
              for (const entry of proxyEntries) entry.assignedAccountIds = entry.assignedAccountIds.filter((id) => id !== accountId);
            }
            if (targetProxy) targetProxy.assignedAccountIds = [...request.accountIds];
            for (const account of localRuntime.accounts) {
              if (request.accountIds.includes(account.id)) account.proxyMode = "account";
              else if (previous.includes(account.id)) account.proxyMode = localRuntime.gateway.commonProxyConfigured ? "common" : "direct";
            }
            refreshGatewayModels(localRuntime);
            return { assigned: request.accountIds.length, unchanged: 0, unavailable: 0, pool: proxyPool() };
          }
          case "assign_free_local_account_proxies": {
            const accountIds = (args.input as { accountIds?: string[] })?.accountIds ?? [];
            let assigned = 0;
            let unchanged = 0;
            let unavailable = 0;
            for (const accountId of accountIds) {
              if (proxyEntries.some((entry) => entry.assignedAccountIds.includes(accountId))) {
                unchanged += 1;
                continue;
              }
              const targetProxy = proxyEntries.reduce<(typeof proxyEntries)[number] | undefined>(
                (leastUsed, entry) => !leastUsed || entry.assignedAccountIds.length < leastUsed.assignedAccountIds.length ? entry : leastUsed,
                undefined,
              );
              if (!targetProxy) {
                unavailable += 1;
                continue;
              }
              targetProxy.assignedAccountIds.push(accountId);
              const target = localRuntime.accounts.find((item) => item.id === accountId);
              if (target) target.proxyMode = "account";
              assigned += 1;
            }
            refreshGatewayModels(localRuntime);
            return { assigned, unchanged, unavailable, pool: proxyPool() };
          }
          case "export_local_accounts":
          case "export_remote_accounts": {
            const request = args.input as { accountIds: string[]; format: string; destination: "copy" | "download"; description?: string };
            const result = {
              format: request.format,
              accountCount: request.accountIds.length,
              fileName: `${request.accountIds.length === 1 ? "account" : "accounts"}-${request.format}.json`,
            };
            return request.destination === "copy"
              ? { ...result, content: request.format === "zenith"
                ? JSON.stringify({ format: "zenith", version: 1, description: request.description, accounts: [{ auth: { accessToken: "synthetic-export-token" } }] })
                : JSON.stringify({ access_token: "synthetic-export-token", account_ids: request.accountIds }) }
              : { ...result, path: `C:\\Temp\\${result.fileName}` };
          }
          case "move_local_accounts_to_remote": {
            if (input.moveAccountsError) throw { code: "gateway_unavailable", message: "Synthetic remote move failure" };
            const accountIds = (args.input as { accountIds: string[] }).accountIds;
            emitEvent("relay-account-transfer-progress", { completed: 0, total: accountIds.length, phase: "preparing", currentAccountId: accountIds[0] });
            for (let index = 0; index < accountIds.length; index += 1) {
              emitEvent("relay-account-transfer-progress", { completed: index, total: accountIds.length, phase: "transferring", currentAccountId: accountIds[index] });
              await new Promise((resolve) => setTimeout(resolve, 40));
              emitEvent("relay-account-transfer-progress", { completed: index + 1, total: accountIds.length, phase: "transferring", currentAccountId: accountIds[index + 1] });
            }
            emitEvent("relay-account-transfer-progress", { completed: accountIds.length, total: accountIds.length, phase: "committing" });
            await new Promise((resolve) => setTimeout(resolve, 40));
            for (const accountId of accountIds) {
              const account = localRuntime.accounts.find((item) => item.id === accountId);
              if (!account) continue;
              account.enabled = false;
              account.inPool = false;
              account.operationalStatus = "disabled";
              account.remoteLocation = { serverId: "server-synthetic", remoteAccountId: accountId };
              const remoteAccount = { ...structuredClone(account), remoteLocation: null, enabled: true, inPool: true, operationalStatus: "rotation" as MockOperationalStatus };
              const remote = remoteRuntime.accounts.find((item) => item.id === accountId);
              if (remote) Object.assign(remote, remoteAccount);
              else remoteRuntime.accounts.push(remoteAccount);
            }
            refreshGatewayModels(localRuntime);
            refreshGatewayModels(remoteRuntime);
            emitEvent("relay-account-transfer-progress", { completed: accountIds.length, total: accountIds.length, phase: "complete" });
            return { moved: accountIds.length, remoteAccountIds: accountIds };
          }
          case "return_remote_account_to_local": {
            const localAccountId = (args.input as { localAccountId: string }).localAccountId;
            const account = localRuntime.accounts.find((item) => item.id === localAccountId);
            if (!account?.remoteLocation) throw new Error("account is not managed by a server");
            remoteRuntime.accounts = remoteRuntime.accounts.filter((item) => item.id !== account.remoteLocation?.remoteAccountId);
            account.remoteLocation = null;
            account.enabled = true;
            account.inPool = true;
            account.operationalStatus = "rotation";
            refreshGatewayModels(localRuntime);
            refreshGatewayModels(remoteRuntime);
            return { localAccountId };
          }
          case "force_activate_remote_account_locally": {
            const localAccountId = (args.input as { localAccountId: string }).localAccountId;
            const account = localRuntime.accounts.find((item) => item.id === localAccountId);
            if (!account?.remoteLocation) throw new Error("account is not managed by a server");
            account.remoteLocation = null;
            account.enabled = true;
            account.inPool = true;
            account.operationalStatus = "rotation";
            account.lastErrorCode = null;
            refreshGatewayModels(localRuntime);
            return { localAccountId };
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
          case "start_local_gateway": localRuntime.gateway.running = true; refreshGatewayModels(localRuntime); return structuredClone(localRuntime);
          case "stop_local_gateway": localRuntime.gateway.running = false; refreshGatewayModels(localRuntime); return structuredClone(localRuntime);
          case "restart_local_gateway": refreshGatewayModels(localRuntime); return structuredClone(localRuntime);
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
            refreshGatewayModels(localRuntime);
            return structuredClone(localRuntime);
          }
          case "set_local_account_proxy_required": {
            const request = args.input as { required: boolean };
            localRuntime.gateway.accountProxyRequired = request.required;
            for (const item of localRuntime.accounts) {
              if (item.proxyMode === "direct") item.proxyAvailable = !request.required;
            }
            refreshGatewayModels(localRuntime);
            return structuredClone(localRuntime);
          }
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
          case "list_codex_profile_snapshots": if (input.recoveryLoadError) throw { code: "recovery_required" }; return { snapshots: structuredClone(profileSnapshots), invalidCount: 0 };
          case "create_codex_profile_snapshot": {
            const snapshot = { id: `22222222-2222-4222-8222-${String(profileSnapshots.length + 1).padStart(12, "0")}`, name: String(args.name), profileDir, createdAtMs: Date.now(), configAvailable: true, authAvailable: true };
            profileSnapshots = [snapshot, ...profileSnapshots];
            return structuredClone(snapshot);
          }
          case "restore_codex_profile_snapshot":
          case "restore_full_codex_profile_snapshot": {
            const safetyName = typeof args.safetyName === "string" ? args.safetyName.trim() : "";
            if (safetyName) {
              const snapshot = { id: `22222222-2222-4222-8222-${String(profileSnapshots.length + 1).padStart(12, "0")}`, name: safetyName, profileDir, createdAtMs: Date.now(), configAvailable: true, authAvailable: true };
              profileSnapshots = [snapshot, ...profileSnapshots];
            }
            return null;
          }
          case "delete_codex_profile_snapshot": profileSnapshots = profileSnapshots.filter((snapshot) => snapshot.id !== String(args.snapshotId)); return null;
          case "stop_managed_codex_profile": return true;
          case "attach_codex_to_local_gateway": if (input.profileSwitchError) throw { code: "profile_restore_blocked", message: "Synthetic profile conflict" }; return { binding: { profileDir: "C:\\Users\\Test\\.codex", credentialKind: "local_gateway", credentialId: systemCredentialId, boundOauthAccountId: args.boundOauthAccountId ? String(args.boundOauthAccountId) : null, active: true } };
          case "attach_codex_to_account":
          case "launch_codex_account": return { binding: { profileDir: "C:\\Users\\Test\\.codex", credentialKind: "oauth_account", credentialId: String(args.accountId), boundOauthAccountId: null, active: true } };
          case "launch_codex_source": return { binding: { profileDir: "C:\\Users\\Test\\.codex", credentialKind: "local_gateway", credentialId: String(args.sourceId), boundOauthAccountId: null, active: true } };
          case "attach_codex_to_remote_gateway": return { binding: { profileDir: "C:\\Users\\Test\\.codex", credentialKind: "local_gateway", credentialId: "key_system", boundOauthAccountId: null, active: true } };
          case "get_relay_storage_info": return { dataPath: "C:\\Users\\Test\\AppData\\Local\\Zenith Relay\\data" };
          case "open_relay_folder": return null;
          case "reset_local_pool_data": localRuntime.sources = []; localRuntime.accounts = []; localRuntime.automations = []; localUsage = []; return null;
          case "clear_local_usage": localUsage = []; return null;
          case "export_usage": return "C:\\Temp\\usage.json";
          case "preview_support_bundle": return { bundle: { generatedAt: new Date().toISOString(), appVersion: "1.1.0", platform: "windows", mode: "local", schemaVersion: 10, gatewayRunning: true, sourceCount: 1, accountCount: 1, automationCount: 1, usageCount: localUsage.length, warningCount: 0 }, excluded: ["secrets", "prompts", "responses", "raw_identities", "raw_headers"] };
          case "export_support_bundle": return "C:\\Temp\\support.json";
          case "list_codex_account_bindings": if (input.recoveryLoadError) throw { code: "recovery_required" }; return populated && input.codexBindings !== false ? [{ profileDir, credentialKind: input.codexBindingKind ?? "local_gateway", credentialId: systemCredentialId, boundOauthAccountId: input.codexBoundOauthAccountId ?? null, active: input.codexBindingActive ?? true }] : [];
          case "connect_remote_server": return { target: { origin: remoteRuntime.runtimeTarget.origin, serverId: remoteRuntime.runtimeTarget.serverId, identityFingerprint: "synthetic-fingerprint", serverVersion: "1.1.0", protocolVersion: 2, allowInsecureHttp: false, connectedAtMs: Date.now() } };
          case "get_remote_linked_account_count": return localRuntime.accounts.filter((account) => account.remoteLocation?.serverId === "server-synthetic").length;
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
          case "plugin:app|bundle_type": return input.bundleType === null ? null : input.bundleType ?? "nsis";
          case "get_portable_update_target": return input.bundleType === null ? "windows-x86_64-portable" : null;
          case "plugin:updater|check":
            if (input.updateCheckError || (input.portableUpdateTargetMissing && args.target === "windows-x86_64-portable")) {
              throw new Error(input.updateCheckError ? "updater signature validation failed" : "portable update target is unavailable");
            }
            return input.updateVersion ? { rid: 901, currentVersion: "1.1.0", version: input.updateVersion, date: input.updateDate ?? "2026-07-15T12:00:00Z", body: input.updateBody ?? "Faster routing\nImproved settings", rawJson: {} } : null;
          case "install_portable_update":
            sendChannel(args.onEvent, { event: "Started", data: { contentLength: 100 } }, 0);
            sendChannel(args.onEvent, { event: "Progress", data: { chunkLength: 100 } }, 1);
            sendChannel(args.onEvent, { event: "Finished" }, 2);
            return null;
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
      return { sessionId, prepared: true, preview: { format: input.importDescription ? "zenith_v1" : "portable", ...(input.importDescription ? { description: input.importDescription } : {}), rows: [
        { itemId: "import_0123456789abcdef", label: "Imported account", identity: "im••••ed", authMode: "oauth", sourceName: "OpenAI", quotaStatus: "available", status: "ready", plan: "k12", defaultSelected: true, selectable: true, existing: false, warnings: [] },
        { itemId: "import_1111222233334444", label: "Second imported account", identity: "se••••nd", authMode: "oauth", sourceName: "OpenAI", quotaStatus: "available", status: "ready", plan: "k12", defaultSelected: true, selectable: true, existing: false, warnings: [] },
        { itemId: "import_fedcba9876543210", label: "Existing account", identity: "ex••••ng", authMode: "oauth", sourceName: "OpenAI", quotaStatus: "available", status: "existing", plan: "k12", defaultSelected: false, selectable: true, existing: true, warnings: [] },
      ], warnings: [] } };
    }

    function importConfirmation(sessionId: string, itemIds: string[]) {
      return {
        sessionId,
        results: itemIds.map((itemId, index) => input.importResult === "item_failure" && index === 0
          ? { itemId, status: "failed", created: false, error: { code: input.importFailureCode ?? "provider_account_id_missing", message: "secret=synthetic-access-token provider=raw-provider-id" } }
          : { itemId, status: "succeeded", created: itemId !== "import_fedcba9876543210", account: { account: { id: `account_imported_${index + 1}` } } }),
      };
    }

    function refreshGatewayModels(runtime: typeof localRuntime) {
      const previousOrder = new Map(runtime.gateway.routingOrder.map((candidate) => [candidate.candidateId, candidate]));
      const previousRank = new Map(runtime.gateway.routingOrder.map((candidate, index) => [candidate.candidateId, index]));
      for (const item of runtime.sources) {
        item.operationalStatus = !item.enabled
          ? "disabled"
          : !item.draining && item.secretAvailable ? "rotation" : "unavailable";
      }
      for (const item of runtime.accounts) {
        const quotaWait = item.quota.limitReached
          || [item.quota.primary, item.quota.secondary].some((window) => window?.availableBasisPoints === 0);
        const available = !item.draining
          && item.authState.state === "active"
          && ["unknown", "healthy", "degraded"].includes(item.health)
          && item.secretAvailable
          && item.proxyAvailable;
        item.operationalStatus = !item.enabled
          ? "disabled"
          : !available ? "unavailable" : quotaWait ? "quotaWait" : "rotation";
      }
      const currentModels = new Map(runtime.gateway.models.map((model) => [model.id.toLowerCase(), model]));
      const members = [...runtime.sources, ...runtime.accounts];
      const eligible = members.filter((member) => member.inPool && member.operationalStatus === "rotation");
      const modelMembers = members.filter((member) => member.enabled
        && member.inPool
        && !member.draining
        && member.secretAvailable
        && ("baseUrl" in member || member.proxyAvailable));
      runtime.gateway.candidateCount = eligible.length;
      runtime.gateway.routingOrder = members
        .filter((member) => member.inPool)
        .sort((left, right) => {
          if ("baseUrl" in left && "baseUrl" in right && left.priority !== right.priority) {
            return right.priority - left.priority;
          }
          return (previousRank.get(left.id) ?? Number.MAX_SAFE_INTEGER) - (previousRank.get(right.id) ?? Number.MAX_SAFE_INTEGER);
        })
        .map((member, index) => {
          const previous = previousOrder.get(member.id);
          const memberActiveModels = member.id === usageAccount.id ? activeModelCounts : [];
          const memberActiveRequestCount = memberActiveModels.reduce((count, item) => count + item.requestCount, 0);
          return {
            candidateId: member.id,
            kind: "baseUrl" in member ? "api_source" as const : "oauth_account" as const,
            available: member.operationalStatus === "rotation",
            inFlight: previous?.inFlight ?? memberActiveRequestCount,
            activeRequestCount: previous?.activeRequestCount ?? memberActiveRequestCount,
            activeModels: previous?.activeModels ?? structuredClone(memberActiveModels),
            lastUsedAtMs: previous?.lastUsedAtMs ?? (usagePresent && member.id === usageAccount.id ? Date.now() - 1_000 : null),
            nextRetryAtMs: previous?.nextRetryAtMs ?? (input.accountCooldown && member.id === account.id ? Date.now() + 30 * 60_000 : null),
            halfOpen: previous?.halfOpen ?? false,
            dispatches: previous?.dispatches ?? index,
          };
        });
      const ids = [...new Map(modelMembers.flatMap((member) => member.models).map((id) => [id.toLowerCase(), id])).values()];
      const models = ids.map((id) => {
        const current = currentModels.get(id.toLowerCase());
        const hasNativeRoute = runtime.accounts.some((account) => account.inPool && account.models.some((model) => model.toLowerCase() === id.toLowerCase()));
        const confirmedReasoning = input.modelReasoning?.[id.toLowerCase()] ?? [];
        const price = current?.customPrice
          ? { catalogRank: current.catalogRank, inputMicroUsdPerMillion: current.inputMicroUsdPerMillion, cachedInputMicroUsdPerMillion: current.cachedInputMicroUsdPerMillion, cacheWrite5mMicroUsdPerMillion: current.cacheWrite5mMicroUsdPerMillion, cacheWrite1hMicroUsdPerMillion: current.cacheWrite1hMicroUsdPerMillion, outputMicroUsdPerMillion: current.outputMicroUsdPerMillion }
          : modelPrices[id.toLowerCase()] ?? { catalogRank: null, inputMicroUsdPerMillion: null, cachedInputMicroUsdPerMillion: null, outputMicroUsdPerMillion: null };
        return {
          id,
          enabled: current?.enabled ?? true,
          memberCount: modelMembers.filter((member) => member.models.some((model) => model.toLowerCase() === id.toLowerCase())).length,
          codexVisible: current?.enabled ?? true,
          codexDisplayName: id.replaceAll("-", " "),
          ...price,
          customPrice: current?.customPrice ?? false,
          reasoningLevels: current?.reasoningLevels ?? (hasNativeRoute ? [] : confirmedReasoning),
          reasoningAllowedLevels: current?.reasoningAllowedLevels ?? [],
          reasoningConfigurable: current?.reasoningConfigurable ?? (!hasNativeRoute && confirmedReasoning.length > 0),
        };
      });
      runtime.gateway.models = input.serverModelOrder
        ? models
        : models.sort(compareModelOrder);
      runtime.gateway.visibleModelIds = runtime.gateway.models.filter((model) => model.enabled).map((model) => model.id);
    }

    function remoteAction(args: Record<string, unknown>) {
      const input = args.input as { action?: { type?: string; id?: string }; payload?: Record<string, unknown> };
      const type = input?.action?.type;
      if (type === "create_source") {
        const created = sourceFromPayload(input.payload ?? {}, `source_remote_created_${remoteRuntime.sources.length + 1}`);
        remoteRuntime.sources = [...remoteRuntime.sources, created];
        return structuredClone(created);
      }
      if (type === "update_source") {
        const target = remoteRuntime.sources.find((item) => item.id === input.action?.id);
        if (target) Object.assign(target, input.payload);
        const priorities = input.payload?.sourcePriorities as Record<string, number> | undefined;
        for (const [sourceId, priority] of Object.entries(priorities ?? {})) {
          const source = remoteRuntime.sources.find((item) => item.id === sourceId);
          if (source) source.priority = priority;
        }
        refreshGatewayModels(remoteRuntime);
        return structuredClone(target ?? null);
      }
      if (type === "preview_account_batch_import") return importSession("remote_import");
      if (type === "confirm_account_batch_import") return importConfirmation("remote_import", input.payload?.selectedItemIds as string[] ?? []);
      if (type === "update_account") {
        const target = remoteRuntime.accounts.find((item) => item.id === input.action?.id);
        if (target && typeof input.payload?.enabled === "boolean") target.enabled = input.payload.enabled;
        if (target && typeof input.payload?.draining === "boolean") target.draining = input.payload.draining;
        if (target && typeof input.payload?.priority === "number") target.priority = input.payload.priority;
        if (target && typeof input.payload?.weight === "number") target.weight = input.payload.weight;
        if (target && typeof input.payload?.purchaseCostMicroUsd === "number" && target.economics) target.economics.purchaseCostMicroUsd = input.payload.purchaseCostMicroUsd || null;
        refreshGatewayModels(remoteRuntime);
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
        refreshGatewayModels(remoteRuntime);
        return structuredClone(remoteRuntime);
      }
      if (type === "set_account_proxy_required") {
        remoteRuntime.gateway.accountProxyRequired = Boolean(input.payload?.required);
        for (const item of remoteRuntime.accounts) {
          if (item.proxyMode === "direct") item.proxyAvailable = !remoteRuntime.gateway.accountProxyRequired;
        }
        refreshGatewayModels(remoteRuntime);
        return structuredClone(remoteRuntime);
      }
      if (type === "set_account_proxy") {
        const target = remoteRuntime.accounts.find((item) => item.id === input.action?.id);
        if (target) {
          target.proxyMode = input.payload?.proxyUrl ? "account" : input.payload?.bypassCommonProxy ? "direct" : remoteRuntime.gateway.commonProxyConfigured ? "common" : "direct";
          target.proxyAvailable = target.proxyMode !== "direct" || !remoteRuntime.gateway.accountProxyRequired;
        }
        refreshGatewayModels(remoteRuntime);
        return structuredClone(target ?? null);
      }
      if (type === "assign_account_proxies") {
        const accountIds = input.payload?.accountIds as string[] ?? [];
        const proxyUrls = input.payload?.proxyUrls as string[] ?? [];
        for (const item of remoteRuntime.accounts) {
          if (!accountIds.includes(item.id)) continue;
          item.proxyMode = "account";
          item.proxyAvailable = true;
        }
        refreshGatewayModels(remoteRuntime);
        return { assigned: accountIds.length, unused: proxyUrls.length - accountIds.length };
      }
      if (type === "set_pool_membership") {
        const accountIds = input.payload?.accountIds as string[] ?? [];
        const sourceIds = input.payload?.sourceIds as string[] ?? [];
        const inPool = Boolean(input.payload?.inPool);
        for (const item of remoteRuntime.accounts) if (accountIds.includes(item.id)) item.inPool = inPool;
        for (const item of remoteRuntime.sources) if (sourceIds.includes(item.id)) item.inPool = inPool;
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
      if (type === "set_model_price") {
        const modelId = String(input.payload?.modelId ?? "");
        const target = remoteRuntime.gateway.models.find((model) => model.id === modelId);
        if (target) {
          target.inputMicroUsdPerMillion = input.payload?.inputMicroUsdPerMillion as number | null;
          target.cachedInputMicroUsdPerMillion = input.payload?.cachedInputMicroUsdPerMillion as number | null;
          target.cacheWrite5mMicroUsdPerMillion = input.payload?.cacheWrite5mMicroUsdPerMillion as number | null;
          target.cacheWrite1hMicroUsdPerMillion = input.payload?.cacheWrite1hMicroUsdPerMillion as number | null;
          target.outputMicroUsdPerMillion = input.payload?.outputMicroUsdPerMillion as number | null;
          target.customPrice = target.inputMicroUsdPerMillion != null && target.outputMicroUsdPerMillion != null;
        }
        return structuredClone(remoteRuntime);
      }
      if (type === "set_model_reasoning") {
        const modelId = String(input.payload?.modelId ?? "");
        const target = remoteRuntime.gateway.models.find((model) => model.id === modelId);
        if (target) target.reasoningAllowedLevels = [...(input.payload?.allowedLevels as string[] ?? [])];
        return structuredClone(remoteRuntime);
      }
      if (type === "set_routing_policy") {
        remoteRuntime.gateway.maxRetryCandidates = Number(input.payload?.maxRetryCandidates);
        remoteRuntime.gateway.cooldownAfterFailures = Number(input.payload?.cooldownAfterFailures);
        remoteRuntime.gateway.keepLastCandidateAvailable = Boolean(input.payload?.keepLastCandidateAvailable);
        if (input.payload?.routingStrategy) remoteRuntime.gateway.routingStrategy = input.payload.routingStrategy as "adaptive" | "quota_highest" | "subscription_expiry" | "subscription_plan";
        if (input.payload?.subscriptionPlanOrder) remoteRuntime.gateway.subscriptionPlanOrder = [...input.payload.subscriptionPlanOrder as string[]];
        if (input.payload?.defaultServiceTier) remoteRuntime.gateway.defaultServiceTier = input.payload.defaultServiceTier as "standard" | "fast";
        return structuredClone(remoteRuntime);
      }
      if (type === "refresh_all_quotas") return { refreshed: remoteRuntime.accounts.length, failed: 0, snapshot: structuredClone(remoteRuntime) };
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
      value: emitEvent,
    });
  }, options);
}
