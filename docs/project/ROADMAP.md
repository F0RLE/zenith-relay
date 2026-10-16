# Zenith Relay roadmap

This file lists unfinished product work. It is not a release checklist and does
not claim that a feature exists. Implemented behavior is documented in
[PLANNING.md](PLANNING.md); contributor and release checks are in
[CONTRIBUTING.md](../../CONTRIBUTING.md).

Relay currently ships as a local-first, user-owned pool. The account connector
has separate native ChatGPT and Excel / Basis Points sign-ins. Other providers
can be configured as generic API sources when their API contract is known; a
subscription connector needs its own authentication, refresh, entitlement,
quota, usage, and recovery work.

## Hosted multi-user Relay

The primary direction is a hosted service with one Relay identity across the
website, Telegram, and future entry channels. Users manage keys, grants,
balance, and usage through the shared account API; compatible applications
connect directly to the inference API. The desktop Relay is the operator's
management client for pools, provider connections, models, user access, and
activity.

The target design, request lifecycle, ownership model, accounting rules,
security boundaries, import flow, deployment shape, and acceptance criteria
are specified in
[HOSTED_RELAY_ARCHITECTURE.md](HOSTED_RELAY_ARCHITECTURE.md). That document is
the architecture proposal, not a claim that hosted multi-user features exist.

### Delivery stages

The implementation order is intentionally dependency-first:

1. **Relay-core foundation only:** take the local P0 work that the hosted
   service requires: retain multiple upstream contracts for one source/model,
   select the exact native upstream, and keep logical-request /
   physical-attempt accounting and stream usage semantics consistent. These
   are shared engine contracts, not a requirement to finish every local
   connector.
2. **Hosted foundation:** close the hosted product and threat-model decisions,
   then build PostgreSQL identity, workspaces, roles, audit, vault references,
   and device-paired Desktop Relay management. Include scoped request
   idempotency, continuation ownership/checkpoint references, and the explicit
   encrypted-state retention policy in the initial schema.
3. **Hosted pool and catalog:** add versioned pool management, route and
   capability validation, workspace grants, model/context projections,
   ingress bridges, native-upstream enforcement, and OAuth `stage_and_drain`
   transfer semantics.
4. **Hosted accounting:** implement the append-only ledger, two-stage input /
   output reservations, finite reconciliation TTL, settlement, refunds,
   adjustments, and idempotent payment events. Use short `READ COMMITTED`
   transactions with guarded wallet updates, reservation transitions, and
   journal postings committed together. Preserve customer-estimate versus
   provider-expense uncertainty, including hidden reasoning and cache usage.
   Prove concurrent balance safety and crash recovery before paid traffic.
5. **Hosted inference:** add inference-key auth, rate/concurrency admission,
   route-scoped execution through `relay-core`, streaming, usage attribution,
   and tenant-safe model and usage APIs. Add protocol-shaped duplicate errors
   with explicit retry policy rather than empty success responses, and durable
   completed-turn continuation recovery without replaying uncertain attempts.
6. **Entry channels and transfer:** connect the website and existing Telegram
   entry/payment flows to the same account APIs; add previewed local-to-hosted
   and authorized hosted-to-local pool transfer.
7. **Release operations:** complete Docker deployment, HTTPS, backups,
   restore drills, migration recovery, security review, measured load limits,
   and real-client/provider acceptance.

Excel / Basis Points deep acceptance and rare connector conversions remain
separately demand-gated. They must still be correct for local use, but
subscription and BPS credentials are not assumed to be eligible for a shared
paid Hosted pool. Their extended acceptance therefore does not block the
first commercial Hosted release unless a specific permission review enables
that credential class.

