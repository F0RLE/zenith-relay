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

Connections owns saved accounts and API sources. Access groups the pool API,
application setup, and account behavior settings in one page. Its Local API tab (Server
API for a remote runtime) owns the listener, request key, port, and protocol-wide
route recovery. Accounts contains settings that affect account requests,
including ChatGPT model-substitution checks. Sign-in and post-login setup remain
in Connections; sign-in does not automatically enable pool membership.
Application settings own the client profile, context, compaction,
background tasks, and WebSocket controls.

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

Default groups are OpenAI, Anthropic, Google, xAI, then other providers
alphabetically. Within a group, native ChatGPT inventory follows ascending
official `priority`, with stable ties and unranked entries last; Basis Points
and API inventories retain response-array order. A member editor keeps that
member's prepared order.
When pool inventories overlap, native accounts establish the order of shared
IDs, then API sources append remaining IDs. Account-only runtime catalogs keep
this order after filtering out API sources. Members outside a request key's
scope cannot establish its catalog order. Manual order overrides the default,
including when reference metadata is unavailable. New models join their saved
group without moving existing manual anchors. The UI moves models within a
group and groups as whole blocks. Model order and member rotation order are
independent.

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
provider capability. Hosted tools are the exception (see Tool catalog): a
bridge drops them rather than failing the whole request.

Translated reasoning controls share one model-aware renderer for every client
protocol. Missing controls preserve provider defaults. Adaptive Messages routes
can carry `xhigh` when the model reference and route allow it. A model declared
budget-based requires an explicit token budget; its token limits do not define
effort levels. Legacy budget bindings without method metadata retain Relay's
compatibility mapping. Explicit output limits stay intact; incompatible controls
fail before sending. Native payloads retain their controls. Usage reads the
requested and sent controls from the exact protocol fields, including Gemini
thinking and BPS's flat effort field.

Each model has one upstream protocol, chosen before client catalogs are
projected. Selection order is a pinned endpoint (hint or endpoint URL), exact
Catalog or Manual endpoint evidence, the model family's native protocol, a
known service-host profile, legacy bindings, and the
configured fallback. Explicit unsupported evidence excludes a protocol at
every stage; if no configured alternative remains, no route is created.
Among exact declarations, confirmed evidence wins, then the latest observation.
Equal observations prefer the model family's native protocol, then the known
service host; the remaining tie uses a stable protocol order.
`ModelMetadataCatalog::native_protocol_for` supplies the family preference:
Responses for OpenAI, Messages for Anthropic, Gemini for Google, and Chat
Completions as a fallback for other known groups. It uses the canonical model
namespace, then a first-party namespace, then the family (`gpt-oss` is not an
OpenAI model). A family preference does not override an exact source contract.
Relay converts requests, responses, streams, and WebSocket traffic between the
client and selected upstream protocol. Cache-write pricing applies only to
routes whose upstream is Messages.

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

**Basis Points** is available only to Excel OAuth credentials. It uses the
connection's quota and rotation slot and buffers a completed upstream response
when needed. Unsupported structured output, remote image URLs, opaque
`previous_response_id`, and non-standard service tiers are rejected. Ordinary
ChatGPT credentials stay on native Responses regardless of legacy preferences.

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
provider's order is preserved and duplicate or malformed model rows are
ignored. The result is cached for 60 seconds per account and exact token
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

The member picker and account actions offer direct pool addition for native
ChatGPT and eligible Excel / Basis Points connections without an account-kind
warning. Mixed imports can add both kinds and API sources in the same action.
Quick setup offers both ChatGPT and Excel / Basis Points sign-in.

## Request timezone

Request timezone binding is disabled by default. The desktop and user-managed
server can save a validated IANA timezone per account; enabled accounts without
a saved choice use `America/New_York`. Rust supplies the catalog and validates the
policy. Updates compare the expected policy and apply to the current runtime
without rebuilding it; stale edits fail. Disabling binding retains choices.
The remote capability is `request_timezone_v1`; older servers hide the control.
Bindings stay on their host and are excluded from portable configuration
presets. Applying a preset preserves the existing bindings.

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
namespace children merge by name. Duplicate declarations within one catalog
item are rejected. Custom tools get a single
`input` string, namespace children are flattened to opaque names, and hosted
tools such as web search are dropped because the upstream cannot run them.
`defer_loading` and `allowed_callers` do not apply upstream and are ignored.
Responses and streams restore the client's tool kind, name, and namespace. A
forced tool choice that names a tool that was not declared still fails before
sending. A restricted or empty allowed-tool set never enables the full
catalog. Selection restrictions reset each request; a continuation that omits
the catalog retains its previous definitions. An empty callable catalog omits
provider tool-choice and parallel-call controls.

Parallel calls stay in one assistant turn. Results must match outstanding call
IDs, precede ordinary user text, and answer every call before the next model
request. Tool-discovery control items do not split that result group. The
Messages and Gemini bridges retain historical call kinds and
namespaces separately from the current catalog, so replacing a declaration
cannot change an earlier result's meaning. Unknown, repeated, or missing
results fail before sending; generated calls cannot reuse an ID from the
conversation history. A request that disables parallel tool calls rejects a
second generated call in both JSON and streaming output.

