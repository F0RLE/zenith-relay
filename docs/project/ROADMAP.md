# Zenith Relay Roadmap

Last reviewed: 2026-08-23.

This roadmap contains only remaining acceptance gates and future work. It does
not repeat completed implementation history. A phase is complete only when its
verification evidence exists, not when its UI is present.

## Security boundary

Every acceptance phase preserves the product boundary: Zenith Relay is a
separate personal deployment, not a client of the production Zenith Gateway or
Control API. Production credentials, customer keys, backend tokens, account
inventory, and internal production routing/business logic must never be copied
into Relay, its fixtures, or its documentation.

User-owned credentials may move from desktop storage only to a server the same
user operates, after an explicit management confirmation. State, usage,
telemetry, diagnostics, exports, and API snapshots must remain redacted. Any
future feature that cannot prove this boundary is out of scope.

## Release baseline - v1.1.0

The product implementation gate is closed for the 1.1.0 release: the local
desktop surface, provider-neutral routing contracts, profile recovery, quota
automation, model availability handling, redaction rules, and responsive UI
are covered by the implementation checks and review required by the release
process. The user-visible scope is recorded in
[CHANGELOG.md](../releases/CHANGELOG.md);
test counts and CI evidence are deliberately kept out of that document.

This does not turn mocked or local checks into a production claim. Live P0
acceptance remains deferred until permitted real accounts are available and the
user explicitly resumes it. The user-managed server portion is the final P0
step and must be tested after the local desktop path, not in parallel with it.

## Current order

1. The v1.1.0 implementation baseline is complete; no live production claim is
   made without the deferred P0 evidence below.
2. P0 live acceptance is deferred until permitted real accounts are available;
   the user-managed server path is last.
3. P1 is in progress: explain the reported slow pool switch with warm timing
   and disk-I/O evidence before changing the loading architecture.
4. P2 is in progress: keep the shared reliability contracts and close the
   remaining `repair` consistency risks.
5. P3 is in progress: the catalog, reasoning, usage, and adapter foundations
   are implemented, but real client/provider acceptance evidence is still open.
6. P4 and P5 remain demand-gated. Do not add speculative account connectors or
   multi-server coordination ahead of the existing acceptance gates.
7. P6 through P8 are long-term design records only. Do not start named-profile,
   grouping, or Zenith runtime-convergence work until P0-P3 are complete and
   the current Zenith production Gateway/Control API path is stable.
8. P9 remains the release-evidence gate for any production claim.

## Review record — 2026-08-20

The previous review was stale by one implementation cycle. The current tree
already contains the following foundations, so they are no longer described as
unstarted work:

- performance samples for startup, page open, mode switch, and full snapshots;
- usage and generation-throughput diagnostics, including protocol, cache, and
  reasoning fields;
- declared reasoning defaults and manual model-group overrides;
- semantic model ordering, provider/launcher presentation grouping, and
  explicit `Responses -> Gemini` route support;
- configuration preset export, preview, validation, revision CAS, runtime
  rebuild, and rollback.
- incremental API-equivalent totals backed by the existing usage rollup,
  including migration-safe handling of retained request logs.

These are implementation and fixture results, not production acceptance. The
remaining release blockers are real-account pool/server runs, real
Responses/Messages/client compatibility evidence, and the unmeasured warm
pool-switch and disk-I/O path. The review found four `repair` risks; the
history-repair follow-up below now closes those consistency cases. The
follow-up audit found two more open issues: catalog-refresh failures were
discarded by startup/restart/background callers, bulk account deletion could
stop after partially deleting the batch. Both implementation follow-ups are
now closed; live provider and client acceptance
remains open.

The profile-recovery follow-up is now closed in the implementation: the first
local startup creates one durable original snapshot and orders it first;
manual snapshots remain available at any time; restore is full-only and never
creates a hidden pre-restore copy; and deletion uses an explicit ten-second
confirmation cooldown. Snapshot metadata also stores portable Windows paths.
The history-repair path now preserves its rollback handle during cleanup,
updates only SQLite threads whose rollout was actually processed, rewrites
every relevant session metadata record, and normalizes extended Windows paths
in preview and backup validation.

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
5. Confirm Usage, snapshots, and telemetry record the selected member, timings,
   token split, and classified result without exposing a secret or prompt body.

### User-managed server acceptance

1. Deploy the server behind HTTPS with a separate management token and vault
   key, then attach the managed ChatGPT/Codex profile.
2. Transfer or create only user-owned permitted test connections through the
   desktop management path and verify the redacted server capability snapshot;
   do not use Zenith production credentials or production account inventory.
3. Send a streaming <code>/v1</code> request through the server.
4. Close the desktop app, send another request, then reopen the app.
5. Compare the server quota, account state, request count, token totals,
   timings, and API-equivalent usage with the desktop view.
6. Take a backup, restore it to a clean test location, and prove that the
   restored runtime passes a request.
