# Zenith Relay Roadmap

Last reviewed: 2026-09-03.

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

## Implementation status

The current product implementation gate is closed for the shipped desktop
scope: the local desktop surface, provider-neutral routing contracts, profile
recovery, quota automation, model availability handling, redaction rules, and
responsive UI
are covered by the implementation checks and review required by the release
process. The user-visible scope is recorded in
[CHANGELOG.md](../releases/CHANGELOG.md);
test counts and CI evidence are deliberately kept out of that document.

This does not turn mocked or local checks into a production claim. Live P0
acceptance remains deferred until permitted real accounts are available and the
user explicitly resumes it. The user-managed server portion is the final P0
step and must be tested after the local desktop path, not in parallel with it.

## Current order

1. The shipped implementation baseline is complete; no live production claim is
   made without the deferred P0 evidence below.
2. P0 live acceptance is deferred until permitted real accounts are available;
   the user-managed server path is last.
3. P1 is in progress: measure warm startup, page open, and pool-switch disk I/O
   before changing the loading architecture, then replace source roles with a
   unified adaptive pool scheduler and explicit manual ordering.
4. P2 is in progress: preserve reliability contracts and validate the new
   application recovery layout on upgraded installations.
5. P3 is in progress: catalog, reasoning, usage, adapter, ChatGPT, and OpenCode
   foundations are implemented; real client/provider acceptance remains open.
6. P4 and P5 remain demand-gated. Do not add speculative account connectors or
   multi-server coordination ahead of the existing acceptance gates.
7. P6 through P8 are long-term design records only. Do not start named-profile,
   grouping, or Zenith runtime-convergence work until P0-P3 are complete and
   the current Zenith production Gateway/Control API path is stable.
8. P9 remains the release-evidence gate for any production claim.

## Implementation review — 2026-09-03

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
- OpenCode connection, live pool model projection, image/reasoning metadata,
  and one-shot original-config recovery.
- Application-first recovery storage with an idempotent migration from the
  previous `recovery/profiles` and sibling-directory layout.

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

The profile-recovery follow-up is now closed in the implementation: managed
ChatGPT switching restores only Relay-owned configuration and authentication,
preserving unrelated settings and rejecting a newer manual sign-in. Separate
named recovery points capture only `config.toml` and authentication for an
explicit, confirmed restore. The history-repair path preserves its rollback
handle during cleanup, updates only SQLite threads whose rollout was actually
processed, rewrites every relevant session metadata record in either direction
between ChatGPT and Relay/API providers, and normalizes extended Windows paths
in preview and backup validation.

The remaining recovery acceptance is client-level: verify that a running
OpenCode desktop/CLI process reloads after connect, that a failed config write
leaves the original file intact, and that restore works for JSON and JSONC
files on each supported platform.

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

### Unified adaptive pool routing

Replace the API-first, stabilizer, and reserve source roles with one routing
contract shared by subscription accounts and API sources. The scheduler must
remain provider-neutral: a logical pool member owns routing policy, while any
protocol-specific candidates derived from that member remain internal runtime
details. The redesign must preserve the current response-ownership, streaming,
failure-isolation, and redaction guarantees.

#### Routing modes and ordering

Expose three user-facing routing modes:

1. **Smart** is the default adaptive mode. It combines manual preference,
   quota headroom, capacity, observed reliability, latency, reset timing, and
   optional cost evidence without allowing one noisy sample to monopolize the
   pool.
2. **In order** selects the first eligible member in the saved manual order and
   proceeds to the next member only when the current request can safely fail
   over.
3. **Round robin** distributes new sessions across eligible members. Requests
   belonging to an already assigned session remain on their affinity owner
   while that owner is usable.

Store one atomic order containing tagged member references such as
`account:<id>` and `source:<id>`. Accounts and sources appear in the same list,
can be reordered together, and do not expose magic numeric role thresholds.
An API source with multiple protocol bindings inherits its logical source
position; its internal candidates must not become separate user-visible rows.

New members are appended deterministically. Deleted references are ignored and
removed on the next successful policy save. A temporarily unavailable member
keeps its position so that recovery does not silently rewrite user policy.
Reordering applies immediately to new sessions and safe retry selection, but
never interrupts an in-flight request or moves an upstream-owned continuation.

#### Selection pipeline

Apply routing in this order:

1. Resolve mandatory response ownership and active connection affinity.
2. Filter candidates by enabled and draining state, secret and proxy
   availability, requested model, client and upstream protocol, adapter
   capability, request lane, quota, concurrency, cooldown, and circuit state.