Do not expose paid inference until key isolation, model grants, bounded spend,
settlement idempotency, usage attribution, and recovery pass their acceptance
tests. Include real-PostgreSQL single-wallet contention/rollback tests, SDK
duplicate-retry tests, estimated-usage uncertainty, and restart recovery of
completed turns awaiting tools; detailed gates are in the
[hosted acceptance contract](HOSTED_RELAY_ARCHITECTURE.md#delivery-and-acceptance).
The first release does not require distributed multi-server scheduling.
Provider sharing, fingerprint spoofing, account resale, and silent transfer of
opaque provider conversations remain outside the product contract.

The P0/P1 work below tracks the existing local and single-owner server product.
The selected Relay-core items in stage 1 above are a prerequisite for Hosted;
the remaining local connector polish remains valid maintenance but is not a
prerequisite sequence for the hosted delivery stages.

## Local runtime P0 — finish and accept the current runtime

This track proves the existing local and user-managed server paths with
permitted accounts and real installed clients.

### Model, provider, and harness compatibility

Extend the existing four protocol implementations into a reliable path from
each configured model source, through the pool, to clients that support
Responses, Chat Completions, Messages, or Gemini. The first routing slice now
retains multiple declared upstream contracts for one source/model and selects
the model's native upstream independently of the harness while keeping one
physical pool member.
Complete the remaining stages below in order; the implemented contract is
documented in [PLANNING.md](PLANNING.md).

The client chooses the protocol it actually sends. Validated model identity
selects the native upstream: OpenAI Responses, Anthropic Messages, Google
Gemini, and xAI/Z.ai Chat Completions. Source evidence must support that
endpoint and the requested features. An incompatible source has no route.
A native route still passes through pool admission, authorization, execution,
stream validation, usage, and statistics. A converted route uses the existing
typed adapters. Neither path may invent a model capability, substitute a
different model, or silently discard a required feature to obtain success.
HTTP, SSE, and WebSocket support are separate from the API protocol.

Codex's Responses input selects the bridge to the model's native upstream;
it does not select a Responses endpoint for a foreign model. Choose from the
exact source/model contracts; protocol age, an
OpenAI-compatible label, or a model family is not proof of endpoint support.
The existence of `/responses` alone does not establish support for provider
response IDs, developer instructions, custom tools, or reasoning continuation.

Before changing defaults, capture known working paths and reproducible adapter
failures as synthetic regression cases. Compare native Responses with the
corresponding Chat bridge where both are supported. Attribute failures to the
actual stage and contract mismatch rather than assuming Chat itself is broken.

#### Unified reference catalog and automatic refresh

Build one generated Relay model catalog from independently refreshed public
sources instead of maintaining provider/model rows by hand. The current
loader already refreshes and validates `models_dev`, `models_dev_details`,
OpenRouter, and LiteLLM, keeps source revisions and freshness, uses HTTP
validators, retries failures, and reconstructs one in-memory catalog from the
validated cache. Extend this pipeline rather than adding a second catalog
format.

- Normalize each source into a common record with a stable source ID,
  canonical model identity, aliases, provider namespace, capabilities,
  limits, pricing references, protocol recommendations, cache semantics, and
  field-level provenance. Keep the raw source envelope separately for
  diagnostics and refresh; the merged record is a derived view.
- Use deterministic precedence per field, preserve explicit `false`, retain
  conflicting values with provenance when they cannot be reconciled, and keep
  missing or stale values unknown. A source outage must not erase the last
  valid record; a replacement source must not silently change a canonical
  identity.
- Generate `model-catalog.json` and its schema atomically from the merged
  catalog. Consumers read the generated file or the same validated snapshot;
  no runtime code edits generated JSON. Versioned manual policy remains only
  for presentation order, aliases, and reviewed corrections that no public
  source can establish.
- Keep provider inventory and route eligibility separate. A provider's
  public model list, LiteLLM price row, or OpenRouter record can enrich the
  reference catalog but cannot prove that a user's endpoint, credential,
  protocol, tool set, or quota can execute that model.
- Let configured provider sources contribute a scoped runtime overlay:
  source/model/endpoint/protocol/features, evidence status, credential
  revision, and observation time. Merge it with the public catalog for route
  selection without writing private inventory, credentials, or usage into the
  public model JSON.
- Expose per-source status, revision, fetched time, stale state, and rejected
  update reason. On refresh failure continue from the last validated snapshot;
  only a first-load failure with no valid snapshot should leave the catalog
  unavailable.

Acceptance: a scheduled refresh adds a new model, enriches an existing model,
and updates a changed price or capability without manual edits; a malformed or
stale source is isolated while other sources remain usable; aliases merge to
one canonical record; a source endpoint can be unavailable without removing
its model from the reference catalog or making that model routable.

#### 1. Preserve the available routes for each source and model

- Retain multiple declared upstream protocols for a source/model instead of
  collapsing them to one before projecting client APIs. Keep endpoint support,
  model access, and individual features distinct, including their origin and
  source configuration/credential revision.
- Add optional ordered protocol preferences to the shared reference metadata
  and its versioned public catalog. These are recommendations, never source
  endpoint declarations. Reuse one policy for runtime and export, with its
  provenance and revision; update the schema and consumers together. Keep
  display ordering separate, and author policy before export instead of
  manually editing the generated catalog.
- Keep supported cache controls and documented cache scope on the exact
  service/model/endpoint profile. Actual cache hits, latency, failures, and
  account availability remain local observations, not public model metadata.
- Describe Responses variants explicitly: full-history input versus
  provider-owned response IDs, role semantics, function/custom/hosted tools,
  reasoning replay, stream completion, and reported usage. A declaration of
  endpoint support must not grant every feature of the OpenAI implementation.
- Preserve explicit endpoint pins and unsupported declarations. Existing
  single-endpoint sources must retain their behavior. Unknown generic services
  keep their configured fallback; model names, prices, model-list entries, and
  a successful text probe do not imply support for every endpoint or feature.
- Extend the current route identity where necessary to distinguish upstream
  protocol, endpoint, and adapter while retaining the physical pool member.
  Additional protocol routes must not multiply its weight, quota, credits,
  concurrency allowance, or share of rotation.
- Start in `sources/capabilities/config.rs`, `sources/binding.rs`,
  `sources/connector.rs`, and `runtime/build/sources.rs` under
  `crates/relay-core/src`. Extend the existing types and stores.

Acceptance: one synthetic model declared on two endpoints retains both;
unsupported and pinned combinations remain excluded; legacy configuration and
inventory survive normalization, persistence, and reload without duplication.

#### 2. Select a compatible route for each request

- Filter routes by the incoming protocol, requested operations/features, exact
  model, pool membership, and continuation ownership before dispatch. Require
  the validated native upstream within the selected member, then choose native
  forwarding or a supported conversion from the client's protocol. Preserve
  compatible explicit pins and the configured pool selection policy,
  including manual order.
- Make route selection deterministic and expose the client protocol, upstream
  protocol, native/conversion mode, and safe reason for that decision.
  Unrepresentable required input must fail before a generation is sent.
- Keep the same common execution and accounting path for native and converted
  requests. Native payloads and extensions must avoid a lossy round trip through
  the internal conversion representation. Validate required semantics on
  matching-protocol routes too; a partial Responses implementation may require
  explicit compatibility handling or rejection before dispatch.
- Pin an active response/tool continuation to its source, upstream model,
  protocol, and provider-owned state. A new independent request can select
  another route; a full history alone does not authorize replay after uncertain
  execution. Reuse the existing retry budget and ownership rules.
- Apply configuration revisions to new requests without changing an in-flight
  stream or reusing stale route evidence. If a continuation owner disappears,
  report the existing continuation error; do not silently restart its work.
- Reuse existing soft session affinity to improve cache reuse within the
  configured pool policy; do not override manual order, capacity, health, or
  continuation ownership for a hoped-for cache hit. Keep protocol selection
  stable for related requests. Do not infer a shared cache between accounts,
  endpoints, or providers from a shared model name.
- Preserve reusable prompt and tool prefixes when forwarding or converting;
  avoid gratuitous reordering or per-request additions. Apply only supported
  cache controls without changing instruction meaning. Cache preference must
  never make an otherwise incompatible route eligible.

Acceptance: every supported client reaches the same model through its native
upstream; matching clients use native forwarding and others use a bridge.
Both enter usage exactly once. Mixed routes
retain pool fairness, shared member limits, cancellation, safe retry budgets,
and continuation ownership during configuration changes. A Responses source
that only accepts full history cannot silently receive an unresolved
`previous_response_id`; working native Chat routes retain their behavior.

#### 3. Strengthen conversion and provider compatibility

- Validate the shared codec architecture defined in
  [PLANNING.md](PLANNING.md#shared-conversion-architecture) against installed
  clients and real providers in all twelve conversion directions. Keep the
  four native routes outside the representation; extend existing codecs,
  not pair-specific runtime engines.
- Extend the existing normalized types and adapter events only for concrete
  missing semantics. Cover complete JSON and streaming paths separately:
  function/custom tools, namespaces, deferred tool declarations, call/result
  pairing, parallel calls, fragmented arguments, signed reasoning, citations,
  structured output, supported media, errors, and terminal outcomes.
- Add narrowly scoped provider rules for schema dialects and model parameters.
  Keep original tool constraints; any relaxation needs explicit diagnostics
  and validation against the original contract. Preserve unknown extensions
  on native routes and reject unrepresentable required fields on bridges.
- Port deterministic custom-tool representation repairs only for declared
  tools and unambiguous inputs. Preserve tool identity, arguments, and intended
  operation; malformed output must not trigger an extra hidden generation.
- Advertise supported client tool modes accurately. Keep unavailable hosted
  tools fail-closed; any future equivalent needs an explicit capability
  contract and provider acceptance, not silent omission.
- Keep provider rules scoped to the configured service/endpoint and credential
  family. A generic compatible endpoint must not inherit unrelated provider
  quirks merely because a model name matches.

Acceptance: synthetic regressions exercise each adopted rule, ambiguous
inputs, unsupported features, and stream interruption. All four native routes
and twelve conversion directions retain their established supported behavior.

#### 4. Make accounting and failure explanations consistent

- Inspect upstream usage before output conversion and retain reported partial
  usage on failure, cancellation, or truncation. Preserve unknown counters and
  costs; do not fill them with zero or estimate tokens from response text.
- Reuse one logical request identity with separate physical-attempt history.
  Protocol alternatives and compatibility repairs must not duplicate request
  totals, cost rollups, quota consumption records, or health observations.
  Retain reported usage and cost for every actually executed attempt, including
  failed attempts; deduplicating the logical request must not erase real spend.
- Select API-equivalent pricing from the actual upstream model, service tier,
  request usage, and applicable published context/cache bands. Keep this
  estimate distinct from observed provider spend and subscription quota.
- Validate protocol recommendations using comparable workloads and reported
  cache-read/write counters, latency, errors, and applicable cost estimates.
  Missing counters remain unknown; cache-hit rate is not a model-quality
  score. Keep these measurements diagnostic in the first release, preserving
  the existing rule that API-equivalent estimates do not rank pool members.
  Do not automatically rewrite shared preferences from recent traffic.
- Extend existing redacted diagnostics to identify the failing stage: route
  selection, request conversion, provider rejection, stream validation,
  response conversion, or continuation. Do not retain raw bodies or prompts.

Acceptance: native and converted versions of equivalent synthetic usage
produce consistent statistics; incomplete and missing usage remain visible;
retries and alternate protocols do not inflate totals. Desktop and server
produce the same core result.

#### 5. Connect harnesses with accurate profiles

- Build on existing application integrations. A profile records protocols,
  transports, endpoint/auth configuration, and required features supported by
  that client version. A generic API client can use the public protocol
  endpoints without a named harness integration.
- For a client with one protocol, configure that endpoint and let Relay choose
  the upstream path. For a client with multiple protocols, prefer a compatible
  native path through a supported client configuration mechanism.
- Treat automatic protocol choice as a decision before a new request/session.
  Do not change the protocol inside an open stream, assume a universal
  negotiation mechanism, or move opaque reasoning/tool state between routes.
- Default new setup to automatic compatible routing while preserving explicit
  pins. Show understandable availability and failure reasons; keep protocol
  overrides in advanced configuration. Capability presentation must reflect
  the selected source and route rather than a union no single route supports.

Acceptance: two real harnesses using different protocols can share the same
pool and model. Switching supported client configuration preserves saved
settings, statistics, and unrelated conversations; incompatible continuations
fail explicitly.

#### 6. Qualify provider families and release incrementally

- Expand by protocol family: Responses/Chat services, Messages services, and
  Gemini services, followed by compatible aggregators and local endpoints.
  Add provider-specific exceptions only with an official contract and a
  reproducible case. Subscription authentication remains separate connector
  work.
- For each advertised combination, verify the exact source, model, client
  version, protocol, and transport. Run text and a full tool round trip, then
  the supported reasoning, structured output, media, cache, and continuation
  cases. Check interrupted streams, rate limits, authorization changes,
  member disable, and restart recovery.
- Use synthetic data for regression fixtures. Live qualification requires
  permitted accounts and scoped authorization; automatic discovery must not
  generate paid test traffic across an entire catalog.
- Complete the affected contributor checks and both localized Help updates
  before exposing new controls or compatibility claims. Migrations must keep
  old source settings readable and preserve explicit route choices.

Acceptance: publish support claims only for qualified combinations; distinguish
protocol implementation, local regression coverage, and real-client/provider
acceptance. Begin implementation with stages 1 and 2 as one bounded slice,
including their accounting and continuation regressions, before widening
provider coverage.

Implementation references: follow the
[API and protocol reference workflow](../../CONTRIBUTING.md#api-and-protocol-references).
Use the [official endpoint contracts](../../.agents/skills/relay-protocol-adapters/references/contracts.md)
as the specification. Optional implementation comparisons include
[New API RelayKit](https://github.com/QuantumNous/new-api/tree/42e694eb60fe3191795c00d416571ced716eb9c4/relaykit)
for conversion structure and diagnostics,
[OpenCodex schema compatibility](https://github.com/lidge-jun/opencodex/blob/33b321853aff76045617824793cc7cbc1d4084e7/src/adapters/openai-chat/tool-schema.ts)
and [custom-tool representation handling](https://github.com/lidge-jun/opencodex/blob/33b321853aff76045617824793cc7cbc1d4084e7/src/responses/apply-patch-envelope.ts)
for targeted provider/client cases, and
[CLIProxyAPI translators](https://github.com/router-for-me/CLIProxyAPI/tree/091ff27f31285ec4fd1dd5823b44f7f51fb8f48b/internal/translator)
as a second comparison. Port relevant behavior and regression cases into the
existing Rust owners, retain required attribution, and check each change
against the provider contract rather than importing an entire proxy runtime.
Check the [OpenAI Responses migration contract](https://developers.openai.com/api/docs/guides/migrate-to-responses),
[DeepSeek Responses compatibility tables](https://api-docs.deepseek.com/guides/responses_api/),
and [Groq Responses guide](https://console.groq.com/docs/responses-api)
when defining endpoint features; matching API names do not guarantee identical
continuation or tool behavior.

### Client and provider acceptance

- Verify the new provider presets with permitted live keys, including Moonshot
  USD/CNY balances, regional key mismatches, and Kimi Code discovery and tools.
  Add Kimi Code subscription and MiniMax M Plan quota readers only after
  verifying their cloud endpoint, credential scope, units, and reset semantics.
  A local Kimi CLI usage endpoint is not a cloud quota contract.
- Exercise native ChatGPT connections, Excel / Basis Points pool connections,
  and generic API sources through add, refresh, disable, remove, restore,
  model refresh, failed-write rollback, and configuration recovery.
- Verify Responses, Chat Completions, Anthropic Messages, and Gemini requests
  for every advertised source, including JSON, streaming, tools, reasoning,
  cache usage, safe pre-output retry, and a fresh turn after restart.
- Cover completed, incomplete, refusal, malformed terminal, interrupted stream,
  and late-usage outcomes separately. Check health, affinity, retry, and
  continuation state as well as the client-visible result.
- Verify parallel tools, fragmented arguments, optional Gemini call IDs, and
  signed reasoning on the next turn. Exercise both retained local state and
  missing state after restart. Unsupported meaningful extensions must fail
  conversion explicitly while native routes preserve them.
- Verify compressed requests, compaction, retained-context continuation, OAuth
  refresh, credential replacement, routing-cookie expiry, and client recovery.
- Exercise `turn_rate_limit_exceeded` in HTTP errors and stream/WebSocket events.
  Check retry delays, unchanged account availability, and no rotation or
  automatic history repair.
- Replace a native ChatGPT login with the same visible token and generation.
  Verify that old turn state and WebSocket capability observations cannot cross
  the authorization incarnation. Cover headers, bodies, metadata, 401 refresh,
  and a subsequent turn on an existing socket.
- Compare HTTP SSE and reusable native WebSocket on the same permitted account,
  model, input, and settings: first-output latency, cache usage, errors, and
  reported usage. Follow the
  [OpenAI WebSocket continuation contract](https://developers.openai.com/api/docs/guides/websocket-mode)
  for socket loss, `store=false`, and full-history recovery. Evaluate an optional
  HTTP-to-upstream-WebSocket path after these checks; transport choice alone
  does not establish better response quality.
- Exercise native fidelity with real clients: ordered tool-schema properties,
  annotated choices, precise extension numbers, repeated protocol headers,
  and opaque reasoning/tool state. Compare root and child-thread routing with
  and without explicit cache keys. Local fixtures do not prove provider
  acceptance, cache-hit improvements, or reduced model-quality degradation.
- Keep the complete source inventory separate from endpoint support. A model
  returned by a source must not disappear only because a route is unavailable.
- Use two permitted BPS connections to check rotation, proxy, quota refresh,
  cooldown/recovery, member removal, and redacted usage.

### Excel / Basis Points acceptance

- Exercise Excel PKCE sign-in and automatic callback validation in the Relay
  window, cancellation, proxies, token rotation, and targeted reauthentication
  with permitted live accounts. Check client selection and contextual Help
  without losing the pending flow.
- Verify separate Codex and Excel connections for the same principal through
  import, export, server transfer, and concurrent credential replacement.
- Check actual model entitlements, reasoning, tools, image input, usage,
  streaming, and continuation on the Excel route.
- Exercise tool-result images (inline upload and file IDs), multiple image/text
  parts, and collaboration history in installed clients. Check that attribution
  cannot become developer authority and attachments do not change turn identity.
- Verify early streamed text, atomic terminal tool release, cancellation,
  heartbeat-only timeout, malformed/absent completion, and first/last-output
  timing on real Excel streams through all supported client protocols.
- Check explicit Responses `relay_bps_output_validation` with valid/invalid
  schema output, tool turns, refusals, and streamed clients. No invalid answer
  may escape before validation; native strict must remain rejected.
- Verify generation 401 recovery on the next request with a retained Excel
  refresh token, simultaneous refresh and delayed 401, a new login during
  `invalid_grant`, and operator disable/draining preserved throughout recovery.
  Check that model permission refusals pause only the canonical model for
  30 minutes and that aliases share the pause without replaying generation.
- Exercise optional request timezone binding through native HTTP, WebSocket,
  Excel, compaction, remote persistence, and restart with installed clients.
  Check existing environment and web-search timezone fields while dates,
  ordinary text, and API-source requests remain intact. These observations
  do not establish response quality.
- Verify mixed pools of native ChatGPT, paid Excel / Basis Points, and API
  sources after import, restart, and server transfer. Native Free accounts
  remain eligible; Free BPS imports, discovery, and generation stay blocked,
  including after a live plan downgrade. Excel refusals
  must never authorize an unsafe replay or change another connection's transport.
- Compare quality under controlled conditions. A separate Excel OAuth client
  does not establish that account degradation has been eliminated.
  Use manual bounded probes only with authorized accounts and isolated routing;
  retain inconclusive samples separately and compare matched model/effort/tasks.
  Local solver tests are not live quality or account acceptance evidence.

### Concurrency and load

- Finish Responses WebSocket multiplexing: independent lanes, FIFO per lane,
  bounded queues, scoped errors, ownership, retry accounting, continuation
  forks, usage, and disconnect handling.
- Measure admission fairness and retained memory under mixed HTTP, SSE,
  WebSocket, and image traffic. Use measurements before adding caches or new
  limits.
- Complete the token-ownership, delayed-result, rollback, and concurrent-refresh
  matrix for desktop and server.

### Server acceptance

1. Pair over HTTPS with separate management and request credentials and a vault
   key.
2. Add or transfer only permitted user-owned connections and inspect redacted
   state.
3. Compare requests, quota, usage, and timing with the desktop open and closed.
4. Verify restart, upgrade, interrupted migration, backup, restore, and a
   request from the restored runtime.
5. Check management snapshots, diagnostics, usage, and support exports for
   secrets, prompts, response bodies, and authorization headers. Explicit
   account exports contain credentials and must use a no-store response.

## Local server P1 — make server publication explicit

The desktop and server already have separate configuration, credential, and
account-transfer operations. Finish a single documented workflow that:

- builds a desired-state revision and shows a diff before applying it;
- validates sources, models, protocols, capabilities, policies, and references;
- reports the active revision, validation failures, health, and rollback handle;
- distinguishes portable settings from local state, secrets, leases, and
  server-owned changes;
- requires explicit confirmation for credential or account transfer;
- never performs implicit two-way synchronization that can overwrite newer
  changes.

The workflow must remain recoverable across desktop/server restarts, interrupted
migration, a failed runtime rebuild, and a lost management connection.

## P1 — measured routing and performance work

Only optimize after measuring the affected path:

- tool catalogs across native and converted routes, JSON, SSE, WebSocket,
  continuation, and policy changes;
- policy-only updates without losing listeners, leases, affinity, or runtime
  state;
- bounded cooldown and health persistence across restart;
- startup, page open, policy save, remote pool switch, history size, latency,
  token usage, cache reuse, and tool-selection quality.

## Maintenance

Revisit these only when the affected platform or recovery path changes:

- upgrade the Tauri/GTK dependency chain as one tested platform change;
- verify application-first recovery, history repair, Windows extended paths,
  partial failure, and cleanup failure;
- test transactional bulk account changes and concurrent reasoning-policy edits
  through real client flows.

## Future connectors

Add subscription providers one at a time. Each connector needs permitted live
accounts and evidence for authentication, refresh, revocation, vault storage,
entitlements, quota units, usage, models, execution, and recovery. Do not mark
a provider as supported because a generic protocol adapter can format its
request.

## Demand-gated future work

- Add distributed server scheduling only after measured demand requires shared
  state, candidate leases, affinity, or cross-node coordination.
- Add named profiles, aliases, and groups with source-scoped identities,
  collision checks, independent ordering, enablement, and price fields.
  Presentation grouping must not change protocol or scheduler policy.
- Keep any future Zenith integration separately requested and separately
  designed. Hosted Relay owns its own identities, tariffs, wallets, payments,
  accounting, and route policy; it must not import Zenith production
  credentials, customer data, inventory, or internal Gateway/Control business
  logic.
