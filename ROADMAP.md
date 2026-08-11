# Zenith Relay Roadmap

Last reviewed: 2026-08-11.

This roadmap contains only remaining acceptance gates and future work. It does
not repeat completed implementation history. A phase is complete only when its
verification evidence exists, not when its UI is present.

## Current order

1. P0 is blocked only on permitted real accounts and resumes when the user
   explicitly supplies them.
2. Until then, work proceeds through P1, P2, and P3: measure visible latency,
   protect the shared reliability contracts, and collect real client/provider
   compatibility evidence.
3. P4 and P5 remain demand-gated. Do not add speculative account connectors or
   multi-server coordination ahead of the existing acceptance gates.

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
Policy-only source and account edits hot-apply their candidate state; network
and secret changes remain explicit rebuild operations.

1. Measure warm startup, mode switch, Connections, Usage, and policy-save
   latency with a representative account set.
2. Prove by regression test that policy edits do not reopen the local listener
   or discard active state; endpoint, port, and secret changes may rebuild.
3. Keep API-equivalent cached until usage or pricing changes and skip identical
   full snapshots when the state revision has not changed.
4. Turn a measured regression into a small reproducible check before
   optimizing it.

The goal is a responsive application, not speculative caching or background
work that makes account state stale.

## P2 - Preserve reliability and ownership boundaries

The broad ownership cleanup is complete. Do not reopen it for cosmetic module
moves. Continue only where a new regression shows two owners for the same
behavior.

1. Keep local and remote account mutations on their canonical transactional
   paths and add a regression test before consolidating a proven duplicate.
2. Preserve the shared `ErrorOrigin` contract across HTTP, SSE, WebSocket,
   local SQLite, server SQLite, UI, and exports. A provider failure must not
   become an account or Relay failure during serialization.
3. Keep cooldown, retry, response affinity, profile recovery, and tool
   continuation rules covered at their public protocol boundaries.
4. Keep server migrations append-only and prove upgrade, interrupted migration
   recovery, backup, and restore against a real server before a release claim.

## P3 - Dynamic model catalogs and client integrations

### Provider-neutral source adapter acceptance

The source catalog must remain provider-neutral. A source contributes model
capabilities, confirmed reasoning options, optional catalog prices, and an
explicit client/upstream binding; the scheduler does not branch on vendor
names. Fixture coverage exists for this contract. The remaining gates require
real endpoint and client evidence.

1. Run source discovery against real Responses and Messages endpoints and prove
   that model availability, protocol bindings, confirmed reasoning, discovered
   prices, and manual price overrides remain separate across refreshes.
2. Keep native passthrough as the default and require an explicit adapter for
   every protocol conversion.
3. Do not claim hosted tools, dynamic discovery, unsupported reasoning, or
   WebSocket bridging until an exact adapter path and regression coverage exist.
4. Before release, run a real `codex.exe` acceptance matrix for every claimed
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

## P6 - Localization, documentation, and release evidence

1. For every new UI locale, add the localized overview and three mode-specific
   Help documents, then register the mode documents in Help Center.
2. Regenerate screenshots from Playwright after a material layout or
   terminology change.
3. Run all relevant checks, review generated assets, and perform the P0 live
   acceptance before a release claim.
