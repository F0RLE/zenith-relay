# Zenith Relay architecture

Current product contracts and limits. Implementation lives in source/tests;
unfinished work and live acceptance are in [ROADMAP.md](ROADMAP.md). Check
commands are in [CONTRIBUTING.md](../../CONTRIBUTING.md), user steps in Help.

## Product and ownership

| Mode | Runtime and data |
| --- | --- |
| This computer | Desktop process, loopback endpoint, local protected storage |
| Choose API | Selected compatible API and its local secret reference |
| My server | User-operated Relay Server, encrypted vault, SQLite |

Relay is separate from production Zenith. A personal Zenith API key is an
ordinary user source; production credentials, customer inventory, and internal
Gateway/Control business logic never enter Relay. Transfer to the user's own
server is an explicit confirmed operation, not automatic synchronization.
Connectors use permitted authentication and automation paths. Relay does not
implement fingerprint/IP spoofing or sharing-concealment features to evade
upstream controls; a loopback endpoint does not authorize account resale.

| Path | Responsibility |
| --- | --- |
| `src/src` | React rendering, i18n, UI state, typed Tauri wrappers |
| `src-tauri/src` | Desktop I/O, credentials, OAuth, profiles, process lifecycle |
| `crates/relay-core` | Shared discovery, scheduling, protocols, gateway, quota, usage |
| `relay-server` | User-managed runtime, vault, persistence, management API |

Desktop and server share runtime contracts. React does not read secrets/files,
call providers, or implement routing. Closing the main window hides and reuses
its WebView while tray/background runtime survives; explicitly quitting the
process stops the local pool. Distributed multi-server coordination is not
implemented.

Renderer snapshot reads accept only the latest requested result. Mode changes
and explicit refreshes retire previous background reads and retries without
blocking new updates; events arriving during a read trigger one coalesced
follow-up. Visible remote runtime pages also poll once per minute and on focus
because server changes do not emit desktop events. Inactive pages do not poll
full runtime snapshots.

`relay-core/error_codes` defines stable error identifiers and upstream defaults
for public codes, HTTP status, and safe messages. Desktop and server reuse these
definitions; domain modules retain classification, retry, and recovery decisions.
Unknown provider codes remain distinct. Catalog tests require both localized
Help references to document each code and its public alias. Help renders those
same Markdown tables as searchable groups, loading only the selected local
language asset; it does not maintain a separate troubleshooting catalog.

## Storage and credentials

Desktop state uses the platform local-data location: normally
`%LOCALAPPDATA%\Zenith Relay` on Windows,
`~/Library/Application Support/Zenith Relay` on macOS, and
`$XDG_DATA_HOME/Zenith Relay` (normally `~/.local/share/Zenith Relay`) on Linux.
Within it, `data/database`, `data/vault`, `data/catalogs`, and
`data/migrations` keep durable Relay state separate; `cache` holds temporary
imports, OAuth state, locks, and the WebView profile; `exports` holds generated
deployment bundles; `logs/errors`, `logs/crashes`, and `logs/operations` hold
bounded redacted diagnostics. A tiny redacted last-stage marker survives an
interrupted session and is cleared after a clean exit; detailed operation
logging is opt-in through Settings; and `recovery` holds application-specific
backups and history-repair operations. The credential-store implementation
owns secret access. Codex's directory is touched only for reversible
integration.

Ordinary snapshots, usage, diagnostics, screenshots, and exports are redacted.
Explicit account export is a separate credential-bearing transfer document,
not a support bundle. Management tokens and pool request keys are distinct.
Server migrations are append-only; upgrades and backup/restore validate state
and encrypted references before activation.

## Connections, discovery, and metadata

ChatGPT is the shipped subscription connector. Accounts support OAuth, existing
local profiles, and compatible imports. API sources are generic records with
endpoint, secret reference, protocol bindings, models, priority/weight,
recovery settings, price evidence, and optional manual prices. Proxies are
optional and may be shared. No provider-name branch defines routing.

Stored-proxy diagnostics are explicit desktop operations using only the selected
proxy and a fixed HTTPS Cloudflare trace endpoint. Checks have bounded response
size, time and concurrency, with no redirect or direct fallback. The renderer
receives only observed IP/country, request duration and stable failure codes.
Session-only results do not change routing or assignments; username-declared
geography remains separate. Import can request checks for newly added entries.

