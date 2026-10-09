# Zenith Relay architecture

This file records the current Relay contracts. Source code and focused tests define
exact behavior; this document states the boundaries that must remain true.
Unfinished acceptance work is in [ROADMAP.md](ROADMAP.md), contributor checks in
[CONTRIBUTING.md](../../CONTRIBUTING.md), and user actions in Help.

## Scope and ownership

Relay is a separate local-first desktop and user-managed server product. It may
use permitted user-owned ChatGPT accounts and compatible API sources. It must
not contain Zenith production credentials, customer data, production inventory,
or Gateway/Control billing and routing logic.

| Area | Responsibility |
| --- | --- |
| `src/src` | React UI, i18n, snapshots, typed Tauri wrappers |
| `src-tauri/src` | Desktop I/O, credential store, OAuth, profiles, lifecycle |
| `crates/relay-core` | Discovery, metadata, scheduling, protocols, gateway, quota, usage |
| `relay-server` | User-managed runtime, encrypted vault, SQLite, management API |

The desktop and server use the same core contracts. The renderer never reads
secrets, files, or provider endpoints and never implements routing. A local
process keeps serving after its window closes; an explicit process quit stops it.
Distributed multi-server scheduling is out of scope.

## State and secret boundaries

Desktop state lives in the platform data directory. Durable database, vault,
catalog, migrations, recovery, export, cache, and redacted log areas stay
separate. The credential-store implementation owns secret access. Codex files
are changed only by the profile integration and are recoverable through Relay.

Snapshots, usage, diagnostics, screenshots, and exports are redacted. Account
exports are explicit credential-bearing documents and are not support bundles.
Management tokens and pool request keys are different credentials. Server
migrations are append-only; backup and restore validate encrypted references
before activation.

## Connections, discovery, and metadata

Connections owns saved accounts and API sources. Integrations separates
application setup from account settings grouped by provider. API owns the
listener, request key, port, and protocol-wide route recovery. ChatGPT account
settings own model-substitution checks; application settings own the client
profile, background tasks, and WebSocket controls.

ChatGPT is the shipped subscription connector. Other providers are generic API
sources until a separate connector proves authentication, refresh, entitlement,
quota, usage, recovery, and permitted live-provider behavior. Proxies are
optional and may be shared. Provider names do not select a route by themselves.
Stored proxy checks retain the latest result in the local credential store.
Snapshots expose only exit IP, country, latency, check time, and a bounded error
code. Diagnostics do not decide account routing health. Completion updates the
same stored proxy only; deleted proxies and newer checks fence late results.

Codex and Excel / Basis Points use separate OAuth clients and separate saved
connections, including for the same ChatGPT principal. Refresh, reauthentication,
imports, exports, and server transfer retain the issuing client. Legacy
credentials without a client hint remain Codex; captured BPS headers do not
identify an OAuth client. Token exchange and refresh reject conflicting known
token-client hints. An invalid refresh response retains saved credentials.
Targeted reauthentication checks the client and principal; a stable user ID
takes precedence over a changed email.

Excel sign-in uses PKCE and a provider callback validated in Rust, through the
Relay sign-in window. The dialog labels are ChatGPT and Excel / Basis Points.
ChatGPT sign-ins always use native Responses; Excel always uses BPS. Legacy
BPS preferences are ignored. Model substitution checks have a separate switch.
Internal OAuth identifiers remain `codex` and `excel_bps`. Contextual
Help leaves the pending flow active. Account surfaces identify Excel
connections with a BPS badge after the plan. Callback and checkpoint material
stay in protected storage. Excel connections are pool-only: they do not write
native Codex `auth.json`, use Agent Identity, or expose the native image bridge.
Excel exports are limited to Zenith and Sub2API formats.

Discovery keeps the complete model inventory reported by a source, including
models whose current endpoint is unsupported or temporarily unavailable. The
inventory is separate from executable route projections and from model rules.
`/models` presence or a model name never proves that generation will succeed.
Discovery does not send generation probes during normal refresh.

