import type { Page } from "../bun-playwright";
import type { ResolvedModelPrice } from "../../src/features/relay/api/types/pricing";
import type { ApiModelPriceOverride, ModelSummary, SourceStats, UpstreamErrorDetails, SourceProtocolBinding, SourceProtocolConfig, WakeTask, ToolPolicy, RequestTimezonePolicy, RequestTimezoneUpdate, AccountSummary } from "../../src/features/relay/api/types";
import type { CacheContextDiagnostics, CacheSessionRecord, RoutingDiagnostics, ProxyPoolEntry, RelayMode, UsageQuery, UsageTotals, UsageTransport } from "../../src/features/relay/api/types";

export type MockOptions = {
  platform?: "windows" | "macos" | "linux";
  fullscreen?: boolean;
  locale?: "en" | "ru";
  onboarding?: boolean;
  mode?: RelayMode;
  /** Raw saved preference for compatibility tests, not an executable runtime mode. */
  storedMode?: string;
  legacyOwnedAccountIds?: string[];
  theme?: "system" | "light" | "dark";
  populated?: boolean;
  runtimeState?: "ready" | "error" | "invalid";
  runtimeStateDelayMs?: number;
  automation?: Partial<WakeTask>;
  profileSnapshots?: boolean;
  claudeDesktopConfigured?: boolean;
  sourceCount?: number;
  sourceEnabled?: boolean;
  sourceRecoveryDelaySeconds?: number;
  sourceOperationalStatus?: AccountSummary["operationalStatus"];
  sourceSecretAvailable?: boolean;
  sourceDraining?: boolean;
  sourceStats?: SourceStats[];
  sourceStatsDelayMs?: number[];
  sourceStatsById?: Record<string, SourceStats>;
  accountCount?: number;
  accountLabel?: string;
  accountRouteKinds?: Array<NonNullable<AccountSummary["oauthClientKind"]>>;
  accountRoutesGrouped?: boolean;
  accountRouteSelectError?: boolean;
  oauthCompletedRouteKind?: NonNullable<AccountSummary["oauthClientKind"]>;
  accountOAuthClientKind?: AccountSummary["oauthClientKind"];
  accountPlanType?: AccountSummary["subscription"]["planType"];
  accountQuotaMissing?: boolean;
  accountCredentialDiagnostics?: AccountSummary["credentialDiagnostics"];
  accountModels?: string[];
  accountAllowedModels?: string[];
  accountExcludedModels?: string[];
  omittedPoolModels?: string[];
  accountHealth?: string;
  staleAccountError?: boolean;
  accountModelErrorCode?: string;
  accountCooldown?: boolean;
  accountModelCooldown?: boolean;
  usageAccountIndex?: number;
  nextCandidateId?: string | null;
  routingHeadroom?: Record<string, NonNullable<import("../../src/features/relay/api/types").CandidateRuntimeSnapshot["resourceHeadroom"]>>;
  usagePresent?: boolean;
  usageActive?: boolean;
  usageFailure?: boolean;
  usageTiming?: { latencyMs: number; ttftMs: number | null; generationMs: number | null };
  usageUpstreamError?: UpstreamErrorDetails;
  usageUnpricedTokens?: number;
  usageCandidateKind?: "account" | "source";
  usageRequestedModel?: string;
  usageResolvedModel?: string;
  usageUpstreamResponseModel?: string;
  usageEndpointKind?: string;
  usageCacheContext?: CacheContextDiagnostics;
  activeModelCounts?: Array<{ model: string; requestCount: number }>;
  usageToolDiagnostics?: "forwarded_text_only" | "dropped_text_only";
  usageTotalPages?: number;
  usageFilterOptionsVersion?: "full" | "advanced" | "missing";
  usageFilterHistory?: Array<{
    wireApi: NonNullable<UsageQuery["wireApi"]>;
    transport: UsageTransport;
    errorCategory: string | null;
    ageDays?: number;
    requestedModel?: string;
    resolvedModel?: string;
    accountId?: string | null;
    sourceId?: string;
  }>;
  usageSummaryTotals?: Partial<UsageTotals>;
  usageAttempts?: boolean;
  usageAttemptsFail?: boolean;
  usageAttemptOutcome?: RoutingDiagnostics["attemptOutcome"];
  usageAttemptsPartial?: boolean;
  usageRepeatedRequests?: boolean;
  cachePreview?: boolean;
  usageCache?: { cachedInputTokens?: number | null; cacheWriteInputTokens?: number | null; cacheWriteTtl?: string | null };
  cacheSessions?: CacheSessionRecord[];
  accountAuthReason?: "invalid_grant" | "reused_refresh_token" | "expired_refresh_token" | "invalidated_refresh_token";
  profileSwitchError?: boolean;
  canonicalProfilePath?: boolean;
  supplementalQuota?: boolean;
  subscriptionExpiresInMs?: number;
  exhaustedQuotaWindow?: "primary" | "secondary";
  quotaAvailable?: boolean;
  resetCreditsAvailable?: number;
  providerCredits?: number;
  providerCreditsUnlimited?: boolean;
  resetCreditsError?: string;
  resetCreditsRefreshError?: string;
  quotaRefreshStatus?: "pending" | "refreshing" | "updated" | "failed" | "requires_reauth";
  freeAccountHealthy?: boolean;
  gatewayRunning?: boolean;
  rotationVersion?: 1 | 2 | 3;
  codexBackgroundTasksEnabled?: boolean;
  codexMemoryModel?: string | null;
  codexMemoryExtractionModel?: string | null;
  codexMemoryExtractionEnabled?: boolean;
  codexMemoryConsolidationEnabled?: boolean;
  codexMemoryModelFailure?: boolean;
  codexMemoryToggleFailure?: boolean;
  codexWebsocketsEnabled?: boolean;
  codexWebsocketsFailure?: "restart";
  basisPointsAvailable?: boolean;
  basisPointsEnabled?: boolean;
  chatgptRetryUntilAvailable?: boolean;
  blockDegradedRoutesEnabled?: boolean;
  requestTimezone?: RequestTimezonePolicy;
  timezoneCatalog?: string[];
  poolMembers?: boolean;
  proxyCount?: number;
  commonProxyConfigured?: boolean;
  commonProxyId?: string | null;
  accountProxyRequired?: boolean;
  proxyCheckError?: string;
  proxyLocationError?: string;
  proxyCountryUnknown?: boolean;
  proxyCheckDelayMs?: number;
  importResult?: "success" | "item_failure" | "not_found";
  importFailureCode?: string;
  importPreviewError?: boolean;
  importPreviewDelayMs?: number;
  importPreviewDeferred?: boolean;
  importConfirmDelayMs?: number;
  sourceUpdateDelayMs?: number;
  importDescription?: string;
  staleAccountReferences?: boolean;
  oauthCallbackBeforeStartReturns?: boolean;
  oauthCompletionDeferred?: boolean;
  oauthCompletionError?: boolean;
  updateVersion?: string;
  updateBody?: string;
  updateDate?: string;
  portableUpdateTargetMissing?: boolean;
  updateCheckError?: boolean;
  updateCheckDelayMs?: number;
  bundleType?: "nsis" | "msi" | null;
  profileSwitchBackupPrompt?: boolean;
  diagnosticDebug?: boolean;
  distinctAccountIdentityHints?: boolean;
  mixedModels?: boolean;
  serverModelOrder?: string[];
  modelMetadata?: Record<string, Pick<ModelSummary, "catalogProvider" | "catalogFamily" | "catalogName">>;
  modelOrderError?: boolean;
  modelOrderDelayMs?: number;
  modelContextDelayMs?: number;
  modelContextFailOnce?: boolean;
  modelSpeed?: Record<string, "standard" | "fast" | "ultrafast">;
  sourceDetectedModelPrices?: Record<string, ApiModelPriceOverride>;
  sourceResolvedModelPrices?: Record<string, ResolvedModelPrice>;
  sourceErrorCode?: string;
  sourceRefreshError?: boolean;
  sourceCreateError?: string;
  sourceFollowupError?: "state" | "membership";
  catalogRefreshWarning?: "failed" | "deferred";
  modelReasoning?: Record<string, string[]>;
  modelProtocolRoutes?: Record<string, ModelSummary["protocolRoutes"]>;
  modelContext?: Record<string, {
    configurable: boolean;
    policy?: { mode: "short" } | { mode: "extended" } | { mode: "custom"; tokens: number };
    contextLimit?: number;
    windows?: ModelSummary["contextWindows"];
    inputLimit?: number;
    outputLimit?: number;
  }>;
  manualReasoningFallbackModels?: string[];
  sourceProtocolBindings?: SourceProtocolBinding[];
  sourceResolvedProtocolBindings?: SourceProtocolBinding[];
  sourceProtocolConfig?: SourceProtocolConfig;
};

export async function emitTauriEvent(page: Page, event: string, payload: unknown) {
  await page.evaluate(([name, value]) => {
    (window as unknown as { __TAURI_TEST_EMIT__: (event: string, payload: unknown) => void }).__TAURI_TEST_EMIT__(name, value);
  }, [event, payload] as const);
}