Automation presentation uses shared type definitions for labels, conditions and
model-field visibility. Editors select the type first and accept an optional
custom name, falling back to the type's name. Current quota editors save
automatic execution only. Desktop loading converts
legacy confirmation rules and their unfinished cycles to automatic execution,
preserving enabled state, cycle deadlines, attempts and history. Execution stays
in the background workers; the Connections UI has no manual start controls.

Account/source inventory preserves all models the source actually makes
available, regardless of the currently implemented endpoint. Discovery does
not discard an entitled model because of an endpoint-support flag or a
hardcoded model allowlist. Explicit user filters remain separate policy.

Inventory and executable client projections are different: the current public
OpenAI model view uses enabled Responses/Chat candidates, while the managed
Codex projection requires Responses routes, including declared bridges and
in-pool accounts. Protocol compatibility is checked at the client/routing
boundary; it must not erase the underlying source inventory. Metadata and
prices cannot create a route.

The management pool catalog includes every model of each in-pool member,
including disabled, excluded, credential-less and temporarily unavailable
members. Desktop and server enrich that complete inventory with names, groups,
prices, reference reasoning modes and saved order before rendering it. Matching
IDs are deduplicated case-insensitively; distinct upstream IDs are not merged
by display name. Reasoning policy remains editable without a live route;
executable route projections and request admission enforce actual support.
Each snapshot resolves a member's protocol configuration once for the entire
catalog, then projects indexed routes and upstream cache-price availability
onto its model rows. The index belongs to that snapshot; configuration changes
cannot reuse stale route entries.

All sources use automatic protocol selection. Discovery preserves endpoint
declarations (`supported_endpoint_types` and provider equivalents, including
Gemini generation methods) with their origin and check time. Known service
defaults and a pasted full endpoint are explicit configuration hints. An
unknown `/models` response preserves inventory and routes every catalog model
through the source fallback protocol. Existing records keep their stored data,
but their routes are recomputed by the same automatic resolver.
Capabilities distinguish declared, confirmed, unsupported, and unknown states.
An explicit bounded synthetic generation probe records diagnostic evidence but
does not admit or remove models; catalog refresh never generates. URL/key
changes invalidate evidence and bump
the configuration revision; stale probes and discovery cannot overwrite it.

Model semantics are resolved once from fixed, validated reference sources:
models.dev identity and detailed records, the public OpenRouter catalog, and
LiteLLM. The references fill missing fields, including models absent from the
primary registry; exact/qualified/unique-leaf matching keeps ambiguous IDs
separate. Accounts and API participants supply inventory and endpoint hints,
not authoritative model capabilities. Their optional reasoning, tool, image,
context or speed fields cannot replace reference metadata. No additional
participant metadata prefetch runs during management polling or catalog export.
Native Codex cards retain account-owned transport controls and instructions;
model labels and semantic capabilities use the same resolver as API sources.

Missing reference fields use Relay's common text/image, text-output, function
tool and structured-output baseline. Explicit reference exclusions remain in
force. Unknown reasoning enums and numeric limits stay unknown; no model-version
allowlists supply them. Model Rules may narrow known levels. Adapters still
check whether a parameter can actually be represented in the target protocol.
Prices are explicitly separate: valid participant prices take precedence over
trusted catalog prices, followed by manual fallback. Actual usage and cache
semantics always come from the executed upstream protocol.