Tool results retain text and supported image blocks. Messages keeps `is_error`;
conversions to Gemini preserve errors in the response object and put inline
media in sibling `functionResponse.parts`. Unsupported result content is
rejected rather than discarded. Gemini keeps a stable client call ID separately
from the optional provider ID and restores the provider ID on calls and results
sent upstream. Thought signatures remain on their original native Parts;
signed and unsigned parts are not merged. Streamed typed arguments and singular
JSON-path fragments are assembled into one valid object before Responses
argument events are emitted. The path parser supports bracket keys and array
indices, not the full JSONPath query language. Custom-tool streams emit the
original text instead of the bridge's JSON wrapper.

Continuation state separates portable history from provider-owned reasoning:
Responses encrypted items, Messages thinking signatures and redacted blocks,
and Gemini thought signatures. Native state is replayed only to its original
source, model, and protocol. Stateless client histories identify bounded local
state through a history key. Plain tool history without that state is allowed
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
reference retrieval disabled. Simple schemas have a compact prompt rendering;
unfamiliar or complex schemas stay verbatim. Encrypted agent assignments are
rejected before their technical history is removed. A single strict direct
client-tool call emitted in place of the envelope is also normalized without
executing code; unknown tools, extra calls, trailing text, and invalid argument
types remain rejected.
This adapter reconstructs its tool catalog and replay from each supplied
history; it does not require a BPS tool-session store or Redis. Full history is
required when earlier declarations or results are needed. An opaque
`previous_response_id` remains unsupported, and process-local routing state
does not establish distributed scheduling.

## Usage and prices

Desktop schema 33 and server schema 42 retain a separate attempt history.
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
Native Codex account cards retain their own context window, extended maximum,
compaction threshold, and effective-window percentage when reference semantics
are projected; missing fields stay absent. An account route without its own
card does not inherit a reference API maximum. Pooled API routes and direct API
profiles publish known reference maxima separately from the default window
in the exact installed Codex card. The API maximum never becomes the default
window; the client default is capped by a smaller known API maximum.
Without an exact Codex card the default stays absent. By default Relay does not invent a
fallback window, compaction threshold, or effective-window percentage. Codex owns its
history budget and compaction policy; explicit configuration overrides remain
user-owned and are preserved by Relay.
The local ChatGPT application's settings include independent context
and auto-compaction controls. Context offers client defaults, the ordinary
official card window capped at 272k, a 1M budget capped by the model maximum, and a custom
budget. Auto compaction offers application settings, automatic compaction, or a custom
token threshold. Desktop defaults retain the client context window with automatic
compaction. A versioned one-time upgrade replaces the previous application-default
compaction with Automatic, retaining custom thresholds and the context selection.
Later explicit application-default choices survive restarts.
Legacy `short`, `extended`, and `custom` policies retain their previous
automatic-compaction behavior; the `configured` policy records the two controls
separately.
An explicit managed budget projects a per-model window into both the live and
managed catalogs, bounded by the route/card capacity. Context presets do not
derive their budget from economic price boundaries.
Compaction reserves ordinary response room (up to 32k and a quarter of the
application window), respects the hard model ceiling with a reserve based on
its output limit, and adds a further 5% input margin, capped for small windows.
Input limits can reduce it further. This controls client-owned compaction without rewriting
history or selecting an economic tariff. A custom compaction threshold is capped
by the same safe input budget. Client-default compaction retains the native
threshold, capped when an explicitly smaller context requires it. Missing capacity never becomes a
million-token fallback. When the context is client-owned, explicit compaction
preserves the native effective-window percentage and respects its reduced budget.
For a managed budget, Relay snapshots and removes root and configured active
profile context/compaction overrides that would mask model cards. The two keys
are acquired and released independently. Returning either control to client
defaults or restoring the profile restores its saved numeric value,
preserving newer user edits. Conflicting external context edits block
reapplication. An open client defers the managed-file/config refresh until it
closes; the next launch uses the saved preference.
Root `model_context_window` and `model_auto_compact_token_limit` settings
belong to the client application, not individual catalog rows. The client's
window override takes precedence over the card default and is capped by the
selected model maximum; catalog refresh does not change either setting.
Backups stay in Relay recovery storage; credential snapshots use the OS secret
store. OpenCode keeps its original JSON/JSONC configuration and restores user
edits through a semantic merge.

The shared application picker only selects the client. Connecting from the
Pool page launches it after a successful attachment by default; Settings owns the
persisted opt-out, including previously saved choices. Launch runs before
the renderer refresh and never after a failed attachment. Explicit launch
actions continue to open the client regardless of this pool preference.

Remote management is capability- and revision-checked. Presets transfer pool,
model, ordering, and price settings, while secrets and account credentials use
separate explicit transfer operations. A preset or snapshot is not a hidden
two-way synchronization mechanism.

## Explicitly out of scope

Relay does not provide hosted multi-user tenants, customer keys, wallets,
payments, customer billing, distributed scheduling, fingerprint spoofing,
sharing concealment, or automatic migration of opaque provider conversations
between accounts.