Each source retains endpoint declarations and their origin. Known service
settings and a full endpoint are configuration hints. If declarations are
unknown, the configured fallback protocol is used. A legacy explicit diagnostic
endpoint is informational and cannot add models or grant route eligibility.
Changing an address or key invalidates old evidence.

Capabilities have four states: declared, confirmed, unsupported, and unknown.
Reference metadata comes from the validated models.dev, OpenRouter, and LiteLLM
catalogs. These sources fill identity, family, limits, modalities, tools,
structured output, and reasoning fields; participant metadata is evidence for
that participant and is not universal model truth. Missing text/image and tool
fields use the shared compatibility baseline; explicit exclusions and unknown
numeric limits remain unknown. A catalog never grants a route.

Route IDs and canonical model IDs remain separate. Qualified IDs are matched
first, then a unique leaf; ambiguous leaves are not merged. Display grouping is
OpenAI, Anthropic, Google, xAI, then other providers alphabetically. Within a
provider, ChatGPT inventory follows ascending official `priority`, with stable
ties and unranked entries last; API inventory retains response-array order.
When inventories overlap, native accounts establish the order of shared IDs,
then API sources append remaining IDs. Manual model order overrides this
default, including when reference metadata is unavailable. Model order and
member rotation order are independent.

## Statistics and refresh

Source statistics distinguish wallet balance, key allowance, and subscription
allowance and preserve the provider's units. They never decide route
eligibility. A failed refresh retains the last value with a stale state;
unknown is not zero. Unsupported means there is no supported reader for that
service or its stats endpoint is unsupported. Provider spend and Relay's
API-equivalent estimate remain separate.

Known service hosts share one protocol profile. Kimi Code is scoped to
`/coding`; MiniMax's `/anthropic` prefix selects Messages. Their SDK roots
normalize to `/coding/v1/` and `/anthropic/v1/` before endpoint construction.
Explicit endpoint URLs take precedence. Profiles declare a protocol, never
model entitlement.
Moonshot wallet reads use `/v1/users/me/balance`: `.ai` preserves USD and `.cn`
preserves CNY; `available_balance` already includes vouchers. Known services
without a verified stats adapter return unsupported without probing unrelated
billing APIs. Unknown compatible services retain stats autodetection.

Account credit totals count each provider account ledger once across OAuth
connections, using its newest known balance. Connections remain separate
members for authentication and routing; labels and email hints do not identify
a credit ledger.

Models, balances, account quota, and usage have independent refresh jobs. Reads
join an existing resource-scoped job instead of starting duplicate work. A
manual refresh does not cancel shared work. Configuration, credential, delete,
and re-add revisions fence late results. Runtime statistics are not durable
freshness evidence after restart.

Model and quota checks preserve timeout failures separately from other
connection errors. A monitoring timeout does not block account authorization.

## Pool and execution

Accounts and API sources use one admission engine and versioned `poolRouting`.
New profiles use **Automatic**; legacy Smart maps to Automatic, and legacy In
order/Round robin maps to **Manual** without changing membership or credentials.

Automatic selection:

1. keep only compatible, enabled, healthy physical members under capacity;
2. choose the least normalized load (`in_flight / effective capacity`);
3. within that group prefer fresh positive quota;
4. when none has positive fresh quota, prefer fresh provider credits;
5. use request weights to resolve a remaining tie.

Wallet balances, prices, latency, and stale or unknown observations do not rank
members. A soft session affinity stays in place until another equally loaded
member leads by at least 15 fresh provider credits, unless a fresh quota
remainder takes precedence. An opaque response owner cannot move without saved
portable history.

Manual mode follows the saved member order, skips unavailable or full members,
and wraps to the first member. It ignores weights and soft affinity. A member
limit covers all its protocol routes; zero means no extra member cap, not
unlimited provider capacity. Changes are applied through serialized, identity-
checked policy saves and do not resurrect removed members.

Admission has bounded queues (1,024 waiters overall and 256 per request key).
Capacity and recovery waits share a 30-second budget unless explicit persistent
text-route waiting is enabled. Cancellation removes a waiter immediately.
Local saturation returns a Relay error without a provider-health vote.