Desktop/server load the validated local metadata cache, then independently
refresh sources roughly hourly with HTTP validators. Failed sources become
stale without discarding healthy sources or blocking requests. Replacement is
schema-checked and atomic. Source payloads are shared as compact immutable JSON
across refresh and rollback states, parsed only for validation and merging;
requests use the resolved catalog. Cache reload borrows source envelopes without
building the untrusted stored merged tree. The cache schema and independent
source hash validation remain unchanged. Each merge indexes model versions and
provider-scoped leaves once; persistence and publication reuse the same merged payload.
Cache-file equality checks use a bounded buffer instead of reading a second
complete file into memory.
Backend ordering places OpenAI, Anthropic, Google,
then xAI first, followed by other companies alphabetically, and uses
contiguous catalog families ranked by their newest release, then release/update
dates within each family. Missing families follow known families; missing dates
follow dated versions in the same family. Model/family ranking comes from
catalog metadata, without model-name or version lists.
Explicit manual order takes precedence; React does not re-rank
models. An empty model
order update clears the persisted override in desktop and server, restoring
catalog ordering for existing and future models. Remote clients offer reset
only when the server advertises `model_order_reset`. Both hosts use the shared
`relay-core` model-edit policy for canonical ID lookup, partial-order completion,
and reasoning overrides. Storage, locking, rollback and transport error mapping
stay in the owning host. Member snapshots apply that same metadata/manual
ordering to their complete discovered inventory,
independently of membership and allow/deny rules. React appends configured IDs
missing from inventory without moving discovered models. `gateway.modelCatalog`
provides advisory company/family metadata for the complete member inventory and
saved rules/prices, so excluded models keep their group. It never grants routes;
older snapshots fall back to metadata on operational model rows. Member editors open on
model selection, with source prices and secondary settings on separate tabs.
Model Rules are operational; source prices remain in the source/member editor.

## API source statistics

`relay-core/sources/stats` owns balance transport, format detection, and amount
normalization for both desktop and server. Zenith, DeepSeek and SiliconFlow use their key
endpoints. OpenRouter uses `/key` for inference keys and queries `/credits`
only for a reported management key. Custom sources try Sub2API `/v1/usage`,
New API `/api/usage/token/`, then compatible One API billing subscription/usage.
Requests preserve the configured origin and reverse-proxy prefix, disable
redirects, remove query/fragment data, and bound response size and total time.
Rate limiting stops discovery. Public `/api/status` receives no credential.

Statistics distinguish wallet, key quota, and subscription allowance, preserve
native currency/unknown quota units, and parse decimal values with integer
arithmetic. New API conversion requires its published `quota_per_unit`;
Sub2API `actual_cost` is spend, while `cost` is only an equivalent. Legacy
billing usage is cents in the server's advertised display currency. Legacy
microUSD fields contain USD only. Additional fields default when reading an
older Relay Server. Missing spend remains missing. React presents provider
spend separately from Relay's local estimate and marks retained values stale
after an unsuccessful refresh. Adapter names remain internal and do not add a
caption to the cards. These statistics never decide route eligibility.

## Quota and execution

Accounts and API sources share a versioned `poolRouting` policy with one tagged
member order. Smart ranks eligible members by physical member load, fresh quota
and recent failures, independently of manual order. The failure score decays
linearly to neutral within 60 seconds of the last observed failure; persisted
counts without a runtime observation time do not impose a permanent penalty.
Unknown quota and
quota observations that have aged beyond the refresh window are neutral, even
if no background refresh has replaced the stored value. Confirmed exhaustion
remains ineligible. It rotates by weight among all members near the best score;
soft affinity can retain a successful session only within that same group,
measured against the best eligible score rather than the weighted winner.
In order selects the
first eligible member. Round robin uses smooth weighted rotation, ignoring soft
affinity. Mandatory response ownership applies in every mode.

Weights range from 1 to 100. A member's request limit covers all its protocol
routes and text/image lanes; zero adds no policy limit. OAuth image concurrency
retains its separate one-request limit. Saturated capacity and an occupied
recovery probe wait for a lease release or policy change for at most 30 seconds
before failing. A reservation owns its recovery probe: an older request's
release cannot clear a newer probe, even after another cooldown. Preview does
not advance rotation. Credits change only after successful reservation.

Provider retry hints use the longer of the header and body delay for every
failure category, including model-scoped rejections and overload. A configured
source recovery delay can extend that pause but cannot shorten it. Without a
hint, the configured delay overrides the automatic error-specific delay.
Model-scoped failures leave the source's other models eligible.
Unified pool recovery ignores legacy retry-count, failure-threshold and
last-candidate exemption fields. Its attempt budget covers every configured
route and one bounded recovery pass. Temporary failures pause for at least
5 seconds, then 60 seconds on the next failure, doubling up to 30 minutes.
Longer provider hints and configured source pauses take precedence. Recovery
waits hold no lease, stay within 30 seconds, and still enforce scoped eligibility
and exclusive half-open probes. Explicit persistent ChatGPT recovery remains
separate. Invalid client input remains terminal; auth, access and model failures
can fall through to other candidates. `model_not_available` is a transient,
model-scoped 503. A new client request resets its attempt index but does not
reset the candidate's failure count or cooldown.