3. Apply explicit cache-key and session affinity when the bound candidate still
   has sufficient health, quota, and capacity.
4. Rank the remaining eligible candidates with the selected routing mode.
5. Attempt bounded fallback only while no response bytes have been committed
   and the request or continuation contract permits another candidate.

`previous_response_id` ownership and an active WebSocket connection are hard
affinities. Prompt-cache and ordinary session affinity are strong preferences
with guarded escape conditions. Create or replace a soft affinity binding only
after a request has reached a verified successful response boundary, so a
failed first attempt cannot pin the session to a bad candidate.

Key every affinity by the minimum complete routing scope, including the client
scope, canonical public model, request lane, and protocol where required. A
candidate recovering at a higher preference may receive new sessions
immediately, but it must not steal an existing healthy session from another
candidate.

#### Smart score

Maintain normalized `0..1` factors for each eligible candidate and calculate a
bounded score from:

- manual priority;
- reported quota headroom;
- free parallel capacity and queue depth;
- error-rate EWMA;
- time-to-first-output EWMA;
- quota or subscription reset urgency;
- confirmed upstream cost evidence when the active profile explicitly enables
  cost-aware routing;
- response, cache, connection, and session affinity bonuses.

Record operational observations at candidate, canonical model, protocol, and
request-lane scope so that a slow or failing route cannot penalize an unrelated
model or binding from the same logical source. Clamp samples, require a minimum
observation count, decay stale data, and add hysteresis before moving a soft
affinity. Unknown quota, latency, cost, or reset data is neutral rather than
zero; incomplete evidence must neither win nor lose a route by accident.

Offer routing profiles instead of exposing raw coefficients by default:

- **Cache** emphasizes affinity and stability and is the Relay default;
- **Balanced** combines quota, capacity, reliability, latency, and manual
  preference;
- **Speed** emphasizes first-output latency, error rate, and available
  capacity;
- **Economy** enables confirmed cost and reset-use factors;
- **Custom** exposes validated bounded coefficients in advanced settings.

The current contract that prices do not affect routing remains true until the
Economy/Custom implementation, evidence rules, UI disclosure, tests, and
stable documentation are complete. Provider-discovered cost, reference API
price, account API-equivalent estimates, and user purchase cost must remain
separate facts; purchase cost and payback never become scheduler inputs.

#### Top-K distribution

Smart routing ranks the eligible set, retains a bounded Top-K, and distributes
new assignments inside that set so that the current highest score does not
receive all traffic indefinitely. The default Top-K is three and is bounded by
the actual eligible count.

Use a stable session-derived choice when a trustworthy session key exists, so
the same session returns to the same candidate and availability changes remap
only affected sessions. Use smooth weighted round robin for unscoped requests.
Candidate weight defaults to one, is validated and bounded, and affects only
distribution inside the selected Top-K; zero weight excludes a candidate from
normal selection without replacing the explicit enabled state. Weight changes
must reset or safely rebase accumulated rotation credit.

#### Failure feedback and recovery

Classify every failed attempt before mutating scheduler state:

- client/request validation failures are terminal and do not penalize a
  candidate;
- unsupported or disabled models cool down only the exact candidate and model;
- quota, authentication, payment, or credential failures cool down the exact
  physical slot at the narrowest proven scope;
- safe pre-response overload, timeout, transport, and generic upstream
  rejection may continue to the next eligible candidate;
- failures after response commitment never start a transparent replay;
- upstream-owned continuations move only after an explicitly proven recoverable
  affinity miss.

Track circuit state as `closed`, `open`, and `half-open`. An expired cooldown
admits a bounded probe before the candidate re-enters normal Top-K selection.
Persist enough timestamped cooldown and recent-health state to avoid a restart
storm, but expire stale observations and never let an old snapshot disable a
candidate indefinitely. Provider/model storm protection remains scoped so that
one failing credential, model, or binding cannot disable unrelated routes.

#### Runtime metrics and diagnostics

Maintain bounded rolling state for dispatches, active requests, queue depth,
success and error EWMA, first-output EWMA, circuit state, affinity hits and
escapes, fallback attempts, and score inputs. Hot policy changes must update the
running scheduler without reopening the listener or discarding active leases,
affinity, or safe health state.

For every request, expose a redacted routing trace containing:

- selected logical member and physical candidate;
- selection mode, profile, final score, and decisive factors;
- affinity type and hit, miss, or escape reason;
- attempted fallback chain and classified rejection reasons;
- cooldown and half-open transitions;
- the policy and runtime revisions used for the decision.