One logical request owns its dispatch, transport, retry, and irreversible-send
budgets across HTTP, SSE, WebSocket, image, auth, and compatibility paths. The
default is three generation dispatches; the validated setting accepts 1–8. A
retry requires a proven pre-execution rejection or not-sent result, repeatable
input, and valid ownership. A sent or uncertain request is never replayed on a
different member. The default safe retry window is 30 seconds; provider
Retry-After and reset deadlines cannot be shortened.

Transient route failures are paced at 250 ms and 500 ms; repeated failures open
a circuit with backoff from 2 to 60 seconds. Health, cooldown, quota, access,
and model blocks are separate. A model block does not disable unrelated models.
Quota monitoring may include enabled accounts outside the pool, but monitoring
never grants a route.

## Continuations, errors, and redaction

A continuation keeps the physical response owner. Relay may move it only after
pre-execution rejection and when complete local history can be replayed. An
unknown response reference returns `409 response_continuation_unavailable`;
partial history, encrypted compaction, unresolved provider references, and
unpaired tool output remain errors. Compaction checkpoints are explicit and
replace the predecessor reference; Relay never discards them to force a retry.

An opaque HTTP 400 is a terminal request rejection. Its generic message does
not justify route cooldown, candidate rotation, or history repair. A
continuation repair requires a specific response-reference or tool-link error.

Failures retain the origin (`Relay`, `Account`, or `Provider`), safe category,
status, timings, and redacted provider details. Client error messages use the
English source prefix; existing codes and provider diagnostic fields remain
unchanged. Raw bodies, prompts, credentials, and authorization headers never
enter diagnostics or usage records.

## Protocol adapters

Relay accepts four public contracts: Responses, Chat Completions, Anthropic
Messages, and Gemini. Native routes preserve provider extensions. Typed
conversions support messages, images, tools, tool results, JSON schemas, and
reasoning controls only when the target protocol can represent them. Unsupported
meaningful parameters fail before sending; a conversion does not create a
provider capability. Hosted tools are the exception (see Tool catalog): a
bridge drops them rather than failing the whole request.

Each model has one upstream protocol, chosen before any client protocol is
projected. The model's group in the validated reference catalog decides it, not
the host or reseller that serves it: OpenAI models use Responses, Anthropic
models use Messages, Google models use the native Gemini endpoint, and every
other model uses Chat Completions. The group comes from the canonical model ID's
namespace, then a first-party namespace, then the family (`gpt-oss` is not an
OpenAI model). Relay translates request, response, streaming, and WebSocket
between the harness protocol and that upstream protocol. Selection order: an
endpoint the source owner pinned (hint or endpoint URL), the group-native
protocol, the known service host, then per-model capability evidence, stored
bindings, and the source default. A protocol the source reports as unsupported
is excluded from every selection stage, including stored bindings and the
source default. A rejected native protocol can fall through to an available
alternative; if all configured alternatives are rejected, no route is created.
The Chat Completions catch-all is only an assumption: declared source
evidence for another protocol outranks it. It is defined in one place,
`ModelMetadataCatalog::native_protocol_for`. Cache-write pricing applies only to
routes whose upstream is Messages, so it follows this selection.

JSON, gzip, and zstd request bodies share one bounded decoder. HTTP/SSE success
requires the client's terminal event. Gemini requires `finishReason: STOP`;
a bare `[DONE]` cannot establish success. The current upstream WebSocket bridge
handles one response and at most one named lane per connection; it is not full
multi-lane multiplexing. Incomplete responses cannot become reusable
continuations.

**Basis Points** is available only to Excel OAuth credentials. It uses the
connection's quota and rotation slot and buffers a completed upstream response
when needed. Unsupported structured output, remote image URLs, opaque
`previous_response_id`, and non-standard service tiers are rejected. Ordinary
ChatGPT credentials stay on native Responses regardless of legacy preferences.

Excel refusals use the ordinary scheduler settlement and cooldown rules,
including refusals inside HTTP 200 responses. Execution certainty is retained:
a refusal does not authorize replay after unknown or completed execution.
Neither transport choice nor model-name checks prove response quality.

Adding an ordinary ChatGPT connection to the pool prompts once per action.
Skipping excludes ordinary connections from the addition but retains imported
accounts and still adds Excel connections and API sources. The optional
reminder preference is device-local; right-click addition bypasses the prompt.