Desktop and server resolve the same policy from the full configured pool,
including unavailable members. Legacy API roles become an initial manual order:
primary APIs, accounts, ordinary APIs, reserve APIs. Roles and old strategy fields
remain import compatibility data. Desktop and server always install the unified
policy; direct core callers without `pool_routing` retain legacy compatibility.
Policy saves compare the previous policy and membership before applying
atomically. Hot policy changes preserve leases, cooldowns, and response ownership.
The rotation editor applies edits immediately through a serialized queue. Each
batch reads current settings and reapplies only identity-based user edits, with
at most three CAS attempts. Membership updates cannot resurrect removed members;
failed writes restore the stored values. Remote routing conflicts retain their
typed code, including the HTTP 400 response of older servers.
Server membership updates reconcile and install the same policy as a restart
before refreshing internal request-key scopes.
Portable presets remap tagged member IDs before validation and application.

Monitoring can refresh enabled accounts outside the pool, while draining, or
with unknown quota. Provider windows/reset times are displayed as reported.
Backoff retains the actual safe failure reason; successful refresh can restore
health. Credential, proxy, auth, and capacity failures are not all quota errors.
Model discovery can recover its own errors, but cannot clear an independent
account block, authentication failure, or verification requirement. Transient
catalog errors preserve a previously terminal discovery failure until a
successful catalog refresh.

Pool and Connections display operational groups in the same order: rotation,
quota wait, unavailable, disabled. Scheduler order remains intact within each
group; Connections can additionally group by subscription. The rotation editor
also groups by operational status in Smart and Round robin, while In order
preserves the editable manual queue. Only In order exposes
reordering, and only Smart and Round robin expose weights. The dialog uses one
scrolling body, concise status labels and detailed explanations in Help.
The scheduler supplies the next-route preview using the request key's scope,
model rules, protocols and current member capacity. It names a physical member
only if all fresh text routes agree; continuations keep their own affinity.
Activity events invalidate older previews until a fresh snapshot arrives.
Runtime IDs and activity revisions reject stale reserve/release events across
runtime replacement. A missing preview is not evidence that all members are
unavailable. Pool warnings use member operational state
and the enabled model catalog; active requests take precedence over stale
availability snapshots. These presentation rules do not change dispatch.

Execution checks membership, enablement/draining, credentials/proxy, model,
protocol/adapter support, health/cooldown, quota, and capacity. Response IDs and
active connections preserve upstream ownership. Soft prompt/session affinity
cannot force an unhealthy member. Retry and credential refresh are bounded;
no transparent fallback occurs after response bytes reach the client. Native
Responses continuations keep a bounded local materialized replay chain. Before
any response bytes are visible, Relay can use that chain to move a
continuation from a temporarily unavailable owner to another compatible
candidate; the replacement receives the full input without the old opaque
response reference. When no local replay exists, opaque ownership remains
mandatory and the request waits or fails safely.
Replaying a turn releases only that request's routing constraint; the saved
response owner remains available for other branches and retries. Only history
is inherited; options come from the current request, including provider
extensions. A server conversation or unresolved input item reference cannot
become a portable replay.
Successful turns containing an unpaired tool output remain owner-bound;
upstream acceptance alone cannot reconstruct the omitted call or its history.

