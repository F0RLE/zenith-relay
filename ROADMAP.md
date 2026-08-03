# Zenith Relay Roadmap

Last reviewed: 2026-08-03.

This roadmap contains only remaining acceptance gates and future work. It does
not repeat completed implementation history. A phase is complete only when its
verification evidence exists, not when its UI is present.

## P0 - Prove the personal pool in production

The immediate priority is live acceptance of the local and user-managed server
paths. Mocked UI, unit tests, and a local build are necessary but insufficient.

### Local pool acceptance

1. Import or sign in to at least two healthy personal accounts.
2. Put both in the pool and prove normal rotation for a shared model.
3. Test an optional proxy route with a real reachable proxy.
4. Exercise a quota refresh, a real cooldown or unavailable state, recovery
   after a success, and the resulting UI state.
5. Confirm Usage records the selected member, timings, token split, and
   classified result without exposing a secret or prompt body.

### User-managed server acceptance

1. Deploy the server behind HTTPS with a separate management token, vault key,
   and a scoped client pool key.
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

1. Measure cold startup, warm startup, mode switch, and opening Connections,
   Pool, and Usage with a representative account set.
2. Verify that Usage is loaded only by Overview and Usage, not every page.
3. Keep API-equivalent cached until usage or pricing changes.
4. Skip identical full snapshots when the state revision has not changed.
5. Turn any measured regression into a small reproducible check before
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

### Provider-neutral source adapters

The source catalog must remain provider-neutral. A source contributes model
capabilities and an explicit client/upstream binding; the scheduler does not
branch on vendor names.

1. Keep native passthrough as the default and require an explicit adapter for
   every protocol conversion.
2. Keep the Responses-to-Messages bridge limited to JSON-schema function tools,
   direct custom text tools, native `tool_use`/`tool_result` continuations,
   and translated JSON/SSE. Do not claim hosted, namespace, or dynamic
   discovery tools until an exact adapter path exists.
3. Keep bridge continuation state bounded and volatile, and never send a tool
   result without the prior native assistant turn.
4. Do not advertise reasoning, opaque hosted tools, or WebSocket support unless
   the selected binding and tests prove those capabilities.
5. Add contract coverage for normal responses, malformed upstream payloads,
   streaming text and tool arguments, reasoning modes, rate/error paths, and
   native passthrough regression.
6. Before release, run a real `codex.exe` acceptance matrix for every claimed
   source family: initial tool call, actual local tool execution, follow-up
   `function_call_output`, streaming, and a fresh turn after restart.

### Pool-backed model catalog for Codex

1. Treat the live pool catalog as the source of truth.
2. Generate a separate Relay-managed provider section in the Codex profile.
3. Include only models exposed by the selected pool and client key.
4. Preserve native Codex models and user-managed provider entries separately.
5. Rebuild the managed catalog on a relevant pool revision, then snapshot,
   apply, verify, and restore safely.
6. Test empty pool, disabled model, removed model, unavailable candidate, and
   profile restore cases.
7. Use Codex's root `model_catalog_json` setting and derive strict catalog
   entries from an installed native template instead of inventing incomplete
   JSON rows.
8. Namespace Relay models separately and keep an exact reversible mapping for
   upstream model ids that already contain `/`.
9. Keep the OpenAI list response and the Codex
   `/v1/models?client_version=...` catalog filtered by the same live registry
   and client-key policy.
10. Write the managed catalog atomically and invalidate `models_cache.json`
    only after a verified catalog change. Recovery restores the previous
    catalog and leaves native/user-managed entries untouched.

This avoids a hard-coded model list and makes the source of each model clear in
the client selector. OpenCodex's MIT-licensed implementation was reviewed as a
compatibility reference; Relay reuses the Codex-native file/endpoint contract,
not its proxy, provider adapters, scheduler, or configuration store.

### Additional client applications

Add a client integration only where users need it and the program supports a
safe reversible configuration path. Each integration must use the shared pool
endpoint and existing client-key model policy; it must not fork routing or
secret storage.

## P4 - Additional subscription account connectors

Potential examples include Kiro and Antigravity. Each is a separate
compatibility project, not a switch in the existing ChatGPT connector.

Before implementation, confirm the provider's permitted authentication and
automation model, then design a connector that supplies canonical credentials,
capabilities, quota/health state, execution, refresh, deletion, and recovery.
Only after live provider tests may the connector enter the pool UI. Never
pretend that a provider's quota or models are equivalent to ChatGPT's.

## P5 - Server scale only when a personal deployment outgrows one instance

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