## Tool catalog

On native routes Relay forwards the client's tool catalog unchanged. It does not
add deferred tool search, hide or rename tools, or execute a local relevance
search. Stored legacy optimization flags do not change new requests. Usage may
record catalog size and mode as history; those fields are not an execution
permission.

Bridged routes (a Responses client on a Chat Completions, Messages, or Gemini
upstream) can only declare function-style tools. Relay sends the catalog (root
`tools` plus `additional_tools` items) as functions: custom tools get a single
`input` string, namespace children are flattened to opaque names, and hosted
tools such as web search are dropped because the upstream cannot run them.
`defer_loading` and `allowed_callers` do not apply upstream and are ignored.
Responses and streams restore the client's tool kind, name, and namespace. A
forced tool choice that names a tool that was not declared still fails before
sending.

## Usage and prices

Usage stores the selected member, model IDs, protocol, status, timing, token and
cache counters, service tier, and a redacted provider error envelope. API-
equivalent cost is informational and never a subscription debit, customer
charge, or routing input. These records come from the upstream payload, not from
the body returned to the client.

Error details retain supported provider request IDs from the error envelope.
The same validated ID is appended once to the client-visible error message;
native structured metadata remains intact. Request details show API-equivalent
cost as an estimate and leave unknown cost unset. Tool details show client,
sent, and returned counts without legacy optimization diagnostics.
Account diagnostics omit identities and show an observation time only when it
belongs to the displayed error.

Native Responses usage can include a local context comparison, shown in request
details only while debug mode is enabled. It distinguishes
client and prepared-upstream changes between completed requests from Relay's
changes within one request. Parameters, whole `input` item prefixes, candidate
changes, and time since completion are classified without retaining content.
Only salted fingerprints stay in a bounded process-local store; no fingerprints,
salt, prompts, tool arguments, or session identifiers enter these diagnostics.
The baseline expires after 30 minutes and resets with the runtime. Failed,
overlapping, unscoped, and oversized requests do not establish a new baseline.
Delta continuations are not compared as full histories. JSON bytes are not
tokens, and matching local items cannot prove a provider-rendered prefix or
cache hit. Bridged routes, Basis Points, compact, search, and wake requests are
outside this comparison. Old usage rows remain readable without the optional
`routing.cacheContext`; new classifications use the existing routing JSON on
desktop and server.

A Responses client on a bridged route receives `usage` with input, output, and
total tokens together, or `null`. A missing total is derived (Messages counts
cache reads and writes as input; Gemini adds thought tokens), a total the
upstream reported is kept, and an unknown count is never filled with zero.

Price resolution is separate from execution: observed provider prices first,
then an exact LiteLLM provider/model record, canonical family reference, and
manual fallback. Input, cached input, cache writes, output, image, and request
prices remain separate; missing values stay unknown rather than zero. Standard,
Flex, and Priority rates are selected only when the upstream reports that tier.
Cache lifetime is displayed as an estimate when the provider reports it or the
model's documented fallback applies; it does not prove a future cache hit.

## Profiles and user-managed server

Profile operations follow inspect, protected snapshot, attach/apply, verify, and
restore. Automatic rollback changes only unchanged Relay-owned fields and never
overwrites a newer manual sign-in. ChatGPT OAuth writes the native token fields
and clears Relay overrides so Codex can discover its models. Pool activation
writes the Relay provider and validated catalog through the live local endpoint.
Backups stay in Relay recovery storage; credential snapshots use the OS secret
store. OpenCode keeps its original JSON/JSONC configuration and restores user
edits through a semantic merge.

Remote management is capability- and revision-checked. Presets transfer pool,
model, ordering, and price settings, while secrets and account credentials use
separate explicit transfer operations. A preset or snapshot is not a hidden
two-way synchronization mechanism.

## Explicitly out of scope

Relay does not provide hosted multi-user tenants, customer keys, wallets,
payments, customer billing, distributed scheduling, fingerprint spoofing,
sharing concealment, or automatic migration of opaque provider conversations
between accounts.