Never retain prompts, response bodies, credentials, cookies, authorization
headers, or raw provider error bodies. Replace the unconditional **Next
candidate** presentation with factual **In use** activity and a model-scoped
**Expected route** preview. A preview must accept a concrete model, protocol,
lane, and client scope and clearly remain a simulation rather than a promise
about the next request.

#### Desktop and server UI

Move all cross-member routing controls into the pool distribution dialog. Show
one draggable list containing accounts and API sources with name, kind,
availability, supported-model count, active-request state, cooldown, and manual
position. Keep source protocol, model, price, and recovery settings in the
source editor; remove source-role selection and API-only order controls from
the member editor.

The distribution dialog contains the Smart, In order, and Round robin mode
control; the Smart profile control; the unified order; and an advanced section
for Top-K, bounded traffic weight, affinity escape thresholds, and Custom
coefficients. The default surface must stay understandable without opening the
advanced section. Local and user-managed server modes render and mutate the
same versioned routing contract, subject to negotiated server capabilities.

#### Migration and compatibility

Add an append-only schema migration and a versioned management contract. Derive
the first unified order from the existing effective path: API-first sources in
their saved order, subscription accounts, stabilizer sources, then reserve
sources. Preserve existing account behavior during migration rather than
turning a formerly balanced account set into an accidental strict chain.

Continue accepting legacy role priorities and routing-strategy values in old
imports and older server payloads long enough to produce an explicit preview,
but emit only the new contract after a successful conversion. Import preview,
revision CAS, runtime rebuild, rollback, backup, restore, and redacted export
must cover the unified order and all scheduler settings. A failed conversion or
runtime rebuild leaves the previous verified policy active.

After local/server compatibility is complete, remove the API role constants,
role inference, source-role UI, stale diagnostic reason variants, unused CSS
and translations, and any legacy weight path that is not connected to the new
Top-K implementation. Do not retain two schedulers or two active routing
contracts.

#### Implementation and acceptance order

1. Specify the versioned routing contract, score semantics, affinity scopes,
   error classes, migration mapping, and rollback behavior.
2. Add shared protocol types and append-only local/server migrations with
   round-trip, old-import, backup, restore, and interrupted-upgrade tests.
3. Refactor eligibility into a provider-neutral candidate pipeline shared by
   every routing mode.
4. Implement normalized observations, EWMA decay, routing profiles, scoring,
   Top-K selection, stable session assignment, and smooth weighted rotation.
5. Implement typed failure feedback, scoped circuit breakers, persisted
   cooldown recovery, half-open probes, and affinity escape hysteresis.
6. Add atomic management operations and hot-apply behavior for desktop and
   user-managed server runtimes.
7. Replace the role UI with the unified order, routing modes, profiles,
   advanced controls, model-scoped route preview, and factual activity state.
8. Add deterministic unit and property tests for ordering, score bounds,
   unknown evidence, weight changes, Top-K membership, minimal remapping,
   concurrency, quota refresh, cooldown recovery, and live policy updates.
9. Add gateway tests for every error class, bounded fallback, partial streams,
   Responses ownership, WebSocket affinity, adapter routes, and one unhealthy
   slot among otherwise healthy members.
10. Add local/server integration, configuration migration, redaction,
    performance, and Playwright coverage; then remove the superseded role and
    scheduler legacy in the same delivery series.

Do not claim this P1 routing work complete until local and user-managed server
runtimes produce the same deterministic decisions from the same snapshot and
policy, cache-affinity behavior has measurable evidence, and no policy-only
change restarts the listener or loses active runtime state.

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
   account API-equivalent estimates on the LiteLLM exact-record path within
   the declared official family, while API sources resolve provider evidence,
   LiteLLM exact, declared-family canonical, then manual price.
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

OpenCode configuration integration is shipped. The remaining acceptance work is
to run it against supported desktop/CLI installations on each platform and
prove model refresh, image input, reasoning variants, restart behavior, and
restore after a failed write. Future applications must use the same shared pool
endpoint and reversible storage boundary; they must not fork routing or secret
storage.

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
6. Keep LiteLLM catalog refresh independent from request execution: load the
   last valid local snapshot before the first runtime snapshot, use stale data
   offline, validate before atomic replacement, and invalidate derived totals
   by catalog/policy revision. Never reintroduce a hand-maintained OpenAI price
   file or let a missing price become `$0`.

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