7. Inspect state, usage, source statistics, diagnostics, and exports to confirm
   that they contain no raw credentials, cookies, prompts, or provider bodies.

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
The browser regression records page and mode timing, but it does not explain
the user's slow pool switch or measure file/SQLite/JSONL bytes read. No warm
or representative local/remote disk-I/O baseline has been accepted yet.

1. Measure warm startup, mode switch, Connections, Usage, and policy-save
   latency with a representative account set. Measure local/remote pool
   switching separately, including wall-clock phases, disk bytes read, and
   history/SQLite/rollout reads; identify any synchronous reread that causes
   gigabytes of disk I/O.
2. Prove by regression test that policy-only source and account edits do not
   reopen the local listener or discard active state; endpoint, port, and
   secret changes may rebuild. Existing hot-apply code and UI coverage do not
   yet prove this listener/state invariant.
3. Skip identical full snapshots when the state revision has not changed. The
   shared desktop refresh path now keeps explicit/manual refreshes forced while
   periodic background refreshes return without invoking the native snapshot
   command for an unchanged revision.
4. Turn a measured regression into a small reproducible check before
   optimizing it.

The goal is a responsive application, not speculative caching or background
work that makes account state stale.

## P2 - Preserve reliability and ownership boundaries

The broad ownership cleanup is complete. Do not reopen it for cosmetic module
moves. Continue only where a new regression shows two owners for the same
behavior or where the repair review below violates a shared ownership or
rollback contract.

1. Keep local and remote account mutations on their canonical transactional
   paths and add a regression test before consolidating a proven duplicate.
2. Preserve the shared `ErrorOrigin` contract across HTTP, SSE, WebSocket,
   local SQLite, server SQLite, UI, and exports. A provider failure must not
   become an account or Relay failure during serialization.
3. Keep cooldown, retry, response affinity, profile recovery, and tool
   continuation rules covered at their public protocol boundaries.
4. Keep server migrations append-only and prove upgrade, interrupted migration
   recovery, backup, and restore against a real server before a release claim.
5. Keep `repair` atomic and reversible. The current implementation retains the
   backup/rollback handle when post-apply cleanup fails, updates SQLite threads
   only for rollouts actually processed, replaces every relevant `session_meta`,
   and normalizes Windows extended paths before writing recovery manifests;
   regression coverage now exists for the latter three consistency cases.
6. Keep bulk account deletion preflighted and transactional. The command
   validates every selected account before mutation, commits account state and
   usage telemetry in one SQLite transaction, and restores reversible side
   effects when preparation or commit fails. Regression coverage covers
   multi-account telemetry cleanup and rollback.
7. Serialize manual reasoning edits against the latest policy revision. The
   dialog locks overlapping mutations; unit and browser coverage keep this
   ordering explicit.

## P3 - Dynamic model catalogs and client integrations

The implementation is ahead of the previous review: reasoning policy,
semantic catalog presentation, usage evidence, and the Gemini bridge have
fixture and browser coverage. This phase remains open because no fixture can
replace a permitted live provider/client acceptance run.

### Provider-neutral source adapter acceptance

The source catalog must remain provider-neutral. A source contributes model
capabilities, non-authoritative reasoning hints, optional catalog prices, and
an explicit client/upstream binding; the scheduler does not branch on vendor
names. Manual reasoning policy is a model-group setting; reasoning modes are
catalog metadata and are never checked with a separate probe. Fixture coverage
exists for this contract. The remaining gates require real endpoint and client
evidence.

1. Run source discovery against real Responses and Messages endpoints and prove
   that model availability, protocol bindings, reasoning hints, discovered
   prices, and manual price overrides remain separate across refreshes. Keep
   account API-equivalent estimates on the official account catalog path, while
   API sources resolve provider evidence, official fallback, then manual price.
2. Keep native passthrough as the provider contract and use a named adapter for
   every protocol conversion. A confirmed native Messages binding may derive
   the existing Responses-to-Messages runtime route; it must not be treated as
   evidence for an unrelated upstream protocol.
    `ResponsesToGemini` now covers discovered `generateContent` models,
    multimodal input, function/namespace/custom tools, thinking, usage, local
    continuation, and JSON/SSE streaming including Vertex partial arguments.
    Keep provider-managed caching, hosted tools, and WebSocket bridging out of
    the binding until each has its own exact adapter path and acceptance proof.
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
4. Keep catalog refresh failures and a Codex-running deferral visible to the
   caller. Startup, restart, and background paths now persist a bounded
   warning in the local snapshot; live profile acceptance must still prove
   that the previous verified catalog remains usable after a failed refresh.
5. Keep availability acceptance provider- and adapter-specific. HTTP 2xx is
   treated as reachability only; model publication still requires semantic
   availability and price evidence. Reasoning modes remain catalog metadata and
   never gate publication. Fixtures cover native Responses and bridge routes;
   live provider acceptance remains required.

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

### Subscription-account accounting contract (deferred)

When work on a permitted subscription connector resumes, model the provider's
actual account entitlement instead of treating a subscription as token-priced
API usage.

