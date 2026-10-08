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

ChatGPT is the shipped subscription connector. Other providers are generic API
sources until a separate connector proves authentication, refresh, entitlement,
quota, usage, recovery, and permitted live-provider behavior. Proxies are
optional and may be shared. Provider names do not select a route by themselves.

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
provider, source order is preserved. Manual model order overrides that default;
model order and member rotation order are independent.

## Statistics and refresh

Source statistics distinguish wallet balance, key allowance, and subscription
allowance and preserve the provider's units. They never decide route
eligibility. A failed refresh retains the last value with a stale state;
unknown is not zero and unsupported means that the adapter confirmed no
supported statistic. Provider spend and Relay's API-equivalent estimate remain
separate.

Models, balances, account quota, and usage have independent refresh jobs. Reads
join an existing resource-scoped job instead of starting duplicate work. A
manual refresh does not cancel shared work. Configuration, credential, delete,
and re-add revisions fence late results. Runtime statistics are not durable
freshness evidence after restart.

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
provider capability.

JSON, gzip, and zstd request bodies share one bounded decoder. HTTP/SSE success
requires the client's terminal event. Gemini requires `finishReason: STOP`;
a bare `[DONE]` cannot establish success. The current upstream WebSocket bridge
handles one response and at most one named lane per connection; it is not full
multi-lane multiplexing. Incomplete responses cannot become reusable
continuations.

**Basis Points** is an explicit optional ChatGPT transport. It shares the
account's quota and rotation slot, buffers a completed upstream response when
needed, and rejects unsupported structured output, remote image URLs, explicit
fast speed, and opaque `previous_response_id`. It does not prove model quality
or bypass provider restrictions.

## Tool catalog

Relay forwards the client's tool catalog unchanged. It does not add deferred
tool search, hide or rename tools, or execute a local relevance search. Stored
legacy optimization flags do not change new requests. Usage may record catalog
size and mode as history; those fields are not an execution permission.

## Usage and prices

Usage stores the selected member, model IDs, protocol, status, timing, token and
cache counters, service tier, and a redacted provider error envelope. API-
equivalent cost is informational and never a subscription debit, customer
charge, or routing input.

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
