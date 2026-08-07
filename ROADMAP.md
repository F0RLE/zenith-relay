# Zenith Relay Roadmap

Last reviewed: 2026-08-05.

This roadmap contains only remaining acceptance gates and future work. It does
not repeat completed implementation history. A phase is complete only when its
verification evidence exists, not when its UI is present.

## P0 - Prove the personal pool in production (deferred)

Deferred at the user's request until real accounts are available and the user
explicitly resumes this work. Do not perform live account operations in the
meantime. Mocked UI, unit tests, and a local build remain necessary but cannot
replace the following acceptance gates.

### Local pool acceptance

1. Import or sign in to at least two healthy personal accounts.
2. Put both in the pool and prove normal rotation for a shared model.
3. Test an optional proxy route with a real reachable proxy.
4. Exercise a quota refresh, a real cooldown or unavailable state, recovery
   after a success, and the resulting UI state.
5. Confirm Usage records the selected member, timings, token split, and
   classified result without exposing a secret or prompt body.

### User-managed server acceptance

1. Deploy the server behind HTTPS with a separate management token and vault
   key, then attach the managed ChatGPT/Codex profile.
2. Transfer or create permitted test connections through the desktop
   management path and verify the server capability snapshot.
3. Send a streaming <code>/v1</code> request through the server.
4. Close the desktop app, send another request, then reopen the app.
5. Compare the server quota, account state, request count, token totals,
   timings, and API-equivalent usage with the desktop view.
6. Take a backup, restore it to a clean test location, and prove that the
   restored runtime passes a request.

Do not call the server pool production-ready until this entire path has passed
against real accounts and providers.

## P1 - Measure and remove visible latency

Instrumentation exists for native startup, vault, SQLite, window creation,
first frame, interactive state, snapshots, and mode switch. Use it to collect
cold and warm baselines before changing architecture.

The 2026-08-05 representative-data cold baseline measured native startup at
35.52 ms, window creation at 409.47 ms, first frame at 76.00 ms, interactive
state at 200.07 ms, Pool opening at 12.22 ms, and a full snapshot at 46.57 ms.
Pool no longer reads Usage and has a local/remote browser regression test.

1. Measure warm startup, mode switch, and opening Connections and Usage with a
   representative account set.
2. Keep API-equivalent cached until usage or pricing changes.
3. Skip identical full snapshots when the state revision has not changed.
4. Turn any measured regression into a small reproducible check before
   optimizing it.

The goal is a responsive application, not speculative caching or background
work that makes account state stale.

## P2 - Finish reliability and ownership cleanup

1. Finish current relay-core, Tauri, and server module moves only where they
   reduce duplicate behavior.
2. Keep one canonical account mutation path for local and remote modes.
3. Keep one error classification and one UI state mapping for quota, proxy,
   credential, and source errors.
4. Remove dead compatibility branches after their regression tests cover the
   supported import and profile flows.
5. Keep server migrations append-only and verify upgrade, interrupted
   migration recovery, backup, and restore.

The refactor is complete when a behavior has one owner, not when every file
has been renamed.

## P3 - Dynamic model catalogs and client integrations

### Provider-neutral source adapter acceptance

The source catalog must remain provider-neutral. A source contributes model
capabilities and an explicit client/upstream binding; the scheduler does not
branch on vendor names.

1. Keep native passthrough as the default and require an explicit adapter for
   every protocol conversion.
2. Do not claim hosted tools, dynamic discovery, unsupported reasoning, or
   WebSocket bridging until an exact adapter path and regression coverage exist.
3. Before release, run a real `codex.exe` acceptance matrix for every claimed
   source family: initial tool call, actual local tool execution, follow-up
   `function_call_output`, streaming, and a fresh turn after restart.

### Pool-backed Codex catalog acceptance

Relay already derives a managed `model_catalog_json` from the live pool and
restores the previous profile catalog on recovery. Remaining release gates:

1. Run attach, refresh, disabled-model, removed-model, and restore cycles
   against a real current Codex profile, including a model id containing `/`.
2. Prove that the live `/v1/models` view and Codex-specific catalog make the
   same eligible model visible through the managed profile.
3. Keep a failure during catalog refresh reversible: the previous verified
   profile must remain usable and native/user settings must not be overwritten.

### Additional client applications

Add a client integration only where users need it and the program supports a
safe reversible configuration path. Each integration must use the shared pool
endpoint and server authentication boundary; it must not fork routing or
secret storage.

## P4 - Additional subscription account connectors (deferred)

Potential examples include Kiro and Antigravity. Each is a separate
compatibility project, not a switch in the existing ChatGPT connector.

This work is deferred until the user explicitly requests a connector and can
provide a permitted live account for acceptance. Do not implement speculative
account adapters before then.

Before implementation, confirm the provider's permitted authentication and
automation model, then design a connector that supplies canonical credentials,
capabilities, quota/health state, execution, refresh, deletion, and recovery.
Only after live provider tests may the connector enter the pool UI. Never
pretend that a provider's quota or models are equivalent to ChatGPT's.

## P5 - Server scale only when a personal deployment outgrows one instance

This phase is demand-gated. Leave the current single-instance architecture in
place until a real personal deployment demonstrates the need to resume it.

1. Define a multi-replica deployment contract and a shared durable state
   boundary.
2. Add distributed candidate leases before more than one server can schedule
   the same account.
3. Add shared prompt affinity only after leases and real multi-replica tests.
4. Re-evaluate provider/model storm-breaker coordination with measured load.

Do not add distributed coordination to the current single-user server without
this demand and acceptance evidence.

## P6 - Localization, documentation, and release hygiene

1. Keep README, planning, roadmap, and Help aligned with shipped behavior.
2. For every new UI locale, add the localized overview and three mode-specific
   Help documents, then register the mode documents in Help Center.
3. Regenerate screenshots from Playwright after a material layout or
   terminology change.
4. Run all relevant checks, review generated assets, and perform the P0 live
   acceptance before a release claim.