HTTP, WebSocket, and account-only endpoints share continuation admission.
An unknown response reference returns `409 response_continuation_unavailable`
before contacting a candidate, even if incoming messages look complete.
An explicit compacted window is a separate stateless contract: a nonempty
compaction checkpoint with materialized retained items and paired tool calls
replaces the predecessor reference before routing. Relay forwards that window
unchanged, without prepending stale replay history or pruning its items.
Compaction settings alone, item references, and incomplete tool state do not
establish such a checkpoint. Invalid encrypted compaction fails explicitly;
recovery must never discard it to retry without the earlier context. See the
[Responses compaction contract](https://developers.openai.com/api/docs/guides/compaction).
Plaintext recovery requires a saved predecessor scoped to the same local key
and owner; it materializes that history before removing the reference. An
unpaired tool output needs a known owner, while paired tool history without an
opaque reference may rotate. Recovery never deletes incomplete tool calls.
After an explicit upstream tool-link rejection, native HTTP/SSE and WebSocket
may repair one unambiguous call/result mapping before visible output. Call IDs
remain distinct from item IDs; matching respects tool kind and namespace.
Repairs apply atomically and never discard results or guess between parallel
calls. Missing history stays an error.
Ownership checks inspect every tool identifier. Without a predecessor reference,
all unpaired outputs must share a known owner; completed historic calls cannot
choose that owner or authorize an unknown output.
Native stream replay retains complete terminal output or completed output
items, preserving tool state and assistant phase; deltas alone are insufficient.
Capture is bounded in memory and never written to diagnostics. An incomplete
response can continue only on its existing WebSocket, using a connection-local
binding that is not persisted or accepted by another connection.

Generation requests have no Relay total-duration, first-output, or inter-event
timeout, in either JSON or streaming mode. HTTP clients retain bounded connection
establishment; metadata and credential operations retain their own timeouts.
SSE emits keepalive comments between complete frames after response commitment;
pre-output buffering preserves safe retry and HTTP error status. WebSocket pings
keep both sides active while waiting for a provider, including HTTP fallback.
Idle WebSocket cleanup applies only when no request is in flight. Provider
completion/failure, transport failure or client cancellation ends a generation;
silence alone does not release its lease, penalize a source or trigger a retry.

Failures preserve `relay`, `account`, or `provider` route origin plus safe
category, status, and timings across protocols, storage, UI, and exports.
Local and server usage additionally retain a bounded, redacted provider error
envelope (original code, type, message, and observed HTTP status), separately
from Relay's classification. Successful replacement attempts clear stale error
details; legacy records cannot reconstruct messages that were never recorded.
Raw payloads and credentials never enter diagnostic records.
Invalid SSE JSON records Relay parser diagnostics in the existing error envelope
with type `relay_stream_parser`: fixed error category, position, byte/line counts
and format flags only. Upstream event names and payload fragments are excluded.
Upstream origin identifies the selected account or API source independently
of whether the failure affects account health. Generic HTTP 400/422 or
`invalid_request_error` alone does not prove a client error: candidate rejection
can still fall back, while explicit request-validation failures stop retries.

## Protocol adapters

All four JSON generation entrances and account compact/search use one bounded
body decoder for identity, gzip and zstd. Wire and decoded bodies each have a
64 MiB limit; stacked encodings are rejected and zstd windows are bounded.
Multipart image edits retain their separate body contract.

Account compact first uses the legacy endpoint. Only an explicit missing
endpoint (405 or an unambiguous route-not-found 404) permits one Responses
request with `compaction_trigger`, on the same account and within the retry
budget. The bridge requires a complete successful SSE terminal response with
encrypted compaction output, preserves usage and retained output, and does not
replay a failed or interrupted generation. Model errors and quota failures do
not trigger this compatibility path.

Sparse terminal responses are reconstructed from completed output items using
the shared bounded replay collector. Unfinished output and events after the
terminal response cannot produce a successful compact result.

Codex turn-state hints are scoped to local key, session, account, upstream model,
exact state and credential identity. OAuth refresh changes that identity; an
AgentAssertion uses the stable signing credential/runtime/task, excluding its
per-request timestamp. HTTP and WebSocket apply ownership after authorization
is prepared. The bounded expiring store retains only fingerprints, so delayed
responses cannot transfer state to another owner or credential.

An open upstream WebSocket is reused only while its authorization fingerprint
matches the current credential. A changed credential requires a new connection
and complete portable history; opaque continuation state fails explicitly.
Background Codex catalog updates recheck the original profile binding under
the profile lock before writing a fetched catalog.
Codex launch applies deferred catalog updates before starting the client; a
failed refresh retains the last verified catalog and its visible warning.
An immediately preceding successful attachment needs no second catalog fetch.
An OpenCode configuration error does not prevent the Codex catalog refresh.
Local pool attachment fetches its catalog before stopping Codex or changing
history. Renderer snapshot refresh follows both attachment and optional launch.
History synchronization inspects imported session metadata even when the profile
already uses the target provider. Its aggregate size budget applies only to
rollouts that need backup and rewriting; matching history does not consume it.
Per-file and file-count bounds, snapshot validation and rollback remain enforced.
Native account catalog reads run with at most four concurrent requests and a
shared twelve-second budget; completed results retain account ranking and
unreachable accounts keep their own last known transport metadata.

ChatGPT account HTTP and WebSocket handshakes retain only the infrastructure
cookie `__oailb`, in memory, per account executor and credential. The store is
bounded, respects cookie path and expiry, and only sends to HTTPS port 443 on
`chatgpt.com/backend-api`. Proxy/runtime replacement creates a new store;
late responses hold the old jar. Browser/authentication cookies are excluded,
and no cookie values enter storage, diagnostics, or client responses.

The adapter registry supports four native contracts and twelve conversions:
Responses (`/v1/responses`), Chat Completions (`/v1/chat/completions`), Messages
(`/v1/messages`), and Gemini (`/v1beta/models/{model}:generateContent` or
`:streamGenerateContent?alt=sse`). Native requests preserve provider extensions.
Catalog feature and reasoning declarations constrain converted routes, not
native request admission; native endpoints validate their own parameters.
Converted requests use typed messages, images, function calls/results, tool
selection, JSON schemas, and reasoning controls where a matching representation
exists. Meaningful unsupported parameters fail before sending. A JSON Schema
is preserved; strict-mode guarantees are not invented for Gemini. Responses
bridges additionally support their existing namespace/custom-text tool contract.
Codex tool-schema normalization expands safe local references once per schema,
within a 1 MiB expansion budget and depth 64. Constraint siblings, resource
boundaries, dynamic references and literal data remain intact. Exceeding the
budget or encountering an unsafe scope preserves the original schema atomically.
Responses bridges accept Codex client tracing and cache-affinity keys without
forwarding them as provider parameters. The optional encrypted-reasoning output
selector does not require fabricated encrypted output; native bridge state stays
local. Supplied encrypted input and compaction still require a compatible native
route. Unsupported known controls report a safe field name in `error.param`
and the error message, never a request value. Neutral text/null controls do not
claim structured-output or reasoning capabilities during admission.

Admission validates the request and continuation, filters compatible routes,
then rotates physical members and prefers a suitable native route within the
selected member before reserving capacity. Routes have stable identities; all
routes of one member share weight and concurrency. Endpoint/model failures
remain scoped, resource failures apply to the physical member, all attempts use
one retry budget, and visible output prevents fallback. Continuation ownership
is retained unless a complete portable history permits replay. Opaque state,
encrypted reasoning and unpaired tool results are never discarded for retry.

Streams retain tool IDs and order, terminal status, and actual upstream usage.
Portable reasoning text is preserved in Chat Completions JSON, streamed deltas
and tool-turn history using `reasoning_content`. Unsigned Messages thinking,
Gemini thoughts and public Responses summaries can use this representation.
Signed, encrypted or otherwise opaque state remains bound to its native route;
foreign reasoning history cannot fabricate native Responses item identifiers.
SSE framing accepts LF, CRLF and CR, including fragmented and mixed line
endings. Multiple data lines remain one event payload; malformed JSON is not
split heuristically. Native stream bytes remain unchanged.
Absent counters remain unknown. Native Messages/Gemini catalogs and model
details enforce the same key scope as generation. The shared Rust projection
exposes routes, feature status, and executable reasoning levels to desktop,
server, and client configuration. Frontend renders this projection.

Cross-protocol WebSocket, Realtime, audio/video conversion, and server-tool
emulation are excluded. Native Responses WebSocket remains available; Codex
uses HTTP/SSE when its catalog includes converted Responses routes.

## Usage and prices

Usage stores observed token/cache splits, service tier, selected member, status,
and timings. API-equivalent is an informational estimate, not subscription
entitlement, provider debit, Zenith customer billing, or a routing input.

Subscription usage uses the exact LiteLLM record in its declared official
family or remains unpriced. API sources resolve provider evidence, exact
LiteLLM provider/model, explicitly declared-family canonical price, then manual
price. Endpoint/protocol changes invalidate stale source evidence. Input,
cached input, cache writes, output, and request/image prices remain distinct;
missing required counters/prices are unknown, not `$0`. Adapters follow the
actual upstream cache contract without borrowing another protocol's semantics.

LiteLLM is the external reference price catalog. Cached startup is nonblocking;
conditional refresh runs at startup and then the cache TTL (currently 24 hours)
with bounded retries. Invalid payloads cannot replace the last good snapshot.
Calculations use immutable catalog/policy revisions; refresh invalidates derived
totals, not stored token facts. Prices do not affect admission or scheduling.

Fast denotes observed upstream priority service, separate from source priority;
it is not a second quota meter or a universal price multiplier. Provider quota
is an operational signal; monetary quota is not inferred from its percentage.
Request-speed options are Relay model-family policy, not participant discovery.
OpenAI conversational models offer Standard, Fast (`priority`) and Ultrafast
(`ultrafast`) by default; other families keep Standard until a project policy
exists. The common family resolver handles new versions without a version list.
No source tier arrays, speed probes, or extra public Codex speed catalog are used.
Management snapshots and native/managed Codex catalogs share this policy even
when the listener is stopped or a member is cooling down. Key scope and model
inventory still govern which models are visible. Explicit client choices take
precedence over pool defaults and survive HTTP/SSE and native WebSocket retries
unchanged. The upstream response supplies the observed tier independently;
publishing a request option never fabricates priority usage or quota entitlement.
Optional purchase cost/payback and remaining API-equivalent estimates are
display-only and require complete priced Relay usage for the relevant window.

## Profiles and user-managed server

Profile changes follow inspect, protected snapshot, attach/apply, verify, and
restore. Only Relay-owned config/auth/catalog fields are managed; newer manual
sign-ins and unrelated settings survive. Cross-provider history repair is
reversible and rolls back if profile application fails. Named ChatGPT recovery
points are separate explicit restores of configuration/authentication.
History scans fingerprint bytes during the metadata pass and skip constructing
JSON trees for conversation/tool events. All session metadata records are still
checked, including later records in imported histories; rollback checks retain
the fingerprint of the complete file.

Codex attachment writes a validated bounded catalog from the selected live
endpoint/key with reversible model IDs. Native rows keep account transport controls; model capabilities use the shared resolver.
Account models and unqualified GPT IDs retain their public spelling even when
an account lacks a native catalog card. Explicit key prefixes stay intact and
old Relay aliases remain accepted. Only the owning account's matching card supplies native transport fields.
Every row uses shared reference model metadata; fallback rows retain the Relay
ownership marker.
Native transport card selection skips incompatible cards when another owning
account has a valid one. Labels always use the resolved reference catalog name
(exact ID or an unambiguous leaf), then a compact label from the ID. The same metadata label resolver serves pool catalogs, management
snapshots and direct API profiles. Labels never merge routes or grant native
account capabilities; numbered GPT fallback labels omit the GPT prefix.
Refresh writes before invalidating the client cache; failure retains the prior
verified profile and exposes a warning. OpenCode preserves the original JSON/
JSONC configuration and restores with a semantic merge of compatible user edits.
OpenCode groups executable models under Responses, Chat, Anthropic and Google
SDK providers, preferring native routes for new assignments while preserving
working provider/model identifiers and the selected model. Automatic refresh
requires unchanged managed endpoint/key/SDK ownership and preserves user
options; it does not restart the client. Direct sources use only native routes.
Relay-owned ChatGPT account calls start with a validated stable Codex fallback,
then asynchronously read the official OpenAI Codex GitHub release feed after
the host loads and once per hour. Only published `rust-v<semver>` releases
without a prerelease component are accepted; the selected version remains
process-local and is never written to a JSON cache. Direct client identity
headers remain client-owned.

Remote management negotiates capabilities and uses revision-checked operations.
`source_protocols_v1` gates endpoint evidence, probes, and the adapter contract.
Preset schema 4 accepts the legacy protocol mode field during import but does
not export or apply it. Adapter settings unsupported by an older server are
rejected before upload.
Secret transfer is separate from profile/config publication. Backup/restore
uses the server CLI with a locked data directory. Real account, proxy, client
tool-use, streaming, restart, and restore acceptance remains necessary before
production-ready claims; mocks or a successful build do not establish it.