export async function installTauriMock(page: Page, options: MockOptions = {}) {
  await page.addInitScript((input) => {
    const modelContextDelayMs = input.modelContextDelayMs;
    let contextSaveShouldFail = input.modelContextFailOnce;
    const assertContextSave = () => {
      if (!contextSaveShouldFail) return;
      contextSaveShouldFail = false;
      throw { code: "gateway_unavailable", message: "Synthetic context save failure" };
    };
    const locale = input.locale ?? "en";
    const rotationVersion = input.rotationVersion ?? 3;
    const timezoneCatalog = input.timezoneCatalog ?? ["Etc/GMT+8", "Etc/GMT-3", "America/New_York", "UTC"];
    const populated = input.populated ?? true;
    const usagePresent = populated && input.usagePresent !== false;
    let sourceStatsIndex = 0;
    let sourceFollowupPending = false;
    const readRuntimeState = async (snapshot: unknown) => {
      if (input.runtimeStateDelayMs) await new Promise((resolve) => setTimeout(resolve, input.runtimeStateDelayMs));
      if (input.runtimeState === "error") throw { code: "invalid_state", message: "Synthetic startup read failure" };
      if (input.runtimeState === "invalid") return { ...(snapshot as object), accounts: null };
      return structuredClone(snapshot);
    };
    const distinctAccountIdentityHints = input.distinctAccountIdentityHints ?? false;
    const dayMs = 24 * 60 * 60_000;
    localStorage.setItem("relay.onboarding", input.onboarding === false ? "0" : "1");
    localStorage.setItem("relay.mode", input.storedMode ?? input.mode ?? "local");
    localStorage.setItem("relay.theme", input.theme ?? "light");
    // Test fixtures seed the preference once.  A renderer reload must use the
    // persisted WebView value just like the packaged desktop application.
    if (localStorage.getItem("relay.profileSwitchBackupPrompt") === null) {
      localStorage.setItem("relay.profileSwitchBackupPrompt", input.profileSwitchBackupPrompt ? "1" : "0");
    }

    type MockQuotaWindow = { kind: "primary" | "secondary"; availableBasisPoints: number; explicitlyFull: boolean; resetAtMs: number; windowStartMs?: number; windowMinutes: number; observedAtMs: number };
    type MockOperationalStatus = "rotation" | "quotaWait" | "unavailable" | "disabled";
    type MockAuthState = { state: string; reason?: string };
    const exhaustedQuotaWindow = input.quotaAvailable ? null : input.exhaustedQuotaWindow ?? "primary";
    const quotaNowMs = Date.now();
    const primaryResetAtMs = quotaNowMs + 90 * 60_000;
    const secondaryResetAtMs = quotaNowMs + 3 * 24 * 60 * 60_000;
    const quota: { primary: MockQuotaWindow | null; secondary: MockQuotaWindow | null; supplemental: Array<{ id: string; label: string; serviceTier?: "standard" | "fast" | "ultrafast" | null; window: MockQuotaWindow }>; limitReached: boolean; resetCreditsAvailable: number | null; availableCreditsMicroUnits: number | null; providerCreditsAvailable: boolean; providerCreditsUnlimited: boolean; updatedAtMs: number | null; error: null } = {
      primary: { kind: "primary", availableBasisPoints: exhaustedQuotaWindow === "primary" ? 0 : 7200, explicitlyFull: false, resetAtMs: primaryResetAtMs, windowStartMs: primaryResetAtMs - 300 * 60_000, windowMinutes: 300, observedAtMs: quotaNowMs },
      secondary: { kind: "secondary", availableBasisPoints: exhaustedQuotaWindow === "secondary" ? 0 : 6400, explicitlyFull: false, resetAtMs: secondaryResetAtMs, windowStartMs: secondaryResetAtMs - 10_080 * 60_000, windowMinutes: 10_080, observedAtMs: quotaNowMs },
      supplemental: input.supplementalQuota ? [
        { id: "code_review:primary", label: "Code Review", window: { kind: "primary", availableBasisPoints: 7200, explicitlyFull: false, resetAtMs: Date.now() + 2 * 60 * 60_000, windowMinutes: 300, observedAtMs: Date.now() } },
        { id: "code_review:secondary", label: "Code Review", window: { kind: "secondary", availableBasisPoints: 8600, explicitlyFull: false, resetAtMs: Date.now() + 5 * 24 * 60 * 60_000, windowMinutes: 10_080, observedAtMs: Date.now() } },
        { id: "additional:0:primary", label: "GPT-5.4 priority", serviceTier: "fast", window: { kind: "primary", availableBasisPoints: 4100, explicitlyFull: false, resetAtMs: Date.now() + 12 * 60 * 60_000, windowMinutes: 1_440, observedAtMs: Date.now() } },
      ] : [],
      limitReached: false,
      resetCreditsAvailable: Math.max(0, input.resetCreditsAvailable ?? 1),
      availableCreditsMicroUnits: input.providerCredits == null ? null : Math.round(input.providerCredits * 1_000_000),
      providerCreditsAvailable: (input.providerCredits ?? 0) > 0,
      providerCreditsUnlimited: input.providerCreditsUnlimited ?? false,
      updatedAtMs: Date.now(),
      error: null as { code: string; occurredAtMs: number } | null,
    };
    if (input.accountQuotaMissing) {
      quota.primary = null;
      quota.secondary = null;
      quota.supplemental = [];
      quota.resetCreditsAvailable = null;
      quota.updatedAtMs = null;
    }
    const sourceModels = input.serverModelOrder ?? (input.mixedModels
      ? ["gpt-5.4", "claude-opus-4-8", "gemini-3.1-pro-preview", "grok-4.5", "glm-5.2", "private-model"]
      : ["gpt-5.4", "gpt-5.4-mini"]);
    const automaticBindings = (models: string[], upstream: "responses" | "chat_completions" | "messages" | "gemini") =>
      (["responses", "chat_completions", "messages", "gemini"] as const).map((client) => ({
        wireApi: client,
        adapter: client === upstream ? "native" : `${client}_to_${upstream}`,
        reasoningMode: client === upstream ? "disabled" : "adaptive",
        modelIds: [...models],
      })) as SourceProtocolBinding[];
    const configuredSourceBindings = input.sourceProtocolBindings ?? [{
      wireApi: "responses" as const,
      adapter: "native" as const,
      reasoningMode: "disabled" as const,
      modelIds: [...sourceModels],
    }];
    const configuredRoutesByKey = new Map<string, SourceProtocolBinding>();
    for (const model of sourceModels) {
      const binding = configuredSourceBindings.find((entry) => entry.modelIds.includes(model))
        ?? configuredSourceBindings[0];
      const upstream = !binding || (binding.adapter ?? "native") === "native"
        ? binding?.wireApi ?? "responses"
        : (["responses", "chat_completions", "messages", "gemini"] as const).find((protocol) =>
          binding.adapter?.endsWith(`_to_${protocol}`)) ?? "responses";
      for (const route of automaticBindings([model], upstream)) {
        const key = `${route.wireApi}:${route.adapter}`;
        const existing = configuredRoutesByKey.get(key);
        if (existing) existing.modelIds.push(model);
        else configuredRoutesByKey.set(key, route);
      }
    }
    const source = {
      id: "source_synthetic",
      name: "Example compatible API",
      enabled: input.sourceEnabled ?? true,
      inPool: input.poolMembers ?? true,
      draining: input.sourceDraining ?? false,
      operationalStatus: input.sourceOperationalStatus ?? (input.sourceEnabled === false ? "disabled" : "rotation") as MockOperationalStatus,
      baseUrl: "https://api.zenithmarket.dev/v1",
      wireApi: "responses" as const,
      protocolConfig: input.sourceProtocolConfig ?? { revision: 0, capabilities: [] } as SourceProtocolConfig,
      resolvedProtocolBindings: input.sourceResolvedProtocolBindings ?? [...configuredRoutesByKey.values()],
      protocolBindings: configuredSourceBindings,
      models: sourceModels,
      allowedModels: [] as string[],
      excludedModels: [] as string[],
      priority: 10,
      weight: 100,
      recoveryDelaySeconds: input.sourceRecoveryDelaySeconds ?? 0,
      modelPriceOverrides: {},
      detectedModelPrices: input.sourceDetectedModelPrices ?? {},
      resolvedModelPrices: input.sourceResolvedModelPrices,
      apiEquivalent: { microUsd: 8_500, pricedTokens: 1_400, unpricedTokens: 0 },
      secretAvailable: input.sourceSecretAvailable ?? true,
      lastErrorCode: input.sourceErrorCode ?? null,
      refreshRevision: 1,
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
      oauthClientKind: input.accountOAuthClientKind ?? "codex",
      credentialDiagnostics: input.accountCredentialDiagnostics,
      id: "account_synthetic",
      label: input.accountLabel ?? "Personal Plus",
      identityHint: distinctAccountIdentityHints ? "p***@example.test" : input.accountLabel ?? "Personal Plus",
      enabled: true,
      inPool: input.poolMembers ?? true,
      draining: false,
      operationalStatus: "rotation" as MockOperationalStatus,
      authState: (input.accountAuthReason ? { state: "requires_reauth", reason: input.accountAuthReason } : { state: "active" }) as MockAuthState,
      health: input.staleAccountError ? "degraded" : input.accountHealth ?? "healthy" as string,
      models: input.accountModels ?? ["gpt-5.4", "gpt-5.4-mini"],
      allowedModels: input.accountAllowedModels ?? [],
      excludedModels: input.accountExcludedModels ?? [],
      priority: 20,
      weight: 100,
      apiEquivalent: { microUsd: 14_100_000, pricedTokens: 2_800_000, unpricedTokens: 0 },
      quotaWindowUsage: {
        kind: "secondary" as const,
        windowStartMs: secondaryResetAtMs - 10_080 * 60_000,
        observedAtMs: quotaNowMs,
        windowMinutes: 10_080,
        apiEquivalent: { microUsd: 5_000_000, pricedTokens: 1_000_000, unpricedTokens: 0 },
      },
      purchaseCostMicroUsd: 18_000_000 as number | null,
      subscription: { planType: input.accountPlanType !== undefined ? input.accountPlanType : input.supplementalQuota ? "pro" : "plus", activeUntilMs: input.accountPlanType === null ? null : Date.now() + (input.subscriptionExpiresInMs ?? 37 * dayMs), status: "active", updatedAtMs: Date.now() },
      quota,
      quotaRefreshStatus: input.quotaRefreshStatus ?? "updated" as "pending" | "refreshing" | "updated" | "failed" | "requires_reauth",
      refreshState: { models: "fresh", quota: "fresh" } as NonNullable<AccountSummary["refreshState"]>,
      secretAvailable: true,
      basisPointsAvailable: input.basisPointsAvailable ?? false,
      basisPointsEnabled: input.basisPointsEnabled ?? false,
      recoveryRequired: false,
      proxyMode: "common",
      proxyAvailable: true,
      lastErrorCode: input.accountModelErrorCode
        ?? (input.staleAccountError ? "quota_transport" : input.accountAuthReason ? "quota_token_prepare" : null as string | null),
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
    const accounts: AccountSummary[] = Array.from({ length: accountCount }, (_, index) => {
      if (index === 0) return account;
      const variant = accountVariants[index % accountVariants.length];
      const item = structuredClone(account);
      item.id = `account_synthetic_${index + 1}`;
      item.label = index === 0 ? account.label : variant.label;
      item.identityHint = distinctAccountIdentityHints ? ["p***@example.test", "b***@example.test", "r***@example.test", "q***@example.test", "s***@example.test", "t***@example.test"][index % 6] : item.label;
      item.authState = { state: "active" };
      item.subscription.planType = variant.plan;
      item.subscription.activeUntilMs = variant.activeUntilMs;
      item.proxyMode = variant.proxyMode;
      item.models = [...variant.models];
      item.priority = variant.priority;
      const accountValue = [
        null,
        { used: 31_500_000, purchase: 24_000_000 },
        { used: 3_800_000, purchase: 8_000_000 },
        { used: 65_000_000, purchase: 70_000_000 },
        { used: 500_000, purchase: 12_000_000 },
        { used: 4_200_000, purchase: null },
      ][index % 6];
      if (accountValue) {
        item.apiEquivalent.microUsd = accountValue.used;
        item.apiEquivalent.pricedTokens = 2_800_000 * (index + 1);
        item.purchaseCostMicroUsd = accountValue.purchase;
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
      item.refreshState.quota = item.quota.error || item.quota.updatedAtMs == null ? "stale" : "fresh";
      return item;
    });
      const systemCredentialId = "key_system";
      const profileDir = input.canonicalProfilePath ? "\\\\?\\C:\\Users\\Test\\.codex" : "C:\\Users\\Test\\.codex";
    type MockModelSummary = { id: string; enabled: boolean; memberCount: number; codexVisible: boolean; codexDisplayName: string; catalogProvider?: string | null; catalogFamily?: string | null; catalogName?: string | null; catalogReleaseDate?: string | null; catalogLastUpdated?: string | null; catalogStatus?: string | null; catalogContextLimit?: number | null; contextPolicy?: { mode: "short" } | { mode: "extended" } | { mode: "custom"; tokens: number }; contextConfigurable?: boolean; catalogInputLimit?: number | null; catalogOutputLimit?: number | null; inputMicroUsdPerMillion: number | null; cachedInputMicroUsdPerMillion: number | null; cacheWrite5mMicroUsdPerMillion?: number | null; cacheWrite1hMicroUsdPerMillion?: number | null; outputMicroUsdPerMillion: number | null; imageRequestPrices: Array<{ operation: "generation" | "edit"; quality: string; size: string; microUsd: number }>; customPrice: boolean; reasoningLevels: string[]; reasoningSupportedLevels: string[]; reasoningAllowedLevels: string[]; reasoningConfigurable: boolean; reasoningManualFallback?: boolean; speedTiers?: Array<"standard" | "fast" | "ultrafast">; speedSupported?: boolean; speedTier?: "standard" | "fast" | "ultrafast"; speedConfigurable?: boolean };
    type MockCandidateRuntime = { candidateId: string; kind: "api_source" | "oauth_account"; available: boolean; nextForNewRequest?: boolean; resourceHeadroom?: import("../../src/features/relay/api/types").CandidateRuntimeSnapshot["resourceHeadroom"]; activityRevision?: number; runtimeId?: number; inFlight: number; activeRequestCount: number; activeModels: Array<{ model: string; requestCount: number }>; modelRetries?: Array<{ model: string; retryAtMs: number }>; lastUsedAtMs: number | null; nextRetryAtMs: number | null; halfOpen: boolean; dispatches: number };
    const modelPrices: Record<string, Pick<MockModelSummary, "inputMicroUsdPerMillion" | "cachedInputMicroUsdPerMillion" | "cacheWrite5mMicroUsdPerMillion" | "cacheWrite1hMicroUsdPerMillion" | "outputMicroUsdPerMillion">> = {
      "gpt-5.4": { inputMicroUsdPerMillion: 2_500_000, cachedInputMicroUsdPerMillion: 250_000, outputMicroUsdPerMillion: 15_000_000 },
      "gpt-5.4-mini": { inputMicroUsdPerMillion: 750_000, cachedInputMicroUsdPerMillion: 75_000, outputMicroUsdPerMillion: 4_500_000 },
      "claude-opus-4-8": { inputMicroUsdPerMillion: 1_400_000, cachedInputMicroUsdPerMillion: 140_000, cacheWrite5mMicroUsdPerMillion: 1_750_000, cacheWrite1hMicroUsdPerMillion: 2_800_000, outputMicroUsdPerMillion: 7_000_000 },
    };
    const modelMetadata: Record<string, Pick<MockModelSummary, "catalogProvider" | "catalogFamily" | "catalogName">> = {
      "gpt-5.4": { catalogProvider: "openai", catalogFamily: "gpt", catalogName: "GPT-5.4" },
      "gpt-5.4-mini": { catalogProvider: "openai", catalogFamily: "gpt", catalogName: "GPT-5.4 mini" },
      "claude-opus-4-8": { catalogProvider: "anthropic", catalogFamily: "claude-opus", catalogName: "Claude Opus 4.8" },
      "gemini-3.1-pro-preview": { catalogProvider: "google", catalogFamily: "gemini-pro", catalogName: "Gemini 3.1 Pro Preview" },
      "gemini-3.6-flash-high": { catalogProvider: "google", catalogFamily: "gemini-flash", catalogName: "Gemini 3.6 Flash High" },
      "gemini-3.6-flash-medium": { catalogProvider: "google", catalogFamily: "gemini-flash", catalogName: "Gemini 3.6 Flash Medium" },
      "gemini-3.6-flash-low": { catalogProvider: "google", catalogFamily: "gemini-flash", catalogName: "Gemini 3.6 Flash Low" },
      "grok-4.5": { catalogProvider: "xai", catalogFamily: "grok", catalogName: "Grok 4.5" },
      "glm-5.2": { catalogProvider: "zai", catalogFamily: "glm", catalogName: "GLM 5.2" },
      ...input.modelMetadata,
    };
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
      ...input.automation,
    };
    const defaultToolPolicy: ToolPolicy = { mode: "pass_through" };


    function saveRequestTimezone(current: RequestTimezonePolicy | undefined, update: RequestTimezoneUpdate): RequestTimezonePolicy {
      const normalize = (policy: Partial<RequestTimezonePolicy> | undefined) => JSON.stringify({ enabled: policy?.enabled ?? false, accountTimezones: policy?.accountTimezones ?? {}, accountTimezoneBindings: policy?.accountTimezoneBindings ?? {}, accountTimezoneSources: policy?.accountTimezoneSources ?? {} });
      if (normalize(current) !== normalize(update.expectedPolicy)) {
        throw { code: "configuration_revision_stale", message: "request timezone changed" };
      }
      return structuredClone(update.policy);
    }
    if (input.accountRouteKinds) {
      accounts.splice(0, accounts.length, ...input.accountRouteKinds.map((kind, index) => {
        const connection = structuredClone(account);
        connection.id = `account_synthetic_${index + 1}`;
        connection.oauthClientKind = kind;
        connection.label = "Synthetic shared account";
        if (kind === "siwc") {
          connection.subscription = { planType: null, activeUntilMs: null, status: "unknown", updatedAtMs: null };
          connection.quota.primary = null;
          connection.quota.secondary = null;
          connection.quota.supplemental = [];
          connection.quota.updatedAtMs = null;
          connection.quota.resetCreditsAvailable = null;
          connection.quota.availableCreditsMicroUnits = null;
          connection.quota.providerCreditsAvailable = false;
          connection.quota.providerCreditsUnlimited = false;
        }
        return Object.assign(connection, input.accountRoutesGrouped ? {
          routeGroupId: "account_synthetic_1", routeInactive: index !== 0, inPool: index === 0,
        } : {});
      }));
    }
    const localRuntime = {
      schemaVersion: 14,
      gateway: { toolPolicy: defaultToolPolicy, requestTimezone: input.requestTimezone ?? { enabled: false, accountTimezones: {}, accountTimezoneBindings: {} } as RequestTimezonePolicy, modelCatalog: modelMetadata, poolRouting: { version: rotationVersion, mode: rotationVersion === 1 ? "smart" : "automatic", members: [] } as import("../../src/features/relay/api/types").PoolRoutingSnapshot, running: input.gatewayRunning ?? true, baseUrl: "http://127.0.0.1:14998/v1", candidateCount: 0, visibleModelIds: [] as string[], maxRetryCandidates: 3, defaultServiceTier: "standard" as "standard" | "fast" | "ultrafast", models: [] as MockModelSummary[], commonProxyConfigured: input.commonProxyConfigured ?? true, commonProxyAvailable: input.commonProxyConfigured ?? true, commonProxyId: input.commonProxyId ?? null, accountProxyRequired: false, quotaRequestTimeoutSeconds: 20, chatgptInterfaceQuotaReserveBasisPoints: 100, codexBackgroundTasksEnabled: input.codexBackgroundTasksEnabled ?? true, codexMemoryExtractionEnabled: input.codexMemoryExtractionEnabled ?? true, codexMemoryConsolidationEnabled: input.codexMemoryConsolidationEnabled ?? true, codexWebsocketsEnabled: input.codexWebsocketsEnabled ?? true, chatgptRetryUntilAvailable: input.chatgptRetryUntilAvailable ?? false, blockDegradedRoutesEnabled: input.blockDegradedRoutesEnabled ?? true, routingOrder: [] as MockCandidateRuntime[] },
      platform: input.platform ?? "windows",
      capabilities: { features: ["sources", "oauth_accounts", "quota_wake", "profiles", "account_proxies", "account_export", "account_identity_reveal", "runtime_routing", rotationVersion === 3 ? "rotation_v3" : "rotation_v2", "source_protocols_v1"] },
      sources: populated ? sources : [],
      accounts: populated ? accounts : [],
      automations: populated ? [automation] : [],
      wakeHistory: populated ? [{ taskId: automation.id, accountId: account.id, windowKind: "primary", modelId: "gpt-5.4-mini", outcome: "confirmed", startedAtMs: Date.now() - 120_000, completedAtMs: Date.now() - 118_000, errorCode: null }] : [],
      warnings: input.catalogRefreshWarning === "failed"
        ? ["model_catalog_refresh_failed:gateway_unavailable"]
        : input.catalogRefreshWarning === "deferred"
          ? ["model_catalog_refresh_deferred:codex_running"]
          : [],
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
        nextForNewRequest: false,
        resourceHeadroom: input.routingHeadroom?.[item.id],
        activityRevision: 0,
        inFlight: item.id === usageAccount.id ? activeRequestCount : 0,
        activeRequestCount: item.id === usageAccount.id ? activeRequestCount : 0,
        activeModels: item.id === usageAccount.id ? structuredClone(activeModelCounts) : [],
        modelRetries: input.accountModelCooldown && item.id === account.id ? [{ model: "gpt-5.4", retryAtMs: Date.now() + 30 * 60_000 }] : [],
        lastUsedAtMs: usagePresent && item.id === usageAccount.id ? Date.now() - 1_000 : null,
        nextRetryAtMs: input.accountCooldown && item.id === account.id ? Date.now() + 30 * 60_000 : null,
        halfOpen: false,
        dispatches: index,
      }));
    for (const account of localRuntime.accounts) {
      if (!input.legacyOwnedAccountIds?.includes(account.id)) continue;
      account.recoveryRequired = true;
      account.enabled = false;
      account.inPool = false;
      account.operationalStatus = "disabled";
    }
    localRuntime.gateway.poolRouting.members = orderedMembers.filter((item) => populated && item.inPool).map((item) => ({ kind: "baseUrl" in item ? "source" : "account", id: item.id, weight: item.weight, maxConcurrency: 0 }));
    refreshGatewayModels(localRuntime);
    const defaultModelOrder = localRuntime.gateway.models.map((model) => model.id);
    async function prepareModelOrder() {
      if (input.modelOrderDelayMs) await new Promise((resolve) => setTimeout(resolve, input.modelOrderDelayMs));
      if (input.modelOrderError) throw { code: "io", message: "Could not save model order" };
    }
    localRuntime.gateway.accountProxyRequired = input.accountProxyRequired ?? false;
    const localAccountTrash: Array<{ record: AccountSummary; trashedAtMs: number }> = [];
    let localGatewayApiKey = "zlr_synthetic_local_gateway_key";
    let networkEnabled = false;
    let networkGatewayApiKey = "zlr_synthetic_network_key";
    const configurationPreset = {
      format: "zenith-relay-configuration",
      schemaVersion: 2,
      settings: {
        sources: localRuntime.sources.map((item) => ({ id: item.id, name: item.name, baseUrl: item.baseUrl, wireApi: item.wireApi, protocolBindings: item.protocolBindings, enabled: item.enabled, inPool: item.inPool, allowedModels: item.allowedModels, excludedModels: item.excludedModels, priority: item.priority, weight: item.weight, recoveryDelaySeconds: item.recoveryDelaySeconds, modelPriceOverrides: item.modelPriceOverrides })),
        accounts: localRuntime.accounts.map((item) => ({ id: item.id, identityHint: item.identityHint, enabled: item.enabled, inPool: item.inPool, allowedModels: item.allowedModels, excludedModels: item.excludedModels, priority: item.priority, weight: item.weight, proxyId: null })),
        routing: { poolRouting: structuredClone(localRuntime.gateway.poolRouting), maxRetryCandidates: 4, defaultServiceTier: localRuntime.gateway.defaultServiceTier, imageBaseModel: null },
        quota: { requestTimeoutSeconds: localRuntime.gateway.quotaRequestTimeoutSeconds, accountProxyRequired: localRuntime.gateway.accountProxyRequired, commonProxyId: null },
        hiddenModels: [],
        modelPriceOverrides: {},
      },
    };
    const assignedProxyAccount = localRuntime.accounts.find((item) => item.proxyMode === "account");
    let proxyEntries: ProxyPoolEntry[] = Array.from({ length: input.proxyCount ?? (populated ? 3 : 0) }, (_, index) => ({
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
          adapter: binding.adapter ?? "native",
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
        protocolConfig: { revision: 0, capabilities: [] },
        resolvedProtocolBindings: automaticBindings(models, wireApi),
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
    const routing = { reason: sourceUsage ? "weighted_rotation" : "quota_headroom", eligibleCandidates: 4, quotaRemainingBasisPoints: sourceUsage ? null : 6300, inFlightBefore: 0, dispatchesBefore: 3, endpointKind: input.usageEndpointKind ?? null, cacheContext: input.usageCacheContext, upstreamResponseModel: input.usageUpstreamResponseModel,
      attemptOutcome: input.usageAttemptOutcome !== undefined ? input.usageAttemptOutcome : input.usageAttempts ? input.usageFailure ? "returned_failure" : "completed" : undefined };
    const localUnpricedTokens = Math.min(28, Math.max(0, input.usageUnpricedTokens ?? 0));
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
    let localUsage = usagePresent ? [{
      id: 1, createdAt: new Date(Date.now() - (input.cachePreview ? 12 * 60_000 : 0)).toISOString(),
      requestId: "req_synthetic_local", attempt: 1, sourceId: source.id,
      accountId: sourceUsage ? null : input.staleAccountReferences ? "account_deleted_internal" : usageAccount.id,
      clientContextId: input.cachePreview ? "client_a1b2c3d4e5f6" : null,
      requestedModel: input.cachePreview ? "gpt-6-astra" : requestedUsageModel,
      resolvedModel: input.cachePreview ? "gpt-6-astra" : resolvedUsageModel,
      requestedReasoningEffort: "max", effectiveReasoningEffort: "low", wireApi: "responses", transport: "http", serviceTier: "standard",
      success: !input.usageFailure, httpStatus: input.usageFailure ? 502 : 200,
      errorCategory: input.usageFailure ? "upstream_failure" : null, upstreamError: input.usageUpstreamError,
      latencyMs: 428, ttftMs: 128, generationMs: 300, inputTokens: input.cachePreview ? 74_905 : 20,
      cachedInputTokens: input.cachePreview ? 73_600 : 12, cacheWriteInputTokens: input.cachePreview ? null : 4,
      reasoningTokens: input.cachePreview ? 275 : 5, outputTokens: input.cachePreview ? 447 : 8, totalTokens: input.cachePreview ? 75_352 : 28,
      apiEquivalent: { microUsd: 148, pricedTokens: 28 - localUnpricedTokens, unpricedTokens: localUnpricedTokens },
      toolUse, routing, ...input.usageTiming, ...input.usageCache,
    }] : [];
    if (input.usageAttempts) {
      localUsage = localUsage.flatMap((event) => (input.usageAttemptsPartial ? [3] : [3, 2, 1]).map((attempt) => ({
        ...event, id: -attempt, attempt, success: attempt === 3 && !input.usageFailure,
        httpStatus: attempt === 3 ? input.usageFailure ? 502 : 200 : attempt === 2 ? 503 : 200,
        errorCategory: attempt === 1 || (attempt === 3 && !input.usageFailure) ? null : "upstream_failure",
        inputTokens: attempt * 20, outputTokens: attempt * 8, totalTokens: attempt * 28,
        resolvedModel: attempt === 1 ? "lookup-model" : event.resolvedModel,
        apiEquivalent: { microUsd: attempt * 148, pricedTokens: attempt * 28, unpricedTokens: 0 },
        routing: { ...event.routing, attemptOutcome: attempt === 3 ? routing.attemptOutcome : attempt === 2 ? "retry_selection" : "tool_discovery" },
      })));
    }
    if (input.usageRepeatedRequests) {
      localUsage = localUsage.flatMap((event) => [4, 3, 2, 1].map((number) => ({ ...event, id: number,
        requestId: `relay-synthetic-common-prefix-${number}1111111-1`, attempt: 1 })));
    }
    if (input.usageFilterHistory && localUsage[0]) {
      const base = localUsage[0];
      localUsage = input.usageFilterHistory.map((record, index) => ({
        ...base, id: index + 1, requestId: `req_filter_${index + 1}`,
        createdAt: new Date(Date.now() - (record.ageDays ?? 0) * 86_400_000).toISOString(),
        wireApi: record.wireApi, transport: record.transport, errorCategory: record.errorCategory,
        requestedModel: record.requestedModel ?? base.requestedModel,
        resolvedModel: record.resolvedModel ?? base.resolvedModel,
        accountId: record.accountId === undefined ? base.accountId : record.accountId,
        sourceId: record.sourceId ?? base.sourceId,
        success: record.errorCategory === null, httpStatus: record.errorCategory === null ? 200 : 502,
      }));
    }
    function syntheticAttemptPage<T extends { requestId: string; success: boolean; routing: object }>(
      records: T[], query: { requestId?: string; page?: number },
    ) {
      const base = records.find((record) => record.requestId === query.requestId);
      const events = base ? (input.usageAttemptsPartial ? [3] : [3, 2, 1]).map((attempt) => ({
        ...base, id: -attempt, attempt, success: attempt === 3 && !input.usageFailure,
        httpStatus: attempt === 3 ? input.usageFailure ? 502 : 200 : attempt === 2 ? 503 : 400,
        errorCategory: attempt === 3 && !input.usageFailure ? null : "upstream_failure",
        routing: { ...base.routing, attemptOutcome: attempt === 3 ? routing.attemptOutcome : attempt === 2 ? "retry_selection" : "retry_repair" },
      })) : [];
      const page = query.page ?? 1;
      return {
        events: structuredClone(events.slice((page - 1) * 2, page * 2)),
        total: events.length, page, pageSize: 2, totalPages: Math.ceil(events.length / 2),
      };
    }
    function usageTotals(events: Array<{ requestId?: string; attempt?: number; success: boolean; latencyMs: number; ttftMs?: number | null; generationMs?: number | null; inputTokens: number | null; cachedInputTokens: number | null; cacheWriteInputTokens?: number | null; reasoningTokens: number | null; outputTokens: number | null; totalTokens: number | null; apiEquivalent?: { microUsd: number; pricedTokens: number; unpricedTokens: number } }>) {
      return events.reduce((totals, item) => {
        const final = !item.requestId || !localUsage.some((other) => other.requestId === item.requestId && other.attempt > (item.attempt ?? 1));
        const outputTokens = item.success ? Math.max(0, item.outputTokens ?? 0) : 0;
        const reasoningTokens = Math.min(outputTokens, Math.max(0, item.reasoningTokens ?? 0));
        const measuredOutputTokens = Math.max(0, outputTokens - reasoningTokens - 1);
        if (final) {
        totals.requests += 1; totals.successfulRequests += Number(item.success); totals.latencyMs += item.latencyMs;
        if (item.ttftMs != null) { totals.ttftMs += item.ttftMs; totals.ttftSamples += 1; }
        if (item.success && item.generationMs != null && item.generationMs > 0 && measuredOutputTokens > 0 && measuredOutputTokens <= item.generationMs) { totals.generationMs += item.generationMs; totals.generationSamples += 1; totals.generationOutputTokens += measuredOutputTokens; }
        }
        totals.inputTokens += item.inputTokens ?? 0; totals.cachedInputTokens += item.cachedInputTokens ?? 0; totals.cachedInputSamples += Number(item.cachedInputTokens != null); totals.cacheWriteInputTokens += item.cacheWriteInputTokens ?? 0; totals.cacheWriteInputSamples += Number(item.cacheWriteInputTokens != null);
        totals.reasoningTokens += item.reasoningTokens ?? 0; totals.outputTokens += item.outputTokens ?? 0; totals.totalTokens += item.totalTokens ?? 0;
        if (outputTokens && item.latencyMs && outputTokens <= item.latencyMs) { totals.speedOutputTokens += outputTokens; totals.speedDurationMs += item.latencyMs; }
        totals.apiEquivalent.microUsd += item.apiEquivalent?.microUsd ?? 0; totals.apiEquivalent.pricedTokens += item.apiEquivalent?.pricedTokens ?? 0; totals.apiEquivalent.unpricedTokens += item.apiEquivalent?.unpricedTokens ?? item.totalTokens ?? 0;
        return totals;
      }, { requests: 0, successfulRequests: 0, latencyMs: 0, ttftMs: 0, ttftSamples: 0, generationMs: 0, generationSamples: 0, generationOutputTokens: 0, inputTokens: 0, cachedInputTokens: 0, cachedInputSamples: 0, cacheWriteInputTokens: 0, cacheWriteInputSamples: 0, reasoningTokens: 0, outputTokens: 0, totalTokens: 0, speedOutputTokens: 0, speedDurationMs: 0, apiEquivalent: { microUsd: 0, pricedTokens: 0, unpricedTokens: 0 } });
    }
    let diagnosticDebugEnabled = input.diagnosticDebug ?? false;
    const invocations: Array<{ command: string; args: Record<string, unknown> }> = [];
    const callbacks = new Map<number, (...args: unknown[]) => unknown>();
    let nextCallback = 1;
    let fullscreen = input.fullscreen ?? false;
    const eventListeners = new Map<number, { event: string; handler: number }>();
    let nextEventListener = 1;
    const emitEvent = (event: string, payload: unknown) => {
      if (event === "tauri://resize" && typeof (payload as { fullscreen?: unknown })?.fullscreen === "boolean") {
        fullscreen = (payload as { fullscreen: boolean }).fullscreen;
      }
      for (const [id, listener] of eventListeners) {
        if (listener.event === event) callbacks.get(listener.handler)?.({ event, id, payload });
      }
    };
    const sendChannel = (channel: unknown, message: unknown, index = 0) => {
      const id = Number((channel as { id?: number } | null)?.id);
      if (Number.isFinite(id)) callbacks.get(id)?.({ index, message });
    };

    let claudeDesktopConfigured = input.claudeDesktopConfigured ?? false;
    let claudeDesktopHasRecovery = claudeDesktopConfigured;
    let claudeDesktopSnapshotName: string | null = null;
    let claudeDesktopSnapshotCreatedAtMs: number | null = claudeDesktopConfigured ? 1_781_000_000_000 : null;
    const applicationLaunchPaths: Record<string, string | null> = {};
    const detectedApplicationPaths: Record<string, string> = {
      chatgpt: "C:\\Program Files\\ChatGPT\\ChatGPT.exe",
      "claude-desktop": "C:\\Users\\Test\\AppData\\Local\\Claude\\Claude.exe",
      opencode: "C:\\Users\\Test\\AppData\\Local\\Programs\\OpenCode\\OpenCode.exe",
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
          case "get_platform": return input.platform ?? "windows";
          case "get_local_network_access": return { enabled: networkEnabled, addresses: [{ interface: "Ethernet", baseUrl: localRuntime.gateway.baseUrl.replace("127.0.0.1", "192.168.1.10") }] };
          case "update_local_network_access": {
            if (args.expectedEnabled !== networkEnabled) throw { code: "invalid_state", message: "Network settings changed" };
            networkEnabled = Boolean(args.enabled);
            return { enabled: networkEnabled, addresses: [{ interface: "Ethernet", baseUrl: localRuntime.gateway.baseUrl.replace("127.0.0.1", "192.168.1.10") }] };
          }
          case "create_saved_top_up_intent_and_open": return null;
          case "open_api_key_page": return null;
          case "prepare_top_up_amount": return { amountCents: 1000, amountUsd: 10, valid: true };
          case "get_local_runtime_state":
            if (sourceFollowupPending && input.sourceFollowupError === "state") {
              sourceFollowupPending = false;
              throw { code: "invalid_state", message: "Synthetic snapshot read failed after source creation" };
            }
            return readRuntimeState(localRuntime);
          case "get_local_runtime_order": return structuredClone(localRuntime.gateway.routingOrder);
          case "export_local_configuration_preset": return "C:\\Temp\\zenith-relay-configuration.json";
          case "preview_local_configuration_preset": return { baseRevision: "cfg_synthetic_current", preset: structuredClone(configurationPreset), changes: [{ path: "/routing/maxRetryCandidates", before: 3, after: 4 }] };
          case "apply_local_configuration_preset": localRuntime.gateway.maxRetryCandidates = 4; return { previousRevision: "cfg_synthetic_current", revision: "cfg_synthetic_applied", changes: [{ path: "/routing/maxRetryCandidates", before: 3, after: 4 }] };
          case "get_local_cache_sessions": {
            if (input.cacheSessions) return structuredClone(input.cacheSessions);
            if (!input.cachePreview) return [];
            const now = Date.now();
            const at = (offsetMs: number) => new Date(now - offsetMs).toISOString();
            return [
              { clientContextId: "client_a1b2c3d4e5f6", candidateKind: sourceUsage ? "source" : "account", candidateId: sourceUsage ? source.id : usageAccount.id, startedAt: at(2 * 60 * 60_000), touchedAt: at(12 * 60_000), model: "gpt-6-astra", cacheWriteTtl: "30m" },
              { clientContextId: "client_998877665544", candidateKind: "account", candidateId: usageAccount.id, startedAt: at(26 * 60 * 60_000), touchedAt: at(25 * 60_000), model: "claude-sonnet-4-6", cacheWriteTtl: "1h" },
              { clientContextId: "client_aabbccddeeff", candidateKind: "account", candidateId: usageAccount.id, startedAt: at(3 * 60 * 60_000), touchedAt: at(8 * 60_000), model: "claude-opus-4-6", cacheWriteTtl: "5m" },
              { clientContextId: "client_112233445566", candidateKind: "account", candidateId: usageAccount.id, startedAt: at(5 * 60 * 60_000), touchedAt: at(45 * 60_000), model: "gpt-6-sol", cacheWriteTtl: null },
              { clientContextId: "client_ff00ff00ff00", candidateKind: "account", candidateId: usageAccount.id, startedAt: at(40 * 60_000), touchedAt: at(3 * 60_000), model: "claude-haiku-4-5", cacheWriteTtl: null },
            ];
          }
          case "get_local_usage_page": {
            const historyQuery = args.input as { requestId?: string; page?: number } | undefined;
            if (historyQuery?.requestId && input.usageAttempts) {
              if (input.usageAttemptsFail) throw { code: "io", message: "Synthetic history load failure" };
              return syntheticAttemptPage(localUsage, historyQuery);
            }
            const query = (args.input ?? {}) as UsageQuery;
            const rangeDays = query.range === "daily" ? 1 : query.range === "weekly" ? 7 : query.range === "monthly" ? 30 : null;
            const fromMs = rangeDays === null ? query.fromMs : Date.now() - rangeDays * 86_400_000;
            const periodEvents = localUsage.filter((item) => (fromMs === undefined || Date.parse(item.createdAt) >= fromMs) && (query.toMs === undefined || Date.parse(item.createdAt) <= query.toMs));
            const events = periodEvents.filter((item) => (query.success === undefined || item.success === query.success) && (!query.modelQuery || item.resolvedModel.includes(query.modelQuery) || item.requestedModel.includes(query.modelQuery)) && (!query.sourceOrAccountQuery || item.accountId?.includes(query.sourceOrAccountQuery) || item.sourceId.includes(query.sourceOrAccountQuery)) && (!query.wireApi || item.wireApi === query.wireApi) && (!query.transport || item.transport === query.transport) && (!query.errorCategory || item.errorCategory === query.errorCategory) && (!query.requestIdQuery || item.requestId.includes(query.requestIdQuery)))
              .filter((item) => query.success !== false || !(item.routing?.attemptOutcome === "tool_discovery" && !item.errorCategory && item.httpStatus >= 200 && item.httpStatus < 300));
            const filterOptions = query.includeFilterOptions && input.usageFilterOptionsVersion !== "missing" ? {
              ...(input.usageFilterOptionsVersion === "advanced" ? {} : {
                models: [...new Set(periodEvents.flatMap((event) => [event.requestedModel, event.resolvedModel]))].filter(Boolean).sort(),
                poolMembers: [...new Set(periodEvents.map((event) => event.accountId ?? event.sourceId))].filter(Boolean).sort(),
              }),
              wireApis: [...new Set(periodEvents.map((event) => event.wireApi))].sort(),
              transports: [...new Set(periodEvents.map((event) => event.transport))].sort(),
              errorCategories: [...new Set(periodEvents.flatMap((event) => event.errorCategory ? [event.errorCategory] : []))].sort(),
            } : undefined;
            const page = Math.max(1, query.page ?? 1);
            const pageSize = query.pageSize ?? 50;
            const pageEvents = events.slice((page - 1) * pageSize, page * pageSize);
            const totals = { ...usageTotals(events), ...input.usageSummaryTotals };
            const createdAtMs = events[0] ? Date.parse(events[0].createdAt) : 0;
            const bucketStart = query.bucketMs && query.fromMs != null ? query.fromMs + Math.floor((createdAtMs - query.fromMs) / query.bucketMs) * query.bucketMs : null;
            const buckets = bucketStart == null ? [] : [{ startMs: bucketStart, totals }];
            const groups = (keyFor: (event: typeof localUsage[number]) => string) =>
              [...new Set(events.map(keyFor))].map((key) => ({ key, totals: usageTotals(events.filter((event) => keyFor(event) === key)) }));
            const models = query.includeModels === false ? [] : groups((event) => event.resolvedModel || event.requestedModel);
            const poolMembers = query.includePoolMembers === false ? [] : groups((event) => event.accountId ?? event.sourceId)
              .map((group) => ({ ...group, label: localRuntime.accounts.find((item) => item.id === group.key)?.label
                ?? localRuntime.sources.find((item) => item.id === group.key)?.name ?? group.key }));
            return { events: query.includeEvents === false ? [] : structuredClone(pageEvents), filterOptions, total: events.length, page, pageSize, totalPages: events.length ? input.usageTotalPages ?? Math.ceil(events.length / pageSize) : 0, totals, buckets, models, poolMembers };
          }
          case "create_local_source": {
            if (input.sourceCreateError) throw { code: input.sourceCreateError, message: "Synthetic upstream model discovery failed" };
            const created = sourceFromPayload(args.input as Record<string, unknown>, `source_created_${localRuntime.sources.length + 1}`);
            localRuntime.sources = [...localRuntime.sources, created];
            sourceFollowupPending = true;
            return structuredClone(created);
          }
          case "update_local_source": {
            const request = args.input as Record<string, unknown> & { sourceId?: string; sourcePriorities?: Record<string, number> };
            if (input.sourceUpdateDelayMs) await new Promise((resolve) => setTimeout(resolve, input.sourceUpdateDelayMs));
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
          case "delete_local_source": {
            localRuntime.sources = localRuntime.sources.filter((source) => source.id !== args.sourceId);
            refreshGatewayModels(localRuntime);
            return structuredClone(localRuntime);
          }
          case "test_local_source": return structuredClone(source);
          case "refresh_local_source_data":
            if (input.sourceRefreshError) throw { code: "upstream_error", message: "Source models unavailable" };
            return structuredClone(localRuntime);
          case "get_local_source_stats": {
            const configuredStats = input.sourceStatsById?.[String(args.sourceId)];
            if (configuredStats) return structuredClone(configuredStats);
            if (input.sourceStats?.length) {
              const index = sourceStatsIndex++;
              const delayMs = input.sourceStatsDelayMs?.[index];
              if (delayMs) await new Promise((resolve) => setTimeout(resolve, delayMs));
              return structuredClone(input.sourceStats[Math.min(index, input.sourceStats.length - 1)]);
            }
            const selected = localRuntime.sources.find((item) => item.id === String(args.sourceId));
            const host = selected ? new URL(selected.baseUrl).host.toLowerCase() : "";
            if (host === "api.zenithmarket.dev") return { provider: "zenith", balanceMicroUsd: 42_500_000, spentMicroUsd: 7_500_000, requests: 128, totalTokens: 987_654 };
            if (host === "openrouter.ai") return { provider: "openrouter", balanceMicroUsd: 21_250_000, spentMicroUsd: 3_750_000, requests: null, totalTokens: null };
            return { provider: "unsupported", balanceMicroUsd: null, spentMicroUsd: null, requests: null, totalTokens: null };
          }
          case "start_local_account_import": return importSession("11111111-2222-4333-8444-555555555555");
          case "preview_local_account_import_files":
            if (input.importPreviewDeferred) await new Promise<void>((resolve) => {
              window.addEventListener("relay-test-finish-import-preview", () => resolve(), { once: true });
            });
            if (input.importPreviewDelayMs) await new Promise((resolve) => setTimeout(resolve, input.importPreviewDelayMs));
            return importSession("11111111-2222-4333-8444-555555555555");
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
          case "consume_local_reset_credit": {
            if (input.resetCreditsError) throw { code: "upstream_error", message: input.resetCreditsError };
            const target = localRuntime.accounts.find((item) => item.id === String(args.accountId));
            if (target) target.quota.resetCreditsAvailable = Math.max(0, target.quota.resetCreditsAvailable - 1);
            if (input.resetCreditsRefreshError) return { refreshed: false, refreshError: input.resetCreditsRefreshError };
            return { refreshed: true };
          }
          case "update_local_account": {
            const request = args.input as { accountId?: string; priority?: number; weight?: number; draining?: boolean; purchaseCostMicroUsd?: number; allowedModels?: string[]; excludedModels?: string[] };
            const target = localRuntime.accounts.find((item) => item.id === request.accountId);
            if (target && typeof request.priority === "number") target.priority = request.priority;
            if (target && typeof request.weight === "number") target.weight = request.weight;
            if (target && typeof request.draining === "boolean") target.draining = request.draining;
            if (target && typeof request.purchaseCostMicroUsd === "number") target.purchaseCostMicroUsd = request.purchaseCostMicroUsd || null;
            if (target && request.allowedModels) target.allowedModels = request.allowedModels;
            if (target && request.excludedModels) target.excludedModels = request.excludedModels;
            refreshGatewayModels(localRuntime);
            return structuredClone(localRuntime);
          }
          case "set_local_account_enabled": {
            const target = localRuntime.accounts.find((item) => item.id === args.accountId);
            if (target) target.enabled = Boolean(args.enabled);
            refreshGatewayModels(localRuntime);
            return structuredClone(localRuntime);
          }
          case "local_account_route_candidates": {
            const current = localRuntime.accounts.find((item) => item.id === args.accountId);
            return localRuntime.accounts.filter((item) => !item.routeGroupId && item.id !== args.accountId && item.oauthClientKind !== current?.oauthClientKind).map((item) => item.id);
          }
          case "select_local_account_route": {
            if (input.accountRouteSelectError) throw { code: "conflict", message: "Synthetic route switch rejected" };
            const current = localRuntime.accounts.find((item) => item.id === args.accountId)!;
            const next = localRuntime.accounts.find((item) => item.id === args.connectionId)!;
            const group = current.routeGroupId ?? current.id;
            const inPool = current.inPool;
            const groupIds = localRuntime.accounts.filter((item) => item.id === current.id || item.routeGroupId === group || item.id === next.id).map((item) => item.id);
            for (const task of localRuntime.automations) {
              if (task.accountSelector.kind === "account_ids" && task.accountSelector.values.some((id) => id !== next.id && groupIds.includes(id))) {
                task.accountSelector.values = [...new Set([...task.accountSelector.values.filter((id) => !groupIds.includes(id)), next.id])];
              }
            }
            for (const item of localRuntime.accounts) {
              if (item.id === current.id || item.routeGroupId === group || item.id === next.id) {
                item.routeGroupId = group;
                item.routeInactive = item.id !== next.id;
                item.inPool = item.id === next.id && inPool;
              }
            }
            next.weight = current.weight;
            for (const member of localRuntime.gateway.poolRouting.members) if (member.kind === "account" && member.id === current.id) member.id = next.id;
            refreshGatewayModels(localRuntime);
            return null;
          }
          case "detach_local_account_route": {
            const target = localRuntime.accounts.find((item) => item.id === args.connectionId)!;
            target.routeGroupId = null;
            target.routeInactive = false;
            target.inPool = false;
            refreshGatewayModels(localRuntime);
            return null;
          }
          case "set_local_pool_membership": {
            if (sourceFollowupPending && input.sourceFollowupError === "membership") {
              sourceFollowupPending = false;
              throw { code: "conflict", message: "Synthetic membership conflict after source creation" };
            }
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
          case "set_local_model_context": {
            if (input.modelContextDelayMs) await new Promise((resolve) => setTimeout(resolve, input.modelContextDelayMs));
            assertContextSave();
            const request = args.input as { modelId: string; contextPolicy: { mode: "short" } | { mode: "extended" } | { mode: "custom"; tokens: number } };
            const target = localRuntime.gateway.models.find((model) => model.id === request.modelId);
            if (target) {
              target.contextPolicy = request.contextPolicy;
            }
            return structuredClone(localRuntime);
          }
          case "set_local_model_reasoning": {
            const request = args.input as { modelId: string; allowedLevels: string[] };
            const target = localRuntime.gateway.models.find((model) => model.id === request.modelId);
            if (target) {
              target.reasoningAllowedLevels = [...request.allowedLevels];
              target.reasoningLevels = [...request.allowedLevels];
            }
            return structuredClone(localRuntime);
          }
          case "set_local_model_service_tier": {
            const request = args.input as { modelId: string; serviceTier: "standard" | "fast" | "ultrafast" };
            const target = localRuntime.gateway.models.find((model) => model.id === request.modelId);
            if (target) target.speedTier = request.serviceTier;
            return structuredClone(localRuntime);
          }
          case "set_local_model_display_order": {
            const request = args.input as { modelIds: string[] };
            await prepareModelOrder();
            const ids = request.modelIds.length ? request.modelIds : defaultModelOrder;
            const positions = new Map(ids.map((id, index) => [id.toLowerCase(), index]));
            localRuntime.gateway.models.sort((left, right) =>
              (positions.get(left.id.toLowerCase()) ?? Number.MAX_SAFE_INTEGER)
              - (positions.get(right.id.toLowerCase()) ?? Number.MAX_SAFE_INTEGER));
            localRuntime.gateway.visibleModelIds = localRuntime.gateway.models.filter((model) => model.enabled).map((model) => model.id);
            return structuredClone(localRuntime);
          }
          case "update_chatgpt_interface_quota_reserve": {
            const request = args.input as { reserveBasisPoints: number };
            localRuntime.gateway.chatgptInterfaceQuotaReserveBasisPoints = request.reserveBasisPoints;
            return structuredClone(localRuntime);
          }
          case "get_request_timezone_catalog": return timezoneCatalog;
          case "set_local_request_timezone": {
            localRuntime.gateway.requestTimezone = saveRequestTimezone(localRuntime.gateway.requestTimezone, args.input as RequestTimezoneUpdate);
            return structuredClone(localRuntime);
          }
          case "set_local_codex_background_tasks": {
            const request = args.input as { enabled: boolean };
            localRuntime.gateway.codexBackgroundTasksEnabled = request.enabled;
            return structuredClone(localRuntime);
          }
          case "get_local_codex_memory_model": return (args.stage === "extraction" ? input.codexMemoryExtractionModel : input.codexMemoryModel) ?? null;
          case "set_local_codex_memory_model": {
            const request = args.input as { stage: "extraction" | "consolidation"; model: string | null; expectedModel: string | null };
            const field = request.stage === "extraction" ? "codexMemoryExtractionModel" : "codexMemoryModel";
            if (input.codexMemoryModelFailure || request.expectedModel !== (input[field] ?? null)) {
              throw { code: "conflict", message: "Memory model changed; reload before saving" };
            }
            input[field] = request.model;
            return request.model;
          }
          case "set_local_codex_memory_enabled": {
            if (input.codexMemoryToggleFailure) throw { code: "storage_error", message: "Memory policy was not saved" };
            const request = args.input as { stage: "extraction" | "consolidation"; enabled: boolean };
            if (request.stage === "extraction") localRuntime.gateway.codexMemoryExtractionEnabled = request.enabled;
            else localRuntime.gateway.codexMemoryConsolidationEnabled = request.enabled;
            return structuredClone(localRuntime);
          }
          case "set_local_codex_websockets": {
            const request = args.input as { enabled: boolean };
            localRuntime.gateway.codexWebsocketsEnabled = request.enabled;
            return structuredClone(localRuntime);
          }
          case "set_local_chatgpt_retry_until_available": {
            const request = args.input as { enabled: boolean };
            localRuntime.gateway.chatgptRetryUntilAvailable = request.enabled;
            return structuredClone(localRuntime);
          }
          case "set_local_block_degraded_routes": {
            const request = args.input as { enabled: boolean };
            localRuntime.gateway.blockDegradedRoutesEnabled = request.enabled;
            return structuredClone(localRuntime);
          }
        case "update_local_routing": {
            const request = args.input as { poolRouting?: import("../../src/features/relay/api/types").PoolRoutingPolicy; expectedPoolRouting?: import("../../src/features/relay/api/types").PoolRoutingSnapshot; maxRetryCandidates: number; defaultServiceTier: "standard" | "fast" | "ultrafast" };
            if (request.poolRouting) {
              if (JSON.stringify(request.expectedPoolRouting) !== JSON.stringify(localRuntime.gateway.poolRouting)) throw { code: "conflict", message: "pool routing changed" };
              localRuntime.gateway.poolRouting = structuredClone(request.poolRouting);
            }
            localRuntime.gateway.maxRetryCandidates = request.maxRetryCandidates;
            localRuntime.gateway.defaultServiceTier = request.defaultServiceTier;
            return structuredClone(localRuntime);
          }
          case "list_local_account_trash":
            return localAccountTrash.map((entry) => ({ id: entry.record.id, label: entry.record.label, trashedAtMs: entry.trashedAtMs }));
          case "trash_local_accounts": {
            const accountIds = new Set(args.accountIds as string[]);
            for (const account of localRuntime.accounts) {
              if (accountIds.has(account.id)) localAccountTrash.push({ record: structuredClone(account), trashedAtMs: Date.now() });
            }
            localRuntime.accounts = localRuntime.accounts.filter((account) => !accountIds.has(account.id));
            for (const entry of proxyEntries) entry.assignedAccountIds = entry.assignedAccountIds.filter((id) => !accountIds.has(id));
            refreshGatewayModels(localRuntime);
            return null;
          }
          case "restore_local_account_trash":
          case "purge_local_account_trash": {
            const accountIds = new Set(args.accountIds as string[]);
            for (let index = localAccountTrash.length - 1; index >= 0; index--) {
              const entry = localAccountTrash[index]!;
              if (!accountIds.has(entry.record.id)) continue;
              if (command === "restore_local_account_trash") {
                localRuntime.accounts.push({ ...entry.record, enabled: false, inPool: false, draining: false });
              }
              localAccountTrash.splice(index, 1);
            }
            refreshGatewayModels(localRuntime);
            return null;
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
          case "check_local_stored_proxy": {
            if (input.proxyCheckDelayMs) await new Promise((resolve) => setTimeout(resolve, input.proxyCheckDelayMs));
            const errorCode = input.proxyCheckError ?? null;
            const entry = proxyEntries.find((entry) => entry.id === args.proxyId);
            const located = !errorCode && !input.proxyLocationError && !input.proxyCountryUnknown;
            const countryCode = errorCode || input.proxyCountryUnknown ? null : "NL";
            const result = { proxyId: String(args.proxyId), checkedAtMs: Date.now(), elapsedMs: 184, ip: errorCode ? null : "203.0.113.42", countryCode, region: located ? "North Holland" : null, city: located ? "Amsterdam" : null, timezone: located ? "Europe/Amsterdam" : entry?.timezone ?? null, locationSource: countryCode ? located ? "ipwhois" as const : "cloudflare" as const : null, locationErrorCode: errorCode || located ? null : input.proxyLocationError ?? "proxy_location_unavailable", errorCode };
            if (entry) { entry.lastCheck = result; entry.timezone = result.timezone; }
            return result;
          }
          case "import_local_proxy_pool": {
            const values = (args.input as { proxyUrls?: string[] })?.proxyUrls ?? [];
            let added = 0;
            let duplicates = 0;
            const addedProxyIds: string[] = [];
            for (const value of values) {
              const match = value.match(/^(?:https?:\/\/)?([^:@]+):(\d+)/);
              const endpoint = match ? `http://${match[1]}:${match[2]}` : `http://imported-${proxyEntries.length + 1}.example.test:8080`;
              if (proxyEntries.some((entry) => entry.endpoint === endpoint)) {
                duplicates += 1;
                continue;
              }
              const countryCode = value.match(/__cr[.-]([a-z]{2})/i)?.[1]?.toUpperCase() ?? null;
              proxyEntries.push({ id: `proxy_synthetic_${proxyEntries.length + 1}`, endpoint, assignedAccountIds: [], countryCode, region: null, createdAtMs: Date.now() });
              addedProxyIds.push(proxyEntries[proxyEntries.length - 1]!.id);
              added += 1;
            }
            return { added, duplicates, addedProxyIds, pool: proxyPool() };
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
          case "export_local_accounts": {
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
          case "recover_local_account": {
            if (args.confirmExternalOwnerStopped !== true) throw new Error("confirmation required");
            const account = localRuntime.accounts.find((item) => item.id === args.accountId);
            if (!account?.recoveryRequired) throw new Error("account does not require recovery");
            account.recoveryRequired = false;
            account.enabled = false;
            account.inPool = false;
            account.operationalStatus = "disabled";
            account.lastErrorCode = null;
            refreshGatewayModels(localRuntime);
            return null;
          }
          case "reveal_local_account_identity": return { accountId: String(args.accountId), identity: "person@example.test" };
          case "start_codex_oauth": {
            const flow = { loginId: "oauth_synthetic", clientKind: String(args.clientKind ?? (args.accountId ? account.oauthClientKind : "codex")), ...(args.accountId ? { targetAccountId: String(args.accountId) } : {}), authorizationUrl: "https://auth.example.invalid/authorize", redirectUri: "http://127.0.0.1:1455/auth/callback", expiresAtMs: Date.now() + 600_000, status: "pending" };
            if (input.oauthCallbackBeforeStartReturns) {
              for (const [id, listener] of eventListeners) {
                if (listener.event === "relay-oauth-status") callbacks.get(listener.handler)?.({ event: listener.event, id, payload: { loginId: flow.loginId, status: "callback_received" } });
              }
            }
            return flow;
          }
          case "resume_codex_oauth": return { loginId: String(args.loginId ?? "oauth_synthetic"), authorizationUrl: "https://auth.example.invalid/authorize", redirectUri: "http://localhost:1455/auth/callback", expiresAtMs: Date.now() + 600_000, status: "pending" };
          case "cancel_codex_oauth": return null;
          case "complete_codex_oauth": {
            if (input.oauthCompletionDeferred) {
              await new Promise<void>((resolve) => {
                window.addEventListener("relay-test-finish-oauth", () => resolve(), { once: true });
              });
            }
            if (input.oauthCompletionError) throw { code: "invalid_state", message: "Synthetic account save failed" };
            if (input.oauthCompletedRouteKind) {
              const saved: AccountSummary = { ...structuredClone(localRuntime.accounts[0]!), id: "account_synthetic_route", oauthClientKind: input.oauthCompletedRouteKind, inPool: false, routeGroupId: null, routeInactive: false };
              if (!localRuntime.accounts.some((account) => account.id === saved.id)) localRuntime.accounts.push(saved);
              return { account: { id: saved.id } };
            }
            if (!localRuntime.accounts.some((saved) => saved.id === account.id)) {
              localRuntime.accounts.push({ ...structuredClone(accounts[0]), inPool: false });
            }
            return { account: { id: account.id } };
          }
          case "start_local_gateway": localRuntime.gateway.running = true; refreshGatewayModels(localRuntime); return structuredClone(localRuntime);
          case "stop_local_gateway": localRuntime.gateway.running = false; refreshGatewayModels(localRuntime); return structuredClone(localRuntime);
          case "restart_local_gateway": refreshGatewayModels(localRuntime); return structuredClone(localRuntime);
          case "reveal_local_gateway_api_key": return args.network ? networkGatewayApiKey : localGatewayApiKey;
          case "rotate_local_gateway_api_key": {
            if (args.network) { networkGatewayApiKey = "zlr_synthetic_rotated_network_key"; return networkGatewayApiKey; }
            localGatewayApiKey = "zlr_synthetic_rotated_gateway_key"; return localGatewayApiKey;
          }
          case "update_local_gateway_port": {
            const port = Number(args.port);
            localRuntime.gateway.baseUrl = `http://127.0.0.1:${port}/v1`;
            return structuredClone(localRuntime);
          }
          case "set_local_common_proxy": {
            const request = args.input as { proxyUrl: string | null };
            localRuntime.gateway.commonProxyConfigured = Boolean(request.proxyUrl);
            localRuntime.gateway.commonProxyAvailable = Boolean(request.proxyUrl);
            if (!request.proxyUrl) localRuntime.gateway.commonProxyId = null;
            for (const item of localRuntime.accounts) {
              if (item.proxyMode === "account") continue;
              item.proxyMode = request.proxyUrl ? "common" : "direct";
              item.proxyAvailable = Boolean(request.proxyUrl) || !localRuntime.gateway.accountProxyRequired;
            }
            refreshGatewayModels(localRuntime);
            return structuredClone(localRuntime);
          }
          case "create_quota_wake_automation": Object.assign(automation, args.input); localRuntime.automations = [automation]; return structuredClone(localRuntime);
          case "update_quota_wake_automation": Object.assign(automation, args.input); return structuredClone(localRuntime);
          case "set_quota_wake_automation_enabled": automation.enabled = Boolean(args.enabled); return structuredClone(localRuntime);
          case "delete_quota_wake_automation": localRuntime.automations = []; return structuredClone(localRuntime);
          case "launch_managed_codex_profile":
            if (input.codexWebsocketsFailure === "restart") throw { code: "io", message: "Synthetic ChatGPT restart failure" };
            return null;
          case "list_codex_profile_snapshots": return { snapshots: input.profileSnapshots ? [{ id: "snapshot_synthetic", name: "Before switch", profileDir, createdAtMs: 1_781_000_000_000, configAvailable: true, authAvailable: true }] : [], invalidCount: 0 };
          case "create_codex_profile_snapshot": return { id: "snapshot_created", name: String(args.name), profileDir, createdAtMs: Date.now(), configAvailable: true, authAvailable: true };
          case "restore_full_codex_profile_snapshot":
          case "delete_codex_profile_snapshot": return null;
          case "get_application_launch_path": return { path: applicationLaunchPaths[String(args.application)] ?? null, detectedPath: detectedApplicationPaths[String(args.application)] ?? null };
          case "set_application_launch_path": {
            applicationLaunchPaths[String(args.application)] = args.path == null ? null : String(args.path);
            return { path: applicationLaunchPaths[String(args.application)], detectedPath: detectedApplicationPaths[String(args.application)] ?? null };
          }
          case "launch_application": return null;
          case "open_hosted_preview": return null;
          case "pick_application_launch_path": return detectedApplicationPaths[String(args.application)] ?? null;
          case "get_opencode_config_status": return { configured: false, modelCount: 0, hasBackup: false, backupCreatedAtMs: null, backupName: null, path: "C:\\Users\\Test\\.config\\opencode\\opencode.json" };
          case "create_opencode_snapshot": return true;
          case "connect_opencode_to_local_gateway": return { path: "C:\\Users\\Test\\.config\\opencode\\opencode.json", modelCount: 2, backupCreated: true };
          case "launch_opencode_source": return { path: "C:\\Users\\Test\\.config\\opencode\\opencode.json", modelCount: source.models.length, backupCreated: true };
          case "restart_opencode_app":
          case "restore_opencode_config": return null;
          case "get_claude_desktop_status": return { configured: claudeDesktopConfigured, modelCount: claudeDesktopConfigured ? 2 : 0, hasRecovery: claudeDesktopHasRecovery, snapshotName: claudeDesktopSnapshotName, snapshotCreatedAtMs: claudeDesktopSnapshotCreatedAtMs, running: false, path: "C:\\Users\\Test\\AppData\\Local\\Claude-3p\\configLibrary" };
          case "create_claude_desktop_snapshot": {
            if (claudeDesktopHasRecovery) return false;
            claudeDesktopHasRecovery = true;
            claudeDesktopSnapshotName = String(args.name).trim();
            claudeDesktopSnapshotCreatedAtMs = Date.now();
            return true;
          }
          case "connect_claude_desktop": claudeDesktopConfigured = true; claudeDesktopHasRecovery = true; claudeDesktopSnapshotCreatedAtMs ??= Date.now(); return { modelCount: 2, backupCreated: true };
  case "launch_claude_desktop": return null;
          case "restore_claude_desktop": claudeDesktopConfigured = false; claudeDesktopHasRecovery = false; claudeDesktopSnapshotName = null; claudeDesktopSnapshotCreatedAtMs = null; return true;
          case "stop_managed_codex_profile": return true;
          case "attach_codex_to_local_gateway": if (input.profileSwitchError) throw { code: "profile_restore_blocked", message: "Synthetic profile conflict" }; return { binding: { profileDir: "C:\\Users\\Test\\.codex", credentialKind: "local_gateway", credentialId: systemCredentialId, boundOauthAccountId: args.boundOauthAccountId ? String(args.boundOauthAccountId) : null, active: true } };
          case "launch_codex_account": return { binding: { profileDir: "C:\\Users\\Test\\.codex", credentialKind: "oauth_account", credentialId: String(args.accountId), boundOauthAccountId: null, active: true } };
          case "launch_codex_source": return { binding: { profileDir: "C:\\Users\\Test\\.codex", credentialKind: "local_gateway", credentialId: String(args.sourceId), boundOauthAccountId: null, active: true } };
          case "get_relay_storage_info": return {
            dataPath: "C:\\Users\\Test\\AppData\\Local\\Zenith Relay\\data",
            logsPath: "C:\\Users\\Test\\AppData\\Local\\Zenith Relay\\logs",
            errorLogsPath: "C:\\Users\\Test\\AppData\\Local\\Zenith Relay\\logs\\errors",
            crashLogsPath: "C:\\Users\\Test\\AppData\\Local\\Zenith Relay\\logs\\crashes",
            operationLogsPath: "C:\\Users\\Test\\AppData\\Local\\Zenith Relay\\logs\\operations",
          };
          case "get_diagnostic_settings": return { debugEnabled: diagnosticDebugEnabled };
          case "set_diagnostic_debug_mode": diagnosticDebugEnabled = Boolean(args.enabled); return { debugEnabled: diagnosticDebugEnabled };
          case "open_relay_folder": return null;
          case "reset_local_pool_data": localRuntime.sources = []; localRuntime.accounts = []; localRuntime.automations = []; localUsage = []; return null;
          case "clear_local_usage": localUsage = []; return null;
          case "export_usage": return "C:\\Temp\\usage.json";
          case "plugin:window|is_fullscreen": return fullscreen;
          case "plugin:window|close":
          case "plugin:window|minimize":
          case "plugin:window|toggle_maximize": return null;
          case "plugin:event|listen": {
            const eventId = nextEventListener++;
            eventListeners.set(eventId, { event: String(args.event), handler: Number(args.handler) });
            return eventId;
          }
          case "plugin:event|unlisten": eventListeners.delete(Number(args.eventId)); return null;
          case "plugin:app|bundle_type": return input.bundleType === null ? null : input.bundleType ?? "nsis";
          case "get_portable_update_target": return input.bundleType === null ? "windows-x86_64-portable" : null;
          case "plugin:updater|check":
            if (input.updateCheckDelayMs) await new Promise((resolve) => setTimeout(resolve, input.updateCheckDelayMs));
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
        ...(input.importPreviewError ? [{ itemId: "import_invalid_refresh_exchange", label: "Invalid imported account", identity: "in••••id", authMode: "oauth", sourceName: "OpenAI", quotaStatus: "skipped", status: "invalid", plan: null, defaultSelected: false, selectable: false, existing: false, warnings: [], error: { code: "refresh_exchange_failed", message: "synthetic import validation failure" } }] : []),
      ].map((row) => ({ ...row, oauthClientKind: input.accountOAuthClientKind ?? "codex" })), warnings: [] } };
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
      const inventory = [...runtime.accounts.filter((m) => m.inPool).map((m) => ({ kind: "account" as const, id: m.id, weight: m.weight, maxConcurrency: 0 })), ...runtime.sources.filter((m) => m.inPool).map((m) => ({ kind: "source" as const, id: m.id, weight: m.weight, maxConcurrency: 0 }))];
      const key = (m: { kind: string; id: string }) => `${m.kind}:${m.id}`;
      const existing = runtime.gateway.poolRouting.members.filter((m) => inventory.some((item) => key(item) === key(m)));
      runtime.gateway.poolRouting.members = [...existing, ...inventory.filter((m) => !existing.some((item) => key(item) === key(m)))];
      const previousOrder = new Map(runtime.gateway.routingOrder.map((candidate) => [candidate.candidateId, candidate]));
      const previousRank = new Map(runtime.gateway.routingOrder.map((candidate, index) => [candidate.candidateId, index]));
      for (const item of runtime.sources) {
        item.operationalStatus = !item.enabled
          ? "disabled"
          : !item.draining && item.secretAvailable ? input.sourceOperationalStatus ?? "rotation" : "unavailable";
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
      const modelMembers = members.filter((member) => member.inPool);
      runtime.gateway.candidateCount = eligible.length;
      const orderedMembers = members
        .filter((member) => member.inPool)
        .sort((left, right) => {
          if ("baseUrl" in left && "baseUrl" in right && left.priority !== right.priority) {
            return right.priority - left.priority;
          }
          return (previousRank.get(left.id) ?? Number.MAX_SAFE_INTEGER) - (previousRank.get(right.id) ?? Number.MAX_SAFE_INTEGER);
        });
      const requestedNextCandidateId = input.nextCandidateId === undefined ? usageAccount.id : input.nextCandidateId;
      const nextCandidateId = requestedNextCandidateId != null
        && orderedMembers.some((member) => member.id === requestedNextCandidateId && member.operationalStatus === "rotation")
        ? requestedNextCandidateId
        : input.nextCandidateId === undefined
          ? orderedMembers.find((member) => member.operationalStatus === "rotation")?.id ?? null
          : requestedNextCandidateId;
      runtime.gateway.routingOrder = orderedMembers
        .map((member, index) => {
          const previous = previousOrder.get(member.id);
          const memberActiveModels = member.id === usageAccount.id ? activeModelCounts : [];
          const memberActiveRequestCount = memberActiveModels.reduce((count, item) => count + item.requestCount, 0);
          return {
            candidateId: member.id,
            kind: "baseUrl" in member ? "api_source" as const : "oauth_account" as const,
            available: member.operationalStatus === "rotation",
            nextForNewRequest: member.operationalStatus === "rotation"
              && member.id === nextCandidateId,
            resourceHeadroom: input.routingHeadroom?.[member.id] ?? previous?.resourceHeadroom,
            activityRevision: previous?.activityRevision ?? 0,
            inFlight: previous?.inFlight ?? memberActiveRequestCount,
            activeRequestCount: previous?.activeRequestCount ?? memberActiveRequestCount,
            activeModels: previous?.activeModels ?? structuredClone(memberActiveModels),
            modelRetries: previous?.modelRetries ?? (input.accountModelCooldown && member.id === account.id ? [{ model: "gpt-5.4", retryAtMs: Date.now() + 30 * 60_000 }] : []),
            lastUsedAtMs: previous?.lastUsedAtMs ?? (usagePresent && member.id === usageAccount.id ? Date.now() - 1_000 : null),
            nextRetryAtMs: previous?.nextRetryAtMs ?? (input.accountCooldown && member.id === account.id ? Date.now() + 30 * 60_000 : null),
            halfOpen: previous?.halfOpen ?? false,
            dispatches: previous?.dispatches ?? index,
          };
        });
      const ids = [...new Map(modelMembers.flatMap((member) => member.models).map((id) => [id.toLowerCase(), id])).values()];
      const models = ids.map((id) => {
        const current = currentModels.get(id.toLowerCase());
        const reportedReasoning = input.modelReasoning?.[id.toLowerCase()] ?? [];
        // A missing manual policy keeps source-declared modes enabled. A saved
        // empty array is intentional and disables them, so use nullish rather
        // than truthy fallback here.
        const hasApiSourceRoute = runtime.sources.some((source) => source.enabled
          && source.inPool
          && !source.draining
          && source.secretAvailable
          && source.models.some((model) => model.toLowerCase() === id.toLowerCase()));
        const hasNativeAccountRoute = runtime.accounts.some((account) => account.enabled
          && account.inPool
          && !account.draining
          && account.secretAvailable
          && account.models.some((model) => model.toLowerCase() === id.toLowerCase()));
        const hasPoolRoute = hasApiSourceRoute || hasNativeAccountRoute;
        const manualReasoning = current?.reasoningAllowedLevels ?? reportedReasoning;
        const context = input.modelContext?.[id.toLowerCase()];
        const price = current?.customPrice
          ? { inputMicroUsdPerMillion: current.inputMicroUsdPerMillion, cachedInputMicroUsdPerMillion: current.cachedInputMicroUsdPerMillion, cacheWrite5mMicroUsdPerMillion: current.cacheWrite5mMicroUsdPerMillion, cacheWrite1hMicroUsdPerMillion: current.cacheWrite1hMicroUsdPerMillion, outputMicroUsdPerMillion: current.outputMicroUsdPerMillion }
          : modelPrices[id.toLowerCase()] ?? { inputMicroUsdPerMillion: null, cachedInputMicroUsdPerMillion: null, outputMicroUsdPerMillion: null };
        return {
          id,
          enabled: current?.enabled ?? true,
          memberCount: modelMembers.filter((member) => member.models.some((model) => model.toLowerCase() === id.toLowerCase())).length,
          codexVisible: (current?.enabled ?? true) && hasPoolRoute,
          protocolRoutes: input.modelProtocolRoutes?.[id],
          codexDisplayName: id.replaceAll("-", " "),
          ...(modelMetadata[id.toLowerCase()] ?? {}),
          catalogContextLimit: context?.contextLimit ?? current?.catalogContextLimit ?? null,
          contextWindows: context?.windows ?? current?.contextWindows,
          contextPolicy: context?.policy ?? current?.contextPolicy ?? { mode: "short" },
          contextConfigurable: context?.configurable ?? current?.contextConfigurable ?? false,
          catalogInputLimit: context?.inputLimit ?? current?.catalogInputLimit ?? null,
          catalogOutputLimit: context?.outputLimit ?? current?.catalogOutputLimit ?? null,
          ...price,
          imageRequestPrices: id.toLowerCase().includes("image") ? [
            { operation: "generation", quality: "low", size: "1024x1024", microUsd: 6000 },
            { operation: "generation", quality: "low", size: "1024x1536", microUsd: 7000 },
            { operation: "generation", quality: "medium", size: "1024x1024", microUsd: 9000 },
          ] : [],
          customPrice: current?.customPrice ?? false,
          reasoningLevels: manualReasoning,
          reasoningSupportedLevels: current?.reasoningSupportedLevels ?? reportedReasoning,
          reasoningAllowedLevels: manualReasoning,
          reasoningConfigurable: reportedReasoning.length > 0,
          reasoningManualFallback: current?.reasoningManualFallback
            ?? input.manualReasoningFallbackModels?.some((model) => model.toLowerCase() === id.toLowerCase())
            ?? false,
          speedTiers: Object.prototype.hasOwnProperty.call(input.modelSpeed ?? {}, id.toLowerCase())
            ? ["standard", "fast", "ultrafast"]
            : ["standard"],
          speedSupported: Object.prototype.hasOwnProperty.call(input.modelSpeed ?? {}, id.toLowerCase()),
          speedTier: input.modelSpeed?.[id.toLowerCase()] ?? "standard",
          speedConfigurable: Object.prototype.hasOwnProperty.call(input.modelSpeed ?? {}, id.toLowerCase()),
        };
      });
      runtime.gateway.models = models.filter((model) => !input.omittedPoolModels?.includes(model.id));
      runtime.gateway.visibleModelIds = runtime.gateway.models.filter((model) => model.enabled).map((model) => model.id);
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