1. Keep four facts separate for every account: the provider-reported
   entitlement, period, and reset; observed account usage; Relay's
   API-equivalent estimate; and actual billable upstream/API spend when it
   exists. Each record needs a redacted source, unit, time interval, freshness,
   and confidence level.
2. Use a provider's live, documented or observed quota only after an acceptance
   run proves its units and reset behavior. Keep that entitlement in its native
   units; do not project a remaining monetary value from an available
   percentage. Relay's API-equivalent remains a direct estimate from recorded
   token usage and must never be presented as a provider debit.
3. Preserve the distinction across local SQLite, Relay Server, runtime
   snapshots, UI, and exports. Personal-pool account accounting must not become
   Zenith customer billing, customer debit, or a source of public API prices.
4. Add fixtures and permitted live-account acceptance for ordinary requests,
   cached and reasoning-token usage where reported, failures, refreshes, quota
   resets, restart recovery, and the single quota-calculation display. Never
   infer a missing provider counter from a different provider's formula.

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

## P6 - Named pool profiles and explicit server publication (future)

Configuration preset export, preview, validation, revision CAS, runtime
rebuild, and rollback already exist. Build named profiles on that contract;
do not introduce a parallel source/account configuration format. This phase
starts only when named profiles or multiple publication targets are actually
needed.

1. Store named profile metadata and immutable profile revisions separately from
   the active runtime. A revision contains source/account rules, routing and
   quota policy, model visibility, reasoning policy, price overrides, aliases,
   and display metadata, but never credentials or vault material.
2. Track active local revision and published revision independently for every
   connected server target. Editing, selecting, or publishing one must not
   mutate another target implicitly.
3. Add the desktop workflow: create or duplicate profile, edit draft, preview
   local or remote diff, validate target references and capabilities, publish
   with expected-revision CAS, verify the rebuilt runtime, and roll back to the
   previous verified revision.
4. Missing accounts, sources, proxies, secret references, bindings, or target
   features make publication fail closed. Secret transfer remains a separate,
   explicit management operation with its own confirmation and audit result.
5. Add tests for local/server independence, stale publication, missing target
   references, runtime rebuild rollback, restart persistence, secret-free
   exports, and switching profiles while requests are in flight.

## P7 - Model identity, ordering, aliases, and provider groups (future; partial)

Semantic sorting and provider/launcher grouping now exist as presentation
helpers. They are not durable profile metadata, a user-editable rank, or a
complete alias contract; keep those distinctions explicit.

1. Separate upstream model id, canonical client id, and localized display name.
   Alias rules are explicit and scoped to a source/binding; reject cycles and
   ambiguous collisions. Provider/date heuristics may produce reviewable
   suggestions only.
2. Add independent model fields for enabled state, display rank, canonical
   alias, and price override. Drag-and-drop edits display rank without changing
   source priority, scheduler order, or price; current semantic ordering is
   presentation-only.
3. When named profiles exist, move launcher grouping out of static UI family
   guesses into optional profile presentation metadata. A group can collect
   models and provider sources, but route eligibility still comes from current
   capabilities and health.
4. Model protocol, prompt-cache, quota, usage, and price dimensions explicitly
   per binding. Claude-style cache write/read behavior must not leak into other
   providers, and provider names must not become scheduler branches.
5. Keep manual prices when discovery has no trusted parser. When an adapter
   reports prices, retain evidence, currency/unit, freshness, and the manual
   override separately. Relay personal API-equivalent values remain
   informational, not customer billing prices.

## P8 - Optional Zenith runtime convergence (future)

Relay Server is currently a personal single-deployment runtime, not a public
multi-user billing gateway. Convergence proceeds only through explicit gates:

1. Publish a contract matrix against Zenith Gateway for all supported HTTP and
   streaming protocols, tools, images, reasoning, prompt caching, errors,
   routing, health, usage evidence, and readiness.
2. Add a shadow comparison that cannot dispatch duplicate paid requests or
   write customer usage. Resolve every catalog/route disagreement at the owner
   of the shared contract.
3. Run an explicit canary where Zenith Gateway keeps customer auth, pricing,
   reservations, durable debit/settlement, and public error policy while Relay
   supplies only provider/account scheduling and execution.
4. Require production evidence for restart recovery, configuration rollback,
   partial streams, upstream outages, secret rotation, backup/restore,
   zero-downtime deployment, and bounded observability labels.
5. Retire the experimental `zenith-account-pool` only after Relay covers its
   intended owned-account path. Replace or thin Gateway only after Relay has an
   approved multi-user isolation and durability design; do not move Control API
   balances, orders, ledger, or customer pricing into the personal server.

## P9 - Localization, documentation, and release evidence

1. For every new UI locale, add one sequential Help guide at
   `docs/help/<locale>/README.md`, register it in Help Center, and keep its
   section order aligned with the application.
2. Regenerate screenshots from Playwright after a material layout or
   terminology change.
3. Keep [CHANGELOG.md](../releases/CHANGELOG.md) current: describe review-ready work under
   `Unreleased`, then move only shipped behavior into a dated tag section.
4. Run all relevant checks, review generated assets, and perform the P0 live
   acceptance before a release claim.
