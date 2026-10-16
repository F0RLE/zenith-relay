# Zenith Relay architecture

This file records the current Relay contracts. Source code and focused tests define
exact behavior; this document states the boundaries that must remain true.
Unfinished acceptance work is in [ROADMAP.md](ROADMAP.md), contributor checks in
[CONTRIBUTING.md](../../CONTRIBUTING.md), and user actions in Help.

## Scope and ownership

The current Relay implementation is a separate local-first desktop and
user-managed server product. It may use permitted user-owned ChatGPT accounts
and compatible API sources. It must not contain Zenith production credentials,
Zenith customer data, production inventory, or Gateway/Control billing and
routing logic. The planned hosted multi-user direction is described in
[ROADMAP.md](ROADMAP.md#hosted-multi-user-relay); it is not an implemented
contract in this document. Its proposed ownership, request, billing, security,
transfer, and deployment logic is documented in
[HOSTED_RELAY_ARCHITECTURE.md](HOSTED_RELAY_ARCHITECTURE.md).

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

## Desktop workspace

Pages use the available workspace beside the sidebar rather than a centered,
site-width container. Headers and data tables share its left edge and available
width. Account, source, proxy, and pool card grids respond to workspace width,
including sidebar changes, while keeping card tracks compact. Grouping does not
stretch a solitary account across several tracks. Settings, integration forms,
reading content, and modal dialogs retain their own readable width limits.

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

Connections owns saved accounts and API sources. Access groups the pool API,
application setup, and account behavior settings in one page. Its Local API tab (Server
API for a remote runtime) owns the listener, request key, port, and protocol-wide
route recovery. Accounts contains settings that affect account requests,
including ChatGPT model-substitution checks. Sign-in remains in Connections.
After authorization, the dialog shows only saving status and closes when the
account is saved, without a mandatory follow-up setup dialog. Pool membership
and optional proxy changes remain explicit account/pool actions; sign-in does
not automatically enable membership. Initial quota and model probes run
concurrently after agent registration; fresh plan validation still precedes
the durable account commit.
Application settings own the client profile, background tasks, and WebSocket
controls. Pool → Model Rules owns per-model context limits and Relay's internal
automatic compaction threshold.

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
ChatGPT sign-ins use native Responses in the pool and also support direct launch;
Excel always uses BPS for the pool. Legacy BPS preferences are ignored.
Model substitution checks have a separate switch.
Internal OAuth identifiers remain `codex` and `excel_bps`. Contextual
Help leaves the pending flow active. Excel connections are marked BPS.
Callback and checkpoint material stay in protected storage.
Excel connections are pool-only: they do not write
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
structured output, reasoning efforts and modes, parameter support, caching,
streaming, and service tiers. Participant metadata is evidence for
that participant and is not universal model truth. The compatibility baseline
supplies text modalities; missing optional capabilities, including images,
tools, and streaming, remain unknown. Explicit exclusions remain unsupported,
and missing numeric limits remain unknown. A catalog never grants a route.

Catalog effort names `very_high`, `extra_high`, and their spaced, hyphenated,
or camel-case forms normalize to `xhigh` before matching saved policies,
projecting client catalogs, or displaying statistics. Imported capability
flags use the same identity. Relay labels this level "Very high" /
"Очень высокое"; localized labels do not change the provider parameter.

Standard is the baseline speed. Fast and Ultrafast require explicit reference
modes and at least one configured transport that can carry the mode. A source's
explicit tier list can narrow these choices, including an empty list that allows
only Standard. Fast uses Responses or Chat Completions; Ultrafast requires
Responses. Basis Points carries Standard only. Model names and prices never
create faster modes; temporary cooldown and health do not remove configured
choices. Management snapshots and client catalogs use the same projection,
with client catalogs further limited by the request key's member scope.
The pool default is captured once after model resolution and survives retries
and WebSocket-to-HTTP fallback. Explicit client tiers are preserved; an ordinary
API request without a tier remains Standard.

Route IDs and canonical model IDs remain separate. Qualified IDs are matched
first, then a unique leaf; ambiguous leaves are not merged. Display groups use
the canonical model's manufacturer, then a unique family owner from the primary
reference or an unambiguous leaf reference, then the reference namespace.
Familiar unresolved IDs have a display-only provider fallback; ambiguous leaves
remain unclassified. One group projection feeds sorting, model summaries, and
complete member catalogs, including binding-only IDs.

Default display order comes from `catalog/model-order.json`: provider, family,
class, descending numeric version, then variant and snapshot. Provider, family,
class, and variant ranks are array positions in that document. Unknown
providers/families have deterministic name and numeric-version fallbacks;
release dates do not imply a capability rank. Opt-in family captures separate
parameter sizes and short release tags from generations; sizes compare
numerically within a generation and variant. Reference names can fill an
omitted generation only within the same family and class.
API, native ChatGPT, BPS, pool,
and managed client lists use the same comparator. Discovery retains the
upstream inventory; upstream picker priorities do not define Relay's order.
Direct native ChatGPT sign-in does not replace the application's catalog.

Manual order overrides the default even without downloaded reference metadata.
New models join their saved group without moving existing manual anchors.
The UI moves models within a group and groups as whole blocks. Management
snapshots include complete inventory positions, including binding-only IDs
and exact saved rules/prices, independently of filtered runtime availability.
Request keys still expose only their own scoped inventory. Model order and
member rotation order are independent.

`catalog/README.md` defines the versioned public policy and reference export.
The exporter uses the same shared comparator and only validated public
metadata; it never exports participant inventory, saved overrides, routes,
credentials, or usage. It records policy and metadata revisions and individual
source freshness. Exporting does not publish a feed.

## Statistics and refresh

Source statistics distinguish wallet balance, key allowance, and subscription
allowance and preserve the provider's units. They never decide route
eligibility. A failed refresh retains the last value with a stale state;
unknown is not zero. Unsupported means there is no supported reader for that
service or its stats endpoint is unsupported. Provider spend and Relay's
API-equivalent estimate remain separate.

Known service hosts share protocol profiles. Zenith declares all four
generation endpoints; validated model identity selects its native upstream
protocol independently of the client's API. OpenAI, OpenRouter, DeepSeek,
Groq, Moonshot, and xAI retain their service defaults. Kimi Code is scoped to
`/coding`; MiniMax's `/anthropic` prefix selects Messages. Their SDK roots
normalize to `/coding/v1/` and `/anthropic/v1/` before endpoint construction.
Explicit endpoint URLs restrict the available protocols. An incompatible
native model has no route through that endpoint. Profiles declare protocols,
never model entitlement.
Gemini catalog and generation URLs share a protocol-specific root. Zenith's
standard `/v1` root uses `/v1beta` for Gemini. Google's bare host and
`/v1beta/openai` compatibility root also use `/v1beta` for native Gemini
requests; explicit native versions and custom source roots are preserved.
Moonshot wallet reads use `/v1/users/me/balance`: `.ai` preserves USD and `.cn`
preserves CNY; `available_balance` already includes vouchers. Known services
without a verified stats adapter return unsupported without probing unrelated
billing APIs. Unknown compatible services retain stats autodetection.

Account credit totals count each provider account ledger once across OAuth
connections, using its newest known balance. Connections remain separate
members for authentication and routing; labels and email hints do not identify
a credit ledger.

Quota remainders and provider credits have independent observation clocks.
Passive window headers never refresh the credit ledger; reset-only headers
do not refresh a carried remainder. Passive traffic cannot postpone a full
quota/credit poll beyond the oldest required observation. A complete ledger
read can update credits while preserving newer passive windows, without
emitting quota transitions from the superseded windows. Missing, future, or
expired ledger timestamps cannot grant credit-only admission or win routing;
credit-only admission expires even without another provider reply. Older
saved balances remain visible but need a complete refresh before serving as
fresh routing evidence. Shared totals select the newest ledger observation,
not the newest response-header timestamp.

Models, balances, account quota, and usage have independent refresh jobs. Reads
join an existing resource-scoped job instead of starting duplicate work. A
manual refresh does not cancel shared work. Configuration, credential, delete,
and re-add revisions fence late results. Runtime statistics are not durable
freshness evidence after restart.

Connections deletes accounts into a durable account trash. Trash records retain
their credential references and usage, but are absent from routing, monitoring,
exports and configuration presets. Moving an account restores bound desktop
profiles and prunes its pending wake work and explicit task selectors. Inventory,
trash and refresh revisions commit together; server wake cleanup joins that
transaction. Restore returns the registration disabled and outside the pool,
with a new refresh revision. Permanent deletion uses the existing credential and
usage cleanup, with UI confirmation before dispatch. Reimport cannot overwrite a
trashed registration. Server trash requires `account_trash_v1`; older servers
expose only the explicitly confirmed permanent action.

Local account notes are also available during reauthentication. Passwords and
2FA secrets start masked and have independent reveal controls. Edits save on
blur or close, preserving unedited values; save feedback has reserved space so
controls do not move during a click. The code row appears only for a nonempty
2FA secret, and a changed secret clears the previous code before recalculation.

Model and quota checks preserve timeout failures separately from other
connection errors. A monitoring timeout does not block account authorization.
Native ChatGPT catalog reads default to a 20-second timeout per HTTP attempt.
A transport failure or interrupted successful response body permits one retry
after 250 ms inside the same refresh job. Each attempt re-enters the shared HTTP
gate and checks the current account revision; cancellation leaves no detached
retry. Admission failures, HTTP errors, malformed/oversized responses and BPS
permission reads do not use this retry. Provider Retry-After prevents an inline
body retry and remains a scheduler floor. Exhausted transient reads retain the
last inventory and report the failure; eligible automatic jobs retry through
the existing scheduler, and a successful catalog clears its discovery error
even when the model list is unchanged.

## Pool and execution

Native ChatGPT accounts, Excel / Basis Points accounts, and API sources use one
admission engine and versioned `poolRouting`. Desktop commands, server
management, imports, and presets allow both OAuth client kinds in the pool.
Known Free plans are rejected only for Excel / Basis Points. Startup preserves
native account membership and removes ineligible Free BPS membership without
deleting saved accounts or credentials. A native account used by the ChatGPT
interface is selected independently of pool membership.
New profiles use **Automatic**; legacy Smart maps to Automatic, and legacy In
order/Round robin maps to **Manual** without changing membership or credentials.

Automatic selection:

1. keep only compatible, enabled, healthy physical members under capacity;
2. choose the least normalized load (`in_flight / effective capacity`);
3. within that group prefer fresh positive quota;
4. when none has positive fresh quota, prefer fresh provider credits;
5. use a shared soft health score, based on fresh failures and first-output
   latency, to remove a clearly worse affinity candidate;
6. use request weights to resolve a remaining tie.

Wallet balances and prices do not rank members. Missing or stale latency does
not exclude a member. A soft session affinity stays in place until another
equally loaded member is materially healthier or leads by at least 15 fresh
provider credits, unless a fresh quota remainder takes precedence. An opaque
response owner cannot move without saved portable history.

A complete tool call/result pair without an opaque predecessor is already
portable: its saved tool-route binding is only a cache preference. Automatic
quota rebalancing can release that preference before dispatch without loading
a nonexistent `previous_response_id`. HTTP, account-only endpoints, Excel/BPS,
and WebSocket use the same check; unpaired outputs and opaque references keep
their owner unless validated local replay materializes the missing history.

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
Local request-preparation incompatibilities and preflight failures before a
generation dispatch remain excluded for that logical request across recovery
passes, without cooling or disabling the member or spending a generation.
Timed and persistent recovery use the same request-local exclusions; neither
may restart an unchanged unprepared route. Other compatible members remain
eligible, and a new request may still use the excluded member; these exclusions
do not alter pool health.
Quota monitoring may include enabled accounts outside the pool, but monitoring
never grants a route.

## Continuations, errors, and redaction

An opaque continuation keeps the physical response owner unless validated local
replay materializes its complete history before dispatch. Automatic quota
selection may then place the unsent turn on another eligible member. Moving a
failed generation still requires proven pre-execution rejection and repeatable
input; rebalancing never permits replay of a sent or uncertain generation. An
unknown response reference returns `409 response_continuation_unavailable`;
partial history, encrypted compaction, unresolved provider references, and
unpaired tool output remain errors. Compaction checkpoints are explicit and
replace the predecessor reference; Relay never discards them to force a retry.

Native ChatGPT turn state is accepted only after Relay observes it for the same
request key, session, account, model, endpoint, credential, and authorization
incarnation. HTTP headers, JSON bodies, and WebSocket `client_metadata` use the
same guard. Unknown state and client-supplied account credentials are removed;
duplicate response references in metadata cannot bypass continuation admission.
Input history, tool IDs, encrypted reasoning, and compaction blocks stay intact.
Complete client User-Agent/version/originator profiles are preserved together;
incomplete profiles use Relay's whole profile and the real host OS. The selected
executor owns the account binding.

Soft account affinity is separate from opaque continuation ownership and the
child thread's context fingerprint. Bounded `x-codex-turn-metadata` session and
thread hints take precedence; per-request `client_metadata` or turn metadata
fills missing logical identity before legacy session/thread headers. Existing
single-thread header fingerprints remain stable. Sibling threads can share a
root account preference without sharing their context observations. Explicit
`prompt_cache_key` remains the stronger affinity hint; the scheduler namespaces
both hint classes by the authenticated local key and resolved model. No prompt
hash or client-supplied account identity creates a session. Only opaque hashes
enter context observations, and these soft hints cannot authorize turn state,
release a response owner, or relax credential/endpoint provenance checks.

An opaque HTTP 400 is a terminal request rejection. Its generic message does
not justify route cooldown, candidate rotation, or history repair. A
continuation repair requires a specific response-reference or tool-link error.

`turn_rate_limit_exceeded` is a terminal limit on the current conversation turn.
It returns 429 with the provider's known retry delay, without account cooldown,
rotation, reauthentication, or automatic history repair. Account-wide rate
limits and exhausted quota keep their separate policies.

Failures retain the origin (`Relay`, `Account`, or `Provider`), safe category,
status, timings, and redacted provider details. Client error messages use the
English source prefix; existing codes and provider diagnostic fields remain
unchanged. Raw bodies, prompts, credentials, and authorization headers never
enter diagnostics or usage records.

## Protocol adapters

### Shared conversion architecture

Relay's architecture is one extensible, typed conversation representation and
one codec per wire protocol: Responses, Chat Completions, Anthropic Messages,
and Gemini Generate Content. A codec decodes and encodes requests, complete
responses, and incremental events. Cross-protocol traffic follows
`client codec -> shared representation -> upstream codec`; the return path
uses the same representation and the client's codec. All twelve cross-protocol
directions use this pipeline; there are no pair-specific runtime engines.
New models reuse their protocol's
codec and declared capabilities rather than acquiring a model-specific adapter.

Matching contracts use native forwarding outside the conversion representation
so unknown compatible extensions do not undergo a lossy decode/encode cycle.
Catalog protocol policy selects the native upstream only when the exact
source/model endpoint and requested features support it; a catalog row does
not establish endpoint access. HTTP, SSE, and WebSocket are transports, not
additional model protocols.

JSON parsing and serialization preserve object insertion order and arbitrary
precision numeric values, including unknown native extension fields. This is
not a byte-for-byte request-body contract: whitespace and JSON escaping can
change, and routing/transport-owned fields are still normalized. Account tool
compatibility may fold a large union of distinct bare constants into `enum`,
but never folds branches with titles, descriptions, or additional constraints.
Numbers outside bounded canonical-exponent arithmetic keep their union intact.

Permitted list-valued native protocol headers retain repeated values and opaque
bytes. Ambiguous repeated session/state identity headers are not forwarded.
The protocol allowlists and credential exclusions remain in force; unknown
headers and fields nominated by `Connection` are not blindly forwarded.
Native SSE and WebSocket payloads retain original wire bytes, including valid
provider extensions, numeric spelling, and SSE framing metadata. Observation
does not rebuild successful native frames; existing terminal validation,
safe error-prefix rewriting, and oversized-frame rejection remain explicit
exceptions. SSE ends at the first complete protocol terminal event, without
waiting for transport EOF, trailing events, or the optional LF after a terminal
CR-terminated blank line. Native forwarding does not authorize malformed or
foreign terminal events as completion.

The shared representation preserves instruction and content order, tool kind,
namespace, call/result identity, schema constraints, generation controls,
citations, usage, and terminal outcomes. It is not a Chat Completions-shaped
lowest common denominator. Provider-owned blocks, signatures, and ciphertext
remain separate opaque state in the existing bounded volatile continuation
store, bound to the originating conversation, model, protocol, route, and
credential identity. They are replayed unchanged only to a compatible owner.
No second continuation cache or persistent prompt/body logging is introduced.

Every meaningful input field must be represented, handled locally with an
explicit contract, or rejected before dispatch with a safe field diagnostic.
Gemini's alternative schema fields are mutually exclusive; conversion rejects
conflicting function or output schemas, and an output schema without JSON MIME
configuration, instead of choosing or discarding constraints.
Its OpenAPI-style `parameters` and `responseSchema` are decoded separately
from JSON Schema: types, nullable unions, and integer-string bounds are
normalized at schema positions only. Literal defaults, enums and property names
are not rewritten. Nonempty `propertyOrdering` has no portable enforcement
contract and fails explicitly. One shared schema walker serves tool and output
schemas without recursively cloning the same subtrees.
Retaining original JSON internally does not make a foreign field meaningful
to another provider. Provider extensions require a compatible destination;
unsupported required features must not disappear, weaken a tool/schema
constraint, or become ordinary prompt text. Conversion never invents a
provider capability. JSON, streaming, and the complete tool-result continuation
cycle have independent acceptance checks across all twelve conversion
directions and four native routes.

### Conversation and routing contracts

Chat conversions accept `reasoning_content` and `reasoning` as plaintext aliases
in assistant history, JSON replies, and stream deltas. Conflicting nonempty
aliases fail explicitly. Streamed tool IDs and names are stable metadata;
repeating them does not append to the value. Only argument deltas concatenate.
Empty text and reasoning deltas do not open output items. Adapter stream
failures retain safe field diagnostics without storing upstream content.

Responses and Chat Completions conversions retain URL citations in both directions.
Chat JSON messages use nested `annotations[].url_citation`; streaming Chat deltas
emit annotations once before the finish reason, after text has streamed.
Responses streams emit annotation events before the final text and content-part
events. Concatenated text offsets count Unicode characters. Event-only annotations
are reconciled with the final Responses payload; conflicting, invalid, and
unrepresentable annotations fail conversion rather than disappearing.

Relay accepts four public contracts: Responses, Chat Completions, Anthropic
Messages, and Gemini. Native routes preserve provider extensions. Typed
conversions support messages, images, tools, tool results, JSON schemas, and
reasoning controls only when the target protocol can represent them. Unsupported
meaningful parameters fail before sending; a conversion does not create a
provider capability. Hosted tools without a destination equivalent are rejected
before dispatch, including when declared beside supported client tools.

Chat Completions-to-Responses function arguments use integer syntax for exact integral
JSON numbers within `i64`/`u64` bounds. Normalization works on literal digits,
preserving fractional values, numeric strings, negative zero, and larger values.
Stream conversion withholds only an unfinished numeric token; emitted arguments,
the completed call, and portable continuation history share the same value.
Native payloads, custom tool text, and provider-owned replay blocks are preserved.

Translated reasoning controls share one model-aware renderer for every client
protocol. Missing controls preserve provider defaults. Adaptive Messages routes
can carry `xhigh` when the model reference and route allow it. A model declared
budget-based requires an explicit token budget; its token limits do not define
effort levels. Legacy budget bindings without method metadata retain Relay's
compatibility mapping. Explicit output limits stay intact; incompatible controls
fail before sending. Native payloads retain their controls. Usage reads the
requested and sent controls from the exact protocol fields, including Gemini
thinking and BPS's flat effort field.
Messages requires `max_tokens`. When a client omits its output limit, Relay uses
at most 8,192 tokens and lowers that default to a smaller known model output
limit. An explicit limit above a known model output limit is rejected before
sending; unknown limits remain unknown.

The proposed hosted all-protocol output-cap and reservation policy is in
[HOSTED_RELAY_ARCHITECTURE.md](HOSTED_RELAY_ARCHITECTURE.md#customer-balance-and-provider-expense).
It is not implemented hosted behavior and does not change the local
Messages-only compatibility rule above.

Each model may retain several upstream protocols for one source. Each
source/model contract remains a separate route binding while the scheduler
keeps one physical pool member, quota, capacity, and rotation share. Selection
starts with the model's validated native protocol and the source's endpoint
restrictions and exact Catalog or Manual evidence. OpenAI uses Responses,
Anthropic uses Messages, Google uses Gemini, and xAI and Z.ai use Chat
Completions. This requirement applies to catalog visibility and dispatch.
An incompatible pin, unavailable native endpoint, or explicit unsupported
evidence leaves the model without a route; Relay does not substitute a
different upstream protocol to match the harness.
`ModelMetadataCatalog::native_protocol_for` resolves known identities from
canonical metadata and the versioned protocol policy. Model names and group
preferences alone do not establish endpoint or feature support. Unknown and
ambiguous identities retain endpoint evidence, model and service preferences,
legacy bindings, and the configured fallback. Among exact declarations,
confirmed evidence wins, then the latest observation; remaining ties use
model preferences, the service profile, and stable protocol order.
The client protocol selects the adapter for that upstream. Native forwarding
applies when the two contracts match; otherwise the existing typed bridge
converts the request and answer.
Relay converts requests, responses, streams, and WebSocket traffic between the
client and selected upstream protocol. Cache-write pricing applies only to
routes whose upstream is Messages.

Converted Messages requests validate standard `metadata.user_id` and preserve
it as OpenAI request metadata on Responses and Chat Completions routes. Those
OpenAI formats retain bounded string metadata when converting between them;
Messages accepts only `user_id`, and Gemini rejects nonempty metadata rather
than losing it. Valid Messages `cache_control` hints (automatic, system/text,
images, function declarations/calls, and tool results) are advisory cache
optimizations: converted routes validate `ephemeral` and `5m`/`1h`, preserve
conversation content, and use the upstream provider's independent cache policy.
Foreign cache hints are not forwarded as generation controls and do not imply
equivalent TTLs, cache hits, or pricing. Matching native routes preserve their
wire fields. Required context edits, unsupported hosted tools, and unknown
generation controls still fail before dispatch; bounded known parameter names
are returned without logging payload contents.

JSON, gzip, and zstd request bodies share one bounded decoder. JSON and streaming
paths share Completed, Incomplete, and Invalid outcomes. Responses terminal
events must match the body status and contain an output array. Chat requires
every observed choice to finish before `[DONE]`; Messages requires its complete
block lifecycle and `message_stop`. Native Gemini requires a supported finish
reason for every observed candidate; a bare `[DONE]` cannot establish success.
Converted Gemini routes support one candidate and drain late usage until EOF,
with a 30-second deadline only after the finish reason. Transport failure does
not become normal EOF. Incomplete answers keep reported usage but do not
recover route health, persist affinity, or become reusable continuations.
A native WebSocket may continue its last incomplete response on the same live
connection and physical owner. This temporary binding is cleared when the
connection closes, changes upstream, or starts an independent request; it
cannot restore a continuation after reconnect or restart.
Refusals preserve their client representation and cannot authorize retry.
Converted refusals do not create reusable continuation state.
Failed bootstrap and buffered collection retain usage from complete frames.
Partial cumulative updates merge known counters; null values do not erase them.
When counters change without a new total, Relay derives the total from the
updated counters with checked arithmetic. A reported total stays unchanged.
`stream_incomplete` means the transport ended before valid completion;
`stream_invalid` means its events or lifecycle violate the selected protocol.
`response_incomplete` means the provider explicitly ended an incomplete answer.
None of these outcomes proves that another generation is safe.
The upstream WebSocket bridge handles one response and
at most one named lane per connection; full multi-lane multiplexing remains open.
Every upgrade attempt, including a 401 refresh retry, consumes the logical
request's wire budget; only sending a generation consumes its dispatch budget.
HTTP-only observations expire after five minutes and are capped at 4,096.
They are scoped to the endpoint, model, tier, route headers, credential, and exact
authorization incarnation. A runtime/proxy replacement starts with fresh
observations. Converted routes use their declared transport without recording
a native WebSocket failure. Unknown sends remain non-replayable.

**Basis Points** is available only to Excel OAuth credentials. It uses the
connection's quota and rotation slot. Ordinary SSE text is incremental; tools
are withheld until the complete terminal response passes local validation.
JSON upstream responses and explicit local output validation remain buffered.
Native strict structured output, remote image URLs, opaque
`previous_response_id`, and non-standard service tiers are rejected. Ordinary
ChatGPT credentials stay on native Responses regardless of legacy preferences.
Codex Responses Lite requests may use this route: Relay applies its serial-tool
normalization and translates the item-level tool catalog locally, without
forwarding the internal Lite transport header to the Basis Points endpoint.

Before BPS dispatch, compact Responses message items are expanded to the explicit
`type: "message"` form and string content becomes typed text blocks. Roles,
content order, images, and existing message extensions are preserved. This also
applies to history produced by the shared Messages, Chat, and Gemini codecs;
it does not change native forwarding to other providers. Message author and
recipient attribution becomes explicit context without changing the role.
Plaintext collaboration messages become lower-authority user context, never
developer instructions; encrypted agent messages remain unsupported.
Tool-result images retain their call association and relative order through
marked attachment messages. Existing inline-image validation, credential-scoped
upload, and file-ID handling apply; Relay does not fetch remote image URLs.
Synthetic context does not advance the logical user turn or reset iteration.
A BPS HTTP 422 with
`Invalid request body` is a client-input rejection even when its envelope says
`server_error`: Relay returns it without another account attempt or a health
cooldown.

BPS metadata contains only Relay-owned `task_id`, `turn_id`, and
`agent_iteration`. Arbitrary client metadata, including Messages `user_id`,
stays on the original request and is restored in the translated Responses
JSON or translated SSE. It cannot override BPS transport fields or internal
prompt/tool settings. Other providers retain their native metadata contract.

Structured output stays unsupported by default. Responses callers may explicitly
set `relay_bps_output_validation: true` with `text.format` `json_object` or
`json_schema` for instruction-based generation followed by local validation.
This is not provider-native constrained decoding; `strict: true` is rejected.
The original schema stays intact, is limited to one MiB, and resolves only
self-contained references through the existing offline validator. Relay holds
the whole answer, including for streamed clients, and rejects invalid JSON,
schema mismatch, oversized text (16 MiB), and incomplete output without another
generation. Valid tool turns and refusals remain protocol items, not invented
JSON. Other providers' native format contracts are unchanged.

Known Free plans cannot be imported or signed in as Excel / Basis Points.
Validation checks normalized plan metadata and available token plan hints;
declaring a paid plan cannot override a Free token hint. Unknown plans are not
assumed Free and do not prove paid access. Stored Free BPS accounts cannot send
model discovery or generation requests. A live plan change clears cached BPS
permissions and disables Free routing; quota and policy updates cannot re-enable
it. Native Free accounts remain eligible under the ordinary runtime rules.

Before publishing or executing an Excel route, Relay reads the provider access
envelope with `include_models=true`. `allowed`, restricted models, availability,
enabled state, picker state, and policy state determine executable models; the
discovery preserves inventory and ignores duplicate or malformed model rows.
Presentation applies the shared catalog policy. The result is cached for
60 seconds per account and exact token
incarnation. A token change or an access failure never reuses an older
permission result; a proven 401 may refresh the token once. Discovery is
bounded by a five-second timeout and a one-megabyte body limit.
Failed reads are coalesced too. A throttled access check keeps HTTP 429 and
the remaining Retry-After instead of restarting the deadline or reporting
an unknown model.

Generation 401 handling is separate from discovery recovery. It expires only
the rejected access-token incarnation, preserves the Excel refresh token, and
returns the failed request without another generation or account fallback.
The next request prepares fresh authorization and rechecks model access before
dispatch. Token invalidation and refresh share the authority slot; a delayed
401 cannot expire a refreshed token or a newer login with identical token text.
Desktop refresh rereads the stored credential after the provider reply, and
server persistence is bound to the login's secret reference. A newer validated
login wins over a late `invalid_grant`; terminal refresh failures require
reauthentication only while they still belong to the current login. Transient
failures preserve credentials. Recovery does not change operator disable,
draining, or pool membership, and cannot recreate a previously deleted token.

BPS account cards and member settings expose local credential status, access
expiry, refresh-token presence, and the next recovery action without token
material. `not_expired` is local expiry evidence, not an upstream authentication
result. Refreshable expiry offers preparation on the next request; missing or
terminally invalid credentials offer sign-in, and transient failures offer retry.

Declared Excel efforts restrict the reference catalog's modes for that account.
Request encoding and access checks share BPS alias normalization, including
`max` to `xhigh`; an omitted effort leaves the provider default intact.
Catalog filtering takes an access snapshot before locking the scheduler.
The final dispatch still checks the current credential and model permissions.

Excel defaults to a 120-second upstream progress deadline. SSE comments and blank
lines do not reset it; fragmented event data does. A terminal event ends
collection without waiting for EOF. Timeout and body failure retain already
reported usage and never authorize another generation. Completed-response
translation errors are terminal, including malformed client-tool envelopes.
Incremental BPS normalization runs before the shared client protocol bridge.
It releases no tool subset from an invalid terminal response and no tools from
failed, incomplete, cancelled, or truncated streams. Call/item identities seen
in incremental events must agree with the authoritative completed response.
EOF or `[DONE]` without a terminal response is an error. Frames are bounded to
16 MiB and output/tool tracking to 1024 items. The first terminal stops upstream
consumption; downstream cancellation drops the reader. Usage, served-model
checks, first output, and completion timing observe raw upstream events before
translation; synthetic client events are not timing evidence.

BPS normalization follows the shared Responses terminal contract, including
`response.done` and named SSE terminal envelopes. The terminal body must still
validate; an incomplete or cancelled outcome never becomes completed success.
Incomplete JSON responses also withhold executable tools while preserving text,
reasoning, incomplete details, and known usage. Parsed terminal responses and
withheld output events pass directly through local validation without another
JSON/SSE serialize-parse cycle. Adapter failures retain a fixed local parameter
in bounded request diagnostics; provider bodies and tool arguments are not logged.
Tool-envelope diagnostics distinguish invalid argument JSON, a non-object
function payload, and a client-schema mismatch. They never repair or infer
missing arguments, execute a partial tool subset, or authorize regeneration.

Excel refusals use the shared scheduler, including refusals inside HTTP 200
responses. BPS 429 pauses all models of that Excel connection. Without a
provider deadline, generation and access checks use a five-second pause;
longer Retry-After/reset hints are preserved. A bare transport refusal does
not change saved quota or account health. Completed-response conversion
errors also do not change account health. Execution certainty is retained:
a refusal does not authorize replay after unknown or completed execution.
Neither transport choice nor model-name checks prove response quality.
`basispoints_model_access_changed` and `model_not_allowed` pause the affected
account's canonical upstream model for at least 30 minutes. Aliases share that
pause; other models and native connections keep their own eligibility. The
pause does not disable BPS, change membership, or authorize account fallback.
Cooldown state is process-local.

`relay-core` also provides a manual `quality_probe` example for controlled
observations, not a background degradation detector. It shares one randomized
batch across compared model IDs, using bounded local grid-path and weighted-XOR
solvers instead of another model as judge. Only validated answer shapes are
compared; transport failures, refusals, malformed answers, and incomplete output
are inconclusive, not incorrect. Reports contain question IDs, verdicts, timings,
and whitelisted numeric usage, never prompt/answer bodies or credentials.
The probe requires explicit request consent and an existing scoped local request
key, accepts only literal loopback HTTP endpoints, and caps one run at 4 models
by 8 samples, sequentially without retries. This caps requests, not tokens or
money: BPS does not enforce `max_output_tokens`. The probe never changes policy
or disables members, but requests still pass normal gateway admission, quota,
usage, and health handling. Mixed-pool results do not identify an account or
upstream protocol; use an isolated preset/scoped key for controlled comparisons.
One run cannot establish absence of degradation. Operator steps and cost limits
live in [CONTRIBUTING.md](../../CONTRIBUTING.md#manual-quality-observations).

The member picker and account actions offer direct pool addition for native
ChatGPT and eligible Excel / Basis Points connections without an account-kind
warning. Mixed imports can add both kinds and API sources in the same action.
Quick setup offers both ChatGPT and Excel / Basis Points sign-in.

## Request timezone

The shared request-timezone rule is disabled by default. An explicit account
disable always prevents rewriting. Without an override and with the shared
rule off, the account uses its assigned proxy's configured timezone; without
one, the client body is preserved. With handling enabled, the selected source
is either the account timezone (default `America/New_York`) or the proxy
timezone. A proxy source never falls back to the account timezone.
Choices remain saved while rewriting is disabled.

The desktop and user-managed server validate IANA zones in Rust. Updates compare
the expected policy and apply to the current runtime without rebuilding it;
stale edits fail.
The remote capability is `request_timezone_v1`; older servers hide the control.
The shared default-source selector lives in Access → Accounts with the ChatGPT account
settings. Each account's action menu in Connections → Accounts opens a searchable
timezone picker with date-aware UTC offsets and a preview of the effective
replacement. Auto follows the shared rule; Set and Don't replace are explicit
account overrides. The picker groups aliases using the WebView's canonical
timezone identifiers, keeps all aliases searchable, and preserves a saved
alias. It never merges zones just because their current offsets match.
Saving changes only that account's settings, preserving other accounts and
the shared enabled flag.
Bindings stay on their host and are excluded from portable configuration
presets. Applying a preset preserves the existing bindings.

Stored proxy timezones are set manually; an exit-IP check does not infer a
timezone. Clearing a proxy timezone does not require a loaded picker catalog.
Proxy assignment counts include API sources and the common route. Deletion
checks those consumers before mutation. Account detachment and proxy deletion
share the Rust credential/pool save and runtime rollback operation; proxies
used by an API source or the common route must first be replaced there.

Local save/delete commands acknowledge only after durable storage and runtime
publication succeed. A prepared replacement runtime is published on the existing
listener when its port is unchanged; open connections use the current router
on each request. Already dispatched response streams can finish, while the
retired runtime rejects delayed dispatches. A port change keeps the bind and
rollback path. Client catalog projections run separately in one coalescing
desktop queue, without holding the setup lock or delaying the saved result.
Projection failures remain visible as catalog warnings, not failed saves.
Stored-proxy account selection reconciles the pool once per operation.
Runtime reconstruction initializes only missing token-authority registrations;
credential admission and renewal remain responsible for updating existing
registrations. Editing an unrelated route does not wait for their OAuth I/O.

Connections presents stored proxies with the shared account/source card layout.
Each card retains its explicit connection check, observed exit IP and location,
manually chosen timezone, assignment counts and selection. Opening the page does
not run a check. Proxy check status appears in the upper-left icon with an
accessible tooltip, without a repeated status line or unset/unused placeholders.
Failed checks retain their error reason; the latest check time remains in the
diagnostic tooltip. Import results retain their full diagnostic status.
API-source cards describe source readiness, missing credentials,
disabled/error, draining and runtime recovery states rather than account rotation.
Readiness is not proof of a live API probe and is independent of pool membership.
Edit and Launch are adjacent footer actions; the overflow menu contains only
enable/disable and delete.

The selected account's prepared body is rewritten on Responses HTTP, native
WebSocket turns, and account endpoints such as compaction. Only existing,
unambiguous `<timezone>` fields inside user `<environment_context>` blocks and
existing web-search `user_location.timezone` fields change. Ordinary text,
dates, non-user messages, missing fields, system timezone, and API-source bodies
stay intact. Each attempt uses its own account's policy. This option does not
establish response quality or provider acceptance.

## Tool catalog

On native routes Relay forwards the client's tool catalog unchanged. It does not
add deferred tool search, hide or rename tools, or execute a local relevance
search. Stored legacy optimization flags do not change new requests. Usage may
record catalog size and mode as history; those fields are not an execution
permission.

Bridged routes (a Responses client on a Chat Completions, Messages, or Gemini
upstream) can only declare function-style tools. Relay sends the catalog (root
`tools` plus historical `additional_tools` and `tool_search_output` definitions)
as functions. Current root declarations override historical definitions;
namespace children merge by name, including nested namespaces. The full dotted
namespace and ancestor descriptions survive flattening and tool roundtrips.
Duplicate declarations within one catalog
item are rejected. Custom tools get a single
`input` string, namespace children are flattened to opaque names, and hosted
tools such as web search fail explicitly because the upstream cannot run them.
An eager `defer_loading` declaration keeps its schema callable. Only direct
client execution is representable: empty or non-direct `allowed_callers`
policies fail before dispatch rather than granting an unintended caller.
Hosted Responses search history supplies loaded definitions, but client-executed
`tool_search_call` and `tool_search_output` are not hosted metadata and currently
fail conversion explicitly. Native Responses preserves them unchanged.
Responses and streams restore the client's tool kind, name, and namespace. A
forced tool choice that names a tool that was not declared still fails before
sending. A restricted or empty allowed-tool set never enables the full
catalog. Selection restrictions reset each request; a continuation that omits
the catalog retains its previous definitions. An empty callable catalog omits
provider tool-choice and parallel-call controls.

For a Messages client using client-owned tool discovery, cross-protocol bridges
validate the complete function catalog before applying `defer_loading`. Only
non-deferred definitions, explicitly referenced tools, retained historical calls
and a named forced choice enter the upstream callable catalog. References must
name current root definitions and occur inside successful user `tool_result`
blocks; call/result pairing still follows the ordinary conversation validator.
The bridge lowers each `tool_reference` to compact JSON text in the same result
and supplies its exact schema once in the callable catalog, not in prompt text.
Discovery, execution and permissions remain client-owned: Relay adds no search
engine, guessed relevance or tool execution. All-deferred catalogs, missing
definitions, invalid hidden definitions and deferred cache breakpoints fail
explicitly. Provider-hosted search tools are not emulated; native Messages
forwarding preserves their original wire contract. Ordinary non-discovery
requests retain the existing path. Foreign tool-list changes can invalidate
provider prompt caches; this bridge does not promise Anthropic-equivalent cache
placement, pricing or a measured reduction for every session.

Parallel calls stay in one assistant turn. Results must match outstanding call
IDs, precede ordinary user text, and answer every call before the next model
request. Tool-discovery control items do not split that result group. The
Messages and Gemini bridges retain historical call kinds and
namespaces separately from the current catalog, so replacing a declaration
cannot change an earlier result's meaning. Unknown, repeated, or missing
results fail before sending; generated calls cannot reuse an ID from the
conversation history. A request that disables parallel tool calls rejects a
second generated call in both JSON and streaming output.
Allowing parallel calls does not require a model to produce several calls;
a model declared as sequential remains eligible for that request.
Gemini has no native parallel-call switch; its bridges keep that request limit
locally and reject a second call before forwarding it. The limit resets to
the protocol default when the next request omits the control.

Tool results retain text and supported image blocks. Messages keeps `is_error`;
conversions to Gemini preserve errors in the response object and put inline
media in sibling `functionResponse.parts`. Unsupported result content is
rejected rather than discarded. Gemini keeps a stable client call ID separately
from the optional provider ID and restores the provider ID on calls and results
sent upstream. Thought signatures remain on their original native Parts;
signed and unsigned parts are not merged. Gemini continuation replay omits only
bare `{"text":""}` parts; empty parts with signatures or metadata and whitespace
text remain in their original order. Gemini bridge schemas remove only the
harness `encrypted` annotation at schema positions, preserving validation
constraints, property names, and data in defaults, const, enum, and examples.
Streamed typed arguments and singular JSON-path fragments are assembled into
one valid object before Responses
argument events are emitted. The path parser supports bracket keys and array
indices, not the full JSONPath query language. Custom-tool streams emit the
original text instead of the bridge's JSON wrapper.

Responses item trace metadata (`internal_chat_message_metadata_passthrough`)
accepts only typed `turn_id` and `create_time` fields on portable message,
reasoning, and tool items. These trace fields do not enter upstream model input
or continuation history keys. Unknown non-null metadata remains an explicit
parameter error; native forwarding retains the original metadata.

Continuation state separates portable history from provider-owned reasoning:
Responses encrypted items, Messages thinking signatures and redacted blocks,
and Gemini thought signatures. Native state is replayed only to its original
source, credential, endpoint, model, and protocol. Full Responses histories
identify the same bounded local snapshot through a portable history key even
without `previous_response_id`. Restoring that snapshot replays the original
native assistant blocks and appends only new results, preserving call IDs and
avoiding duplicate historical turns. Conflicting snapshots are rejected.
For Messages clients, converted public reasoning summaries are emitted as
unsigned `thinking` blocks in JSON and as `thinking_delta` events in SSE.
They are displayable portable summaries, not Anthropic-signed thinking.
Messages clients may replay these summaries with an absent, null or empty
`signature`; none carries opaque state. Nonempty foreign signatures and malformed
signature values still fail explicitly on converted routes; native Messages
forwarding is unchanged. Empty signatures neither grant continuation ownership
nor allow a missing native tool turn to be reconstructed from summary text.
Relay never fabricates a signature or decrypts private reasoning. The tool
result resumes the original scoped native turn, including its opaque state;
a public summary cannot replace missing continuation state. A provider that
returns no public summary is not made to expose private reasoning.
BPS JSON-to-SSE and withheld-output synthesis emit complete reasoning-summary
part/text events before subsequent tool items. The Messages bridge therefore
retains summary/tool order instead of discovering an earlier thinking block only
at the terminal response. Opaque-only reasoning stays invisible but replayable;
terminal validation and atomic tool release are unchanged.
A new ordinary user message can carry the visible history of completed
foreign turns, including public summaries and paired tool calls/results.
Foreign ciphertext is not sent to the new provider. An unfinished tool loop
still requires compatible native state and cannot be hidden by a new message.
Plain tool history without that state is allowed
for Chat upstreams or a confirmed non-reasoning model; other routes require
the missing native state explicitly. Conversions reject unsupported meaningful
output extensions rather than silently dropping them. Native routes preserve
those extensions. Cache breakpoints skip thinking and empty text blocks.
Manual thinking budgets cannot be combined with forced tools, and explicit
output limits are preserved. Messages streams emit one content block at a
time; Responses text events include complete added and finished parts.

Excel translates client tools through its native transport envelope. Current
root declarations override historical `additional_tools` and `tool_search_output`
definitions; the latest history fills omitted definitions. Original function
schemas validate both declarations and returned arguments with external
reference retrieval disabled. Tool instructions carry the original minified JSON
Schema, including nested required fields, descriptions and constraints, rather
than a separate shorthand schema language. Encrypted agent assignments are
rejected before their technical history is removed. A single strict direct
client-tool call emitted in place of the envelope is also normalized without
executing code; unknown tools, extra calls, trailing text, and invalid argument
types remain rejected.
JSON and SSE schema mismatches identify the argument-validation failure without
including tool arguments or provider response bodies. Tools remain atomic and
are not released or automatically retried after a failed validation.
This adapter reconstructs its tool catalog and replay from each supplied
history; it does not require a BPS tool-session store or Redis. Full history is
required when earlier declarations or results are needed. An opaque
`previous_response_id` remains unsupported, and process-local routing state
does not establish distributed scheduling.

## Usage and prices

Desktop migration 33 and server migration 42 add a separate attempt history.
The ordinary report and its rollups still store one final row per logical
request. An exact `requestId` usage query returns retained attempts across
members, including pre-output compatibility repairs and HTTP authorization
replays. `requestIdQuery` remains a substring filter for the ordinary report.
Exact history pagination counts attempt rows; its cost/totals projections
remain those of the logical request and must not be summed as attempts.
History expires with the corresponding raw request, is removed on clearing
usage, and purges records belonging to deleted accounts. Migrations seed only
the surviving final records; prior discarded attempts cannot be reconstructed.
The optional `routing.attemptOutcome` records the executor's actual decision.
Missing decisions remain unknown. Authorization replay records are audit-only
and do not create another auth/health vote. This telemetry changes no retry
policy and never makes uncertain execution safe to replay.

Request details use a dedicated report layout for the model, connection,
timestamp, final outcome, and available metrics. Wide windows place identity
beside a metrics grid; narrow windows stack the same content. First-output
timing precedes total response time; a single known timing stays compact. Protocol,
transport, model parameters, token breakdowns, and the final provider error
message are visible without expanding sections. Errors use a soft surface
without a side stripe. Technical details expand into separate identifier,
response/error, route, and tool groups, retaining copy controls, original
codes, counters, warnings, and nested debug context. Both themes inherit the
launcher palette. Empty diagnostic groups are omitted.
Saved sends appear chronologically in Request activity when there are repeats
or history is unavailable or incomplete; inspecting an earlier send never
replaces the final report outcome.
Recorded decisions explain retry or stop behavior, and missing decisions/history
stay explicitly unknown. For a single failed send ending in a retry decision,
the missing-result notice is kept in technical details. The report groups by the
exact Relay request ID, never by time, model or client session. Separate client
requests retain separate records. Counts, tokens and cost use final request rows
rather than adding the history's repeated projections.

Usage stores the selected member, model IDs, protocol, status, timing, token and
cache counters, service tier, and a redacted provider error envelope. API-
equivalent cost is informational and never a subscription debit, customer
charge, or routing input. These records come from the upstream payload, not from
the body returned to the client.

Excel/BPS buffered requests measure first generated output while collecting the
real upstream SSE, and total time when collection ends, including partial
failures. Heartbeats, lifecycle events, empty deltas, and terminal snapshots do
not establish first-output timing. Synthetic client SSE is not timing evidence.
For complete JSON or a stream without an observed output delta, first-output
time stays unknown. The Usage table's first/total column displays `— / total`
when only total time is known; historical unknown timings are not reconstructed.

Error details retain validated provider request IDs from the error envelope or
allowlisted response headers (`x-request-id`, `openai-request-id`, `request-id`).
An error-envelope ID is appended once to the client-visible error message;
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
Streaming usage merges raw cumulative snapshots before converting totals,
preserving earlier cache counters when a later update omits them. Derived
arithmetic is checked; overflow remains unknown rather than retaining an older
lower total. Reported usage is retained even when completion validation fails.

Price resolution is separate from execution: observed provider prices first,
then an exact LiteLLM provider/model record, canonical family reference, and
manual fallback. Input, cached input, cache writes, output, image, and request
prices remain separate; missing values stay unknown rather than zero. Standard,
Flex, Fast (`priority`), and Ultrafast have separate schedules selected by the
observed upstream tier. A missing schedule or component remains unpriced; it
never inherits Standard rates.
Provider and manual token quotes retain their published tier schedules and
long-context rates above 100k, 128k, 200k, 272k, and 512k input tokens, including
cache components and explicit zero rates. The price editor has independent
tier and context selectors. Context choices come only from that model's
published or saved manual price bands; flat/unknown quotes do not create
thresholds. Maximum context metadata never creates a price band. These
selectors choose prices to inspect/edit, not a request's price class.
Actual per-request input usage determines the range, with a strict greater-than
boundary; the highest exceeded published band applies. Aggregate usage remains
grouped by price class and context range. Desktop migration 32 reclassifies
retained request logs without duplicating their rollups. Migration 34 moves
retained Ultrafast contributions out of Standard without changing token totals.
Already archived totals retain their historical classification because their
individual request sizes and tiers are unavailable. Missing components stay unknown; a published
long-context tier can be used even when its base tier has no published rates.
Cache lifetime is displayed as an estimate when the provider reports it or the
model's documented fallback applies; it does not prove a future cache hit.

## Profiles and user-managed server

Profile operations follow inspect, protected snapshot, attach/apply, verify, and
restore. Automatic rollback changes only unchanged Relay-owned fields and never
overwrites a newer manual sign-in. ChatGPT OAuth writes the native token fields
and clears Relay overrides so Codex can discover its models. Pool activation
writes the Relay provider and validated catalog through the live local endpoint.
Native Codex account cards retain their own context evidence independently of
reference API metadata. An account route without its own card does not inherit
a reference API maximum. Pooled API routes and direct API profiles publish
known reference maxima separately from the installed card's provider default.
The selected client history budget is a projection, not a change to the
provider's capabilities or its original default. Codex owns its history and
explicit client overrides remain user-owned and are preserved by Relay.
Pool → Model Rules stores a context choice per model. Short preserves the
model's published default window, capped only by a known hard maximum. A known
short/long pair is never replaced by a percentage or a pricing-band threshold.
Only when no default is published does Short use 25% of a known hard maximum,
rounded down with a 1,024-token floor that never exceeds that maximum.
Extended uses the full known maximum. With no hard maximum both preserve an
available provider default, without
repeatedly reducing it on later catalog projections. With no window evidence,
the window stays unknown rather than assuming 128k. Custom accepts 1,024 to
2,000,000 tokens even when the catalog has no window; Relay clamps it only to a
known hard model or route maximum. A published default by itself is not treated
as a hard maximum. Custom changes the managed model card's selected window but
does not claim an unknown hard maximum. Native budgets derive only from the
account's own card; without an exact card only explicit Custom supplies a window.
Management publishes original default/maximum window evidence separately from
the selected policy. Native windows come from exact owning account cards;
API-only models use reference API maxima without borrowing context defaults
from the installed Codex catalog. Installed Codex metadata supplies only Ultra
orchestration, not default or maximum context windows. Empty native evidence
never falls back to a reference API maximum.
The UI shows token counts without percentage captions, preserves a fixed
single-line control size across modes, and validates custom input against that known
maximum as well as the 2,000,000-token editor ceiling. OpenCode projects all
three policies, not just Custom, and never advertises an input limit above the
selected context. Request bodies are not silently truncated to fit a budget.
Relay sets automatic compaction to 90% of the selected window, lowered only by
known input limits and a known hard maximum minus the largest output limit.
There is no user-facing compaction setting. Root `model_context_window` and
`model_auto_compact_token_limit` settings are saved in Relay recovery storage
and removed from the root and active named profile while the pool manages
context. Restore preserves newer user edits. The per-model policy is projected
through the managed catalog. Updates change the running Relay catalog without
rebuilding the scheduler or interrupting active requests. Desktop refreshes
the managed catalog atomically even while the client runs; profile/config
changes remain deferred until the client stops. Clients that load that file
only at startup still require relaunch. Model Rules displays a persisted Codex
reload or catalog-refresh status as a compact accessible tooltip indicator;
saving context never closes or
restarts applications automatically. OpenCode may need a new session to read
its updated configuration. Claude Desktop controls its own history; Relay does
not promise enforcement of this client-side budget there. Custom input saves
after a short pause, on blur, or on Enter; invalid values never replace the
saved policy, and Escape cancels the current draft. Rapid per-model edits are
serialized with the latest queued choice winning, without disabling the input.
Backups stay in Relay recovery storage; credential snapshots use the OS secret
store. OpenCode keeps its original JSON/JSONC configuration and restores user
edits through a semantic merge. Claude Desktop uses its separate `Claude-3p`
configLibrary, keeps foreign profiles and metadata, and writes only the
Relay-owned UUID profile plus its selected marker. Desktop ignores non-UUID
selected IDs. An orphaned `zenith-relay` entry and profile file left by an
older Relay version are cleaned up on connect only when the static gateway
configuration matches this Relay's current endpoint and key, with no foreign
settings. A display name alone never authorizes deletion or adoption. UUID
collisions block reconnect without overwriting foreign data. If only the recovery
record was lost, the same binding checks permit reclaiming the UUID profile and
recreating recovery, even for an unchanged catalog. An unknown previous selection
is not guessed; restore clears Relay's selection rather than selecting Relay itself.
Its explicit model list preserves the validated pool catalog's Messages-compatible
models, labels and existing order. Claude Desktop validates route names before
inference and rejects other vendors' names, so its profile uses bounded, stable
route IDs derived from canonical model identity. Plain routes use
`claude-sonnet-5-relay-`; models with a representable native effort scale use
`claude-fable-5-relay-` to activate Desktop's built-in Claude effort control.
Each suffix uses the first 64 bits of SHA-256 in four four-character hex groups,
independent of labels, pool order and enabled reasoning levels. Ambiguous
matches fail closed rather than selecting a model by pool order. Previous
full-digest `claude-zenith-` and `claude-fable-5-zenith-` routes remain accepted;
newly written profiles use the compact routes. Usage displays the observed real
model, retaining the raw client alias only in expandable technical details.
Without a resolved model, an opaque alias is not presented as a real model name.
These are client-only UI/routing identities, not upstream Claude identities.
The gateway resolves both against configured real models, then applies the
same key scope, visibility, rules and admission as the original ID; a raw model
cannot shadow the reserved alias namespace. Upstream requests, context, reasoning
and pricing use the real model. No family pins, 1M context flags or upstream
capabilities are inferred from the UI identity.
The profile has exactly one entry per enabled Messages-routable model; reasoning
levels are request parameters, never additional models or alias suffixes.
`output_config.effort` and `thinking` use the shared Messages request policy and
typed bridge, retaining the real model's supported levels and pool restrictions.
No effort is inferred from a route name. Removed effort-suffixed aliases are
unavailable; reconnect or launch through Relay to refresh an older profile.
Desktop's native Effort picker uses Anthropic's
[signed catalog or bundled metadata](https://claude.com/docs/third-party/claude-desktop/configuration#models).
`modelCatalogUrl` only mirrors signed files; Relay does not replace them, modify
the client or disable signature verification. As with
[CC Switch's Claude role mapping](https://github.com/farion1231/cc-switch/blob/797e0276ffc20e54cbdf55e3de84505d77231c91/src-tauri/src/claude_desktop_config.rs),
the client-facing Claude identity is translated back before upstream routing.
The unique opaque suffix preserves one independently labelled entry per real
model, rather than limiting the pool to a few fixed role slots. Desktop owns the
native `low`, `medium`, `high`, `xhigh`, `max` scale. `maxEffort` caps it to the
longest prefix present in both the model's allowed levels and its executable
Messages routes. Gaps and additional levels such as `ultra` are not invented
or remapped; models without a representable prefix retain a plain route and
no native picker. Explicit requests still pass the shared real-model policy,
including after a profile becomes stale. Compatibility with the native picker
depends on Desktop's bundled/signed metadata; verify it when Desktop changes.
Profile creation rejects
more than Desktop's 200 real models before closing Desktop or writing its
profile; it never silently truncates the catalog.
The owned Desktop profile opts into `toolSearchEnabled` so the client can load
MCP schemas on demand instead of including the full catalog in every prompt.
The shared Messages bridge supports the client-owned discovery shape above;
native routes still require upstream support for the original Messages feature.
This is Desktop's documented setting, not a global environment override or a
removal of tools, skills, agents or browser capabilities. Reconnect or launch
through the updated Relay to refresh an old owned profile, then start a new
Claude session; an already running old Relay is not upgraded by source edits.
Recovery ownership and the previous profile selection remain unchanged. Lost
recovery may reclaim an otherwise exactly bound older profile that omits this
setting, but not one with an explicit conflicting setting or foreign fields.
Claude Desktop reads this configuration at startup. Connect validates the pool,
key, catalog and profile ownership before closing Desktop, then repeats ownership
checks and writes the profile. Only the caller's launch preference starts it again.
Standalone launch also validates the selected profile and running pool, refreshes
the catalog, resolves the launch target before restarting, and confirms a Desktop
process appeared instead of treating an Explorer spawn as success. Restore
refuses when the Relay-owned profile or entry was changed outside Relay;
foreign metadata changes and newer manual selections are preserved.
Windows launch and running-state detection recognize both the current
`AnthropicPBC.Claude` MSIX identity and the older `Anthropic.Claude` identity,
without treating Claude Code CLI processes as Desktop.

Application choices place ChatGPT first, Claude Desktop second, and OpenCode
third. The shared modal picker arranges logos above names in a horizontal row.
Access uses the same compact vertical navigation for Applications and Accounts,
with the selected settings alongside it, stacking below on narrow windows.
Local Quick Setup can attach Claude Desktop without
launching it; remote setup does not offer an active local Desktop connection.
The shared application picker only selects the client. Connecting from the
Pool page launches it after a successful attachment by default; Settings owns the
persisted opt-out, including previously saved choices. Launch runs before
the renderer refresh and never after a failed attachment. Explicit launch
actions continue to open the client regardless of this pool preference.
Access starts each application's settings with an application-path field,
a folder picker button and Save, without a separate reset button or explanatory
text. Connection/status actions are not duplicated from the Pool page. A native
preference under `data/preferences/application-launch.json` stores one local
executable override per application. Clearing the field and saving restores
existing automatic detection; an invalid saved target fails instead of silently
selecting another installation.
Native validation rejects relative paths, command arguments, shortcuts and
terminal clients; macOS bundles resolve to their desktop executable. Launch
resolves the validated target, and OpenCode resolves it before stopping a running
instance. Browsing and saving never connect, launch or stop a client. These
preferences remain on this computer in every mode and are not pool/server
configuration or preset data. Claude's manual disconnect/restore action lives
in local Recovery, uses the same shared snapshot layout as ChatGPT and OpenCode,
and preserves the same profile ownership checks. A named snapshot before
connection records only prior profile selection metadata — no foreign profile
secrets are copied — and is consumed on restore without changing Claude files
or stopping the application. Restoring an attached profile validates ownership
before closing Desktop and does not automatically relaunch it.

Remote management is capability- and revision-checked. Presets transfer pool,
model, ordering, and price settings, while secrets and account credentials use
separate explicit transfer operations. A preset or snapshot is not a hidden
two-way synchronization mechanism.

## Not provided by the current implementation

The current Relay Server is a single-owner user-managed runtime. It does not
provide hosted multi-user tenants, user accounts, customer-scoped API keys,
balances, payments, or customer billing. These are planned in
[Hosted multi-user Relay](ROADMAP.md#hosted-multi-user-relay), not available
features. Existing pool request keys and usage records do not represent
customer identity or a wallet ledger.

The server's SQLite affinity store already persists response-owner and route
fingerprints. Native replay and bridge continuation payloads remain bounded
in-memory state; those fingerprints alone cannot restore lost native blocks.
The proposed hosted PostgreSQL checkpoints, optional encrypted payload policy,
SDK idempotency errors, and wallet transactions are specified in
[HOSTED_RELAY_ARCHITECTURE.md](HOSTED_RELAY_ARCHITECTURE.md#durable-continuations-and-restart-recovery),
not provided by this local/single-owner implementation.

Distributed multi-server scheduling, fingerprint spoofing, sharing concealment,
account resale, and automatic migration of opaque provider conversations
between accounts remain outside the planned first hosted release.
