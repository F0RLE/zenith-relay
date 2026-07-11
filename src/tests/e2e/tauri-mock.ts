import type { Page } from "@playwright/test";

export type MockOptions = {
  locale?: "en" | "ru";
  onboarding?: boolean;
  mode?: "local" | "remote" | "zenith";
  theme?: "system" | "light" | "dark";
  populated?: boolean;
  importResult?: "success" | "item_failure" | "not_found";
};

export async function installTauriMock(page: Page, options: MockOptions = {}) {
  await page.addInitScript((input) => {
    const locale = input.locale ?? "en";
    const populated = input.populated ?? true;
    localStorage.setItem("relay.onboarding", input.onboarding === false ? "0" : "1");
    localStorage.setItem("relay.mode", input.mode ?? "local");
    localStorage.setItem("relay.theme", input.theme ?? "light");

    const quota = {
      primary: { kind: "primary", availableBasisPoints: 0, explicitlyFull: false, resetAtMs: Date.now() + 90 * 60_000, windowMinutes: 300, observedAtMs: Date.now() },
      secondary: { kind: "secondary", availableBasisPoints: 6400, explicitlyFull: false, resetAtMs: Date.now() + 3 * 24 * 60 * 60_000, windowMinutes: 10_080, observedAtMs: Date.now() },
      resetCreditsAvailable: 1,
      updatedAtMs: Date.now(),
      error: null,
    };
    const source = {
      id: "source_synthetic",
      name: "Example compatible API",
      enabled: true,
      draining: false,
      baseUrl: "https://example.invalid/v1",
      wireApi: "responses",
      models: ["gpt-5.4", "gpt-5.4-mini"],
      allowedModels: [],
      excludedModels: [],
      priority: 10,
      weight: 100,
      secretAvailable: true,
      lastErrorCode: null,
    };
    const account = {
      id: "account_synthetic",
      label: "Personal Plus",
      identityHint: "ac••••42",
      enabled: true,
      draining: false,
      authState: "active",
      health: "healthy",
      models: ["gpt-5.4", "gpt-5.4-mini"],
      allowedModels: [],
      excludedModels: [],
      priority: 20,
      weight: 100,
      subscription: { planType: "Plus", activeUntilMs: null, status: "active", updatedAtMs: Date.now() },
      quota,
      secretAvailable: true,
      lastErrorCode: null,
    };
    const key = {
      id: "key_synthetic",
      label: "Codex",
      enabled: true,
      sourceIds: null,
      accountIds: null,
      allowedModels: [],
      excludedModels: [],
      modelPrefix: null,
      createdAtMs: Date.now() - 86_400_000,
      lastUsedAtMs: Date.now() - 60_000,
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
      schemaVersion: 4,
      runtimeTarget: { kind: "local", connected: true, origin: "http://127.0.0.1:14998", serverId: null, version: "1.0.5" },
      gateway: { running: true, baseUrl: "http://127.0.0.1:14998/v1", candidateCount: populated ? 2 : 0, visibleModelIds: populated ? ["gpt-5.4", "gpt-5.4-mini"] : [] },
      platform: "windows",
      capabilities: { features: ["sources", "oauth_accounts", "quota_wake", "profiles"] },
      sources: populated ? [source] : [],
      accounts: populated ? [account] : [],
      keys: populated ? [key] : [],
      automations: populated ? [automation] : [],
      wakeHistory: populated ? [{ taskId: automation.id, accountId: account.id, windowKind: "primary", modelId: "gpt-5.4-mini", outcome: "confirmed", startedAtMs: Date.now() - 120_000, completedAtMs: Date.now() - 118_000, errorCode: null }] : [],
      warnings: [],
    };
    const remoteRuntime = structuredClone(localRuntime);
    remoteRuntime.runtimeTarget = { kind: "remote", connected: true, origin: "https://relay.example.invalid", serverId: "server_synthetic", version: "1.0.5" };
    remoteRuntime.gateway.baseUrl = "https://relay.example.invalid/v1";
    remoteRuntime.platform = "linux";
    remoteRuntime.capabilities = { features: ["sources", "oauth_accounts", "quota_wake", "usage", "backup_restore"] };

    let localUsage = populated ? [{ id: 1, createdAt: new Date().toISOString(), requestId: "req_synthetic_local", attempt: 1, localKeyId: key.id, sourceId: source.id, accountId: account.id, requestedModel: "gpt-5.4", resolvedModel: "gpt-5.4", success: true, httpStatus: 200, errorCategory: null, latencyMs: 428, inputTokens: 20, outputTokens: 8, totalTokens: 28 }] : [];
    const remoteUsage = populated ? [{ id: 2, requestId: "req_synthetic_remote", localKeyId: key.id, candidateKind: "account", candidateHint: "a1b2c3d4e5f6", requestedModel: "gpt-5.4", resolvedModel: "gpt-5.4", success: true, httpStatus: 200, errorCategory: null, latencyMs: 512, inputTokens: 18, outputTokens: 7, totalTokens: 25, createdAtMs: Date.now() }] : [];
    let readyKey = "zrk_synthetic_ready_key";
    const invocations: Array<{ command: string; args: Record<string, unknown> }> = [];
    const callbacks = new Map<number, (...args: unknown[]) => unknown>();
    let nextCallback = 1;

    const tauri = {
      transformCallback(callback: (...args: unknown[]) => unknown, once = false) {
        const id = nextCallback++;
        callbacks.set(id, (...args: unknown[]) => { const result = callback(...args); if (once) callbacks.delete(id); return result; });
        return id;
      },
      unregisterCallback(id: number) { callbacks.delete(id); },
      convertFileSrc(path: string) { return path; },
      async invoke(command: string, args: Record<string, unknown> = {}) {
        invocations.push({ command, args: structuredClone(args) });
        switch (command) {
          case "get_system_locale": return locale;
          case "get_platform": return "windows";
          case "get_state": return { providerActive: Boolean(readyKey), codexRunning: false, hasSavedApiKey: Boolean(readyKey) };
          case "get_key_stats": return { balance: 42.5, spent: 7.5, requests: 18, totalTokens: 2500, inputTokens: 1700, cachedTokens: 300, reasoningTokens: 100, outputTokens: 400 };
          case "get_saved_key_stats": return { balance: 42.5, spent: 7.5, requests: 18, totalTokens: 2500, inputTokens: 1700, cachedTokens: 300, reasoningTokens: 100, outputTokens: 400 };
          case "get_key_usage_history": return { usage: populated ? [{ id: 3, createdAt: new Date().toISOString(), status: "success", model: "gpt-5.4", modelDisplay: "gpt-5.4", streamDurationMs: 390, timeToFirstByteMs: 120, totalTokens: 30, requestId: "req_synthetic_ready", responseTimeDisplay: "390 ms" }] : [], limit: 100, sinceId: null };
          case "get_saved_key_usage_history": return { usage: populated ? [{ id: 3, createdAt: new Date().toISOString(), status: "success", model: "gpt-5.4", modelDisplay: "gpt-5.4", streamDurationMs: 390, timeToFirstByteMs: 120, totalTokens: 30, requestId: "req_synthetic_ready", responseTimeDisplay: "390 ms" }] : [], limit: 100, sinceId: null };
          case "create_saved_top_up_intent_and_open": return null;
          case "save_key": readyKey = String(args.apiKey ?? ""); return readyKey;
          case "reset_key": readyKey = ""; return "reset";
          case "prepare_top_up_amount": return { amountCents: 1000, amountUsd: 10, valid: true };
          case "get_local_runtime_state": return structuredClone(localRuntime);
          case "get_remote_server_state": return structuredClone(remoteRuntime);
          case "get_local_usage": return structuredClone(localUsage);
          case "get_remote_server_usage": return { events: structuredClone(remoteUsage), total: remoteUsage.length, page: 1, pageSize: 100, totalPages: 1 };
          case "create_local_source": localRuntime.sources = [source]; return structuredClone(source);
          case "update_local_source": return structuredClone(localRuntime);
          case "rotate_local_source_key": return structuredClone(localRuntime);
          case "set_local_source_enabled": source.enabled = Boolean(args.enabled); return structuredClone(localRuntime);
          case "delete_local_source": localRuntime.sources = []; return structuredClone(localRuntime);
          case "test_local_source": return structuredClone(source);
          case "start_local_account_import": return importSession("11111111-2222-4333-8444-555555555555");
          case "resume_local_account_import": return importSession(String(args.sessionId ?? "11111111-2222-4333-8444-555555555555"));
          case "prepare_local_account_import": return importSession(String((args.input as { sessionId?: string })?.sessionId ?? "11111111-2222-4333-8444-555555555555"));
          case "confirm_local_account_import": {
            if (input.importResult === "not_found") throw { code: "not_found" };
            const request = args.input as { sessionId?: string; selectedItemIds?: string[] };
            const itemIds = request.selectedItemIds ?? [];
            return {
              sessionId: request.sessionId ?? "11111111-2222-4333-8444-555555555555",
              results: itemIds.map((itemId, index) => input.importResult === "item_failure" && index === 0
                ? { itemId, status: "failed", error: { code: "item_not_found", message: "synthetic failure" } }
                : { itemId, status: "succeeded" }),
            };
          }
          case "cancel_local_account_import": return null;
          case "refresh_local_account_quota": return structuredClone(localRuntime);
          case "refresh_all_local_account_quotas": return structuredClone(localRuntime);
          case "update_local_account": return structuredClone(localRuntime);
          case "set_local_account_enabled": account.enabled = Boolean(args.enabled); return structuredClone(localRuntime);
          case "set_local_account_draining": account.draining = Boolean(args.draining); return structuredClone(localRuntime);
          case "delete_local_account": localRuntime.accounts = []; return structuredClone(localRuntime);
          case "start_codex_oauth": return { loginId: "oauth_synthetic", authorizationUrl: "https://auth.example.invalid/authorize", redirectUri: "http://127.0.0.1:14521/callback", expiresAtMs: Date.now() + 600_000, status: "pending" };
          case "resume_codex_oauth": return { loginId: String(args.loginId ?? "oauth_synthetic"), authorizationUrl: "https://auth.example.invalid/authorize", redirectUri: "http://127.0.0.1:14521/callback", expiresAtMs: Date.now() + 600_000, status: "pending" };
          case "get_codex_oauth_status": return { loginId: "oauth_synthetic", authorizationUrl: "https://auth.example.invalid/authorize", redirectUri: "http://127.0.0.1:14521/callback", expiresAtMs: Date.now() + 600_000, status: "callback_received" };
          case "submit_codex_oauth_callback":
          case "complete_codex_oauth":
          case "cancel_codex_oauth": return null;
          case "create_local_gateway_key": localRuntime.keys = [key]; return { key: structuredClone(key), secret: "zlr_synthetic_local_key" };
          case "update_local_gateway_key": return structuredClone(localRuntime);
          case "rotate_local_gateway_key": return { key: structuredClone(key), secret: "zlr_synthetic_rotated_key" };
          case "set_local_gateway_key_enabled": key.enabled = Boolean(args.enabled); return structuredClone(localRuntime);
          case "delete_local_gateway_key": localRuntime.keys = []; return structuredClone(localRuntime);
          case "start_local_gateway": localRuntime.gateway.running = true; return structuredClone(localRuntime);
          case "stop_local_gateway": localRuntime.gateway.running = false; return structuredClone(localRuntime);
          case "create_quota_wake_automation": Object.assign(automation, args.input); localRuntime.automations = [automation]; return structuredClone(localRuntime);
          case "update_quota_wake_automation": Object.assign(automation, args.input); return structuredClone(localRuntime);
          case "set_quota_wake_automation_enabled": automation.enabled = Boolean(args.enabled); return structuredClone(localRuntime);
          case "delete_quota_wake_automation": localRuntime.automations = []; return structuredClone(localRuntime);
          case "run_due_quota_wake_confirmations": return 1;
          case "attach_codex_to_local_gateway":
          case "launch_saved_codex":
          case "restore_codex_profile":
          case "attach_codex_to_account":
          case "restore_codex_account_profile": return null;
          case "open_relay_folder": return null;
          case "reset_local_pool_data": localRuntime.sources = []; localRuntime.accounts = []; localRuntime.keys = []; localRuntime.automations = []; localUsage = []; return null;
          case "export_usage": return "C:\\Temp\\usage.json";
          case "export_support_bundle": return "C:\\Temp\\support.json";
          case "list_codex_account_bindings": return populated ? [{ profileDir: "C:\\Users\\Test\\.codex", credentialKind: "local_gateway", credentialId: key.id }] : [];
          case "connect_remote_server": return { target: { origin: remoteRuntime.runtimeTarget.origin, serverId: remoteRuntime.runtimeTarget.serverId, identityFingerprint: "synthetic-fingerprint", serverVersion: "1.0.5", protocolVersion: 1, allowInsecureHttp: false, connectedAtMs: Date.now() } };
          case "disconnect_remote_server": return null;
          case "refresh_remote_server_capabilities": return { target: remoteRuntime.runtimeTarget };
          case "prepare_remote_server_deployment": return { directory: "C:\\Temp\\zenith-relay-deploy", publicBaseUrl: "https://relay.example.invalid", managementToken: "synthetic-management-token-000000", composeCommand: "docker compose up -d" };
          case "execute_remote_server_action": return remoteAction(args);
          case "plugin:event|listen": return 1;
          case "plugin:event|unlisten":
          case "plugin:updater|check":
          case "plugin:process|relaunch":
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
        { itemId: "import_fedcba9876543210", label: "Existing account", identity: "ex••••ng", authMode: "oauth", sourceName: "OpenAI", quotaStatus: "available", status: "existing", plan: "Plus", defaultSelected: false, selectable: true, existing: true, warnings: [] },
      ], warnings: [] } };
    }

    function remoteAction(args: Record<string, unknown>) {
      const input = args.input as { action?: { type?: string }; payload?: Record<string, unknown> };
      const type = input?.action?.type;
      if (type === "rotate_key") return { key, secret: "zlr_synthetic_remote_rotated_key" };
      if (type === "create_key") return { key, secret: "zlr_synthetic_remote_key" };
      if (type === "preview_account_import") return { sessionId: "remote_import", accountId: "account_remote", duplicateAccountId: null, label: "Remote account", identityHint: "re••••te" };
      return null;
    }

    Object.defineProperty(window, "__TAURI_INTERNALS__", { configurable: true, value: tauri });
    Object.defineProperty(window, "__TAURI_TEST_INVOKES__", { configurable: true, value: invocations });
  }, options);
}
