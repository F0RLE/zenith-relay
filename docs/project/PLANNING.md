# Zenith Relay Planning

Last reviewed: 2026-09-03.

This document describes the implementation that exists today, its boundaries,
and the design rules for compatible integrations. It is not a historical task
log; completed detail belongs in code, tests, and Git.

## Current implementation

The current implementation is a standalone local-first desktop product with
the This computer, Choose API, and My server modes; a personal account/API
pool; provider-neutral protocol adapters; quota,
model, reasoning, pricing, and usage contracts; reversible Codex profile
attachment, OpenCode configuration integration, application recovery, and a
user-managed server with an encrypted vault.

Automated checks and live acceptance are maintainer evidence, not product
behavior. Their commands and results belong to CI and the release process, not
the user-facing changelog. A passing local or mocked check does not claim that
a live provider, real account, proxy, or user-managed server is
production-ready; that requires the matching gate in [ROADMAP.md](ROADMAP.md).

The optional server path is intentionally the last acceptance priority. It is
shipped as a separate user-owned deployment contract, not as a connection to
Zenith production Gateway or Control API.

## Product boundary

Zenith Relay manages a user's own accounts and compatible API sources behind a
single OpenAI-compatible endpoint. It has three explicit modes:

| Mode | Runtime | Data ownership |
| --- | --- | --- |
| This computer | The desktop process and a loopback endpoint. | The user's computer and OS credential store. |
| Choose API | A selected compatible hosted API. | The selected source and its local secret reference. |
| My server | A user-managed Relay server. | The user's server, encrypted vault, and SQLite database. |

The desktop app is not a public account marketplace, a Zenith billing backend,
or a way to move a user's accounts into Zenith inventory. User-owned secrets
remain on the chosen device unless the user explicitly transfers them to their
own connected server through a confirmed management operation. That server is
outside Zenith production systems.

### Secret and business-logic boundary

Relay is a separate desktop/personal-pool product. It does not receive or
forward Zenith production credentials, customer API keys, backend tokens,
account-pool inventory, provider cabinet credentials, or internal Gateway and
Control API business/routing logic. Those values and decisions remain owned by
their respective production systems.

Desktop secret material is kept in the operating-system credential store.
Server secret material is kept in the user-managed server vault. A confirmed
desktop-to-server management operation may transfer only the selected user's
own provider/session secret to that user's server. It is never an implicit
upload to Zenith, and the management token is never reused as a profile
credential.

Typed snapshots, SQLite state, telemetry, logs, diagnostics, support bundles,
and screenshots contain redacted operational data only. They may include model
names, status, timing, and aggregates, but not credentials, cookies,
authorization headers, prompts, response bodies, or provider session material.
The explicit account-export document is a separate credential-bearing transfer
artifact and may contain the selected account's OAuth tokens; it is never an
ordinary diagnostic or telemetry export.

### Provider-policy boundary

Relay does not implement, document, or recommend ways to conceal subscription
account sharing or bypass an upstream provider's abuse controls. Risk signals
include multi-user access, public API keys, account rotation, unusual
parallelism or request frequency, shared proxy/IP infrastructure, and client
identity or protocol mismatches. IP rotation, TLS or client-fingerprint
spoofing, User-Agent impersonation, and artificial request shaping are not
supported evasion features. A loopback endpoint reduces exposure to the local
machine; it does not establish permission to resell or share an account.

Commercial or shared access must use a provider plan and API whose terms
explicitly allow that use. Relay must not claim a probability of detection or
policy compliance without provider-specific evidence.

## Development direction

Relay develops the supported local and user-managed server paths before adding
new provider-specific behavior. The shared contracts stay provider-neutral:
model identity, client/upstream protocol binding, reasoning capability, price,
and error origin are data, not model-name heuristics. Production claims require
the live acceptance evidence in [ROADMAP.md](ROADMAP.md); a passing fixture,
mock, or desktop build is not a substitute for a permitted real-account test.

## Component ownership

| Component | Responsibility |
| --- | --- |
| React and Vite | Render typed snapshots, collect user intent, and localize the interface. |
| Tauri host | Native lifecycle, OS credential store, OAuth return, local endpoint, and reversible profile changes. |
| relay-core | Shared canonical account/source state, eligibility, scheduler, source-adapter gateway execution, quota normalization, and usage math. |
| relay-server | Persistent personal runtime, encrypted vault, SQLite storage, migrations, retention, backup/restore, and management API. |

React never reads a secret or provider file directly. The desktop and server
both consume the shared runtime model rather than maintaining separate routing
rules.

When the primary desktop window is closed, the Tauri host destroys its WebView
instead of hiding it. The tray icon, local gateway, account state, and native
background runtime remain alive; opening Relay from the tray creates a fresh
WebView. Windows retains ownership of working-set reclamation and pagefile
placement, so Relay does not force live process pages to disk.

## Connections, quotas, and routing

### Desktop storage boundary

Relay desktop state is kept under `%LOCALAPPDATA%\\Zenith Relay`: `data` holds
the SQLite runtime database, pricing catalog, and encrypted vault, `cache` holds
WebView/import/deployment working data, and `recovery` is organized by owner:
`applications/chatgpt` contains ChatGPT profile and API-config backups,
`applications/opencode` contains OpenCode config backups, and
`operations/history-repair` contains short-lived history repair files. The database
keeps bounded, redacted request diagnostics (retained for 30 days) and an
incremental API-equivalent rollup so old logs can be removed without losing
totals. Raw secret material is never written to these records. Relay does not
create runtime databases or caches in `%USERPROFILE%\\.codex`;
that directory is touched only for the Codex files required by the reversible
client integration.

### Connections

Current account intake supports ChatGPT OAuth, an existing local profile, and
compatible imported session material. Compatible API sources are independent
records with their own address, protocol, credential references, models, priority,
recovery delay, discovered price metadata, and optional model-price overrides.
A source catalog also records the route-specific protocol binding and only the
reasoning options the source explicitly confirms. A proxy is optional and may
be shared; there is no one-proxy-per-account rule.

Discovery refreshes provider-derived catalog data for that source without
turning it into a global vendor assumption. Source pricing keeps provider,
LiteLLM exact, LiteLLM canonical, and manual provenance separate. Runtime
resolution is provider evidence first, then an exact LiteLLM provider/model
record, then a canonical record from an explicitly declared official family,
then a manual source value. Changing an endpoint or protocol causes stale
provider-derived prices to be rediscovered rather than carried to a different
source contract.

Every candidate has a stable record, credential availability, health, model
availability, and a user-facing operational state. A missing secret, revoked
session, unavailable proxy, or reauthentication requirement is recorded as
its actual cause rather than silently treated as exhausted quota.

### Monitoring

Quota monitoring eligibility is intentionally different from routing
eligibility. Enabled local accounts can be queued for a safe quota/model
refresh even when they are not in the pool, are draining, or have an unknown
quota. Refresh results update quota, subscription, model health, error state,
and UI state together. Failures retain a safe error and timestamp, use
backoff, and do not remove the account from future checks merely because a
parser or transport attempt failed.

Quota windows are provider-reported and may have arbitrary labels and reset
times. Relay displays the reported window rather than inventing a fixed
five-hour, weekly, or subscription duration. A successful refresh can restore
health and clear a model-level transient restriction.

The account view always keeps the provider-reported quota window visible. When
the user enables the account calculation, Relay shows the direct
API-equivalent of recorded token usage and, when the user enters a purchase
cost, the resulting payback ratio. If Relay has complete priced usage from the
start of the active provider window, it may also extrapolate an approximate
remaining API-equivalent for that window; activity outside Relay is excluded
and an incomplete record renders no estimate. This never turns a reported
percentage into a monetary entitlement. None of this changes routing
eligibility, provider quota, or stored request usage.

### Scheduler and execution

The scheduler evaluates factual conditions for the requested model:

1. the candidate is enabled and belongs to the managed pool;
2. it is not draining and its credential and proxy are available;
3. its account and requested model are healthy and not cooling down;
4. the account has usable quota or the source is otherwise usable;
5. the managed ChatGPT/Codex profile credential allows the requested protocol.

It preserves response ownership affinity where a protocol response requires the
same upstream account. Prompt affinity is a preference guarded by capacity and
quota, not a way to force an unhealthy account. The user-managed server can
apply a provider/model storm breaker to prevent repeated 429 waves from
spreading across candidates.

An execution may refresh one credential and retry a compatible candidate after
an authentication failure or safe transient upstream failure. It must never
retry transparently after response bytes have been sent to the client. Stream
timings and terminal errors are recorded for diagnostics. Each terminal failure
has a safe origin: `relay` for Relay configuration or translation, `account`
for account credential or account-route failures, and `provider` for a
compatible API source or upstream provider. Usage and exports retain the
origin, category, and HTTP status without retaining raw prompts, secrets,
provider response bodies, or other raw provider payloads.

## Models and client visibility

The runtime model registry is built from connection capabilities and key/model
rules. A model appears through an endpoint only when at least one enabled
candidate matching that key and client protocol can serve it. The managed
ChatGPT/Codex catalog is narrower: it contains Responses-capable candidates,
including explicitly verified `Responses -> Messages` bridge bindings, and
ChatGPT accounts marked `in_pool`. A model rule can hide a model without
deleting the source capability.

The generic OpenAI `/v1/models` view advertises Responses and Chat Completions
models only; Messages-only models are not inserted into that list. Native
Messages clients use the model IDs from their explicitly configured source and
key.

Profile attachment fetches the selected endpoint's live Codex catalog and
writes a bounded Relay-managed <code>model_catalog_json</code>. It contains
only models that the endpoint and selected key can serve; disabled,
unsupported, or unavailable pool models are not advertised. Model ids are
encoded reversibly when Codex needs a Relay route, including ids that already
contain <code>/</code>.

Relay snapshots the previous profile catalog, auth, and provider configuration
before attaching. User-managed catalog rows are used only as a validated schema
template while Relay is attached; they are not presented as pool routes. Native
ChatGPT catalog rows keep their original reasoning, service-tier, context, and
other capabilities, including their original image-input declaration or its
absence. Relay-managed routed rows advertise text and image input by default so
an incomplete provider catalog cannot make Codex reject an attachment before
the request reaches the selected model. This is a client-side admission policy,
not a claim that every upstream model understands images; the upstream remains
the final authority. API-source metadata is applied only to routed rows. A
refresh changes the managed catalog only after validation, invalidates Codex's
model cache only after the new catalog is written, and restores the previous
profile state when the managed files are still unchanged.

## Client protocol boundaries

Relay separates the client-facing wire contract from the upstream wire contract.
Every source binding records the client protocol, the adapter, the optional
bridge reasoning mode, and the models assigned to that route. `Native` is a
passthrough: the client and upstream contracts must match. The current bridge
is deliberately explicit: a client-facing <code>/v1/responses</code> binding
with `ResponsesToMessages` sends a translated Anthropic Messages request to
<code>/v1/messages</code>, using the source key as <code>x-api-key</code> and
the Anthropic version header.

The Responses-to-Messages bridge supports JSON-schema function tools, direct
custom text tools, user image input, image blocks in function output,
`tool_use`/`tool_result` continuations, ordinary JSON responses, and translated
Messages SSE including streamed tool arguments. Images are accepted only as
validated base64 data URIs for GIF, JPEG, PNG, or WebP. A custom tool is
represented upstream as a function with one raw-text `input` field, then
returns to the client as `custom_tool_call` and accepts only its direct string
`custom_tool_call_output`; the original tool host remains the validator and
executor. It stores the native assistant turn only in a bounded volatile local
continuation store keyed by bridge response id and candidate.
A missing or mismatched continuation is rejected instead of sending a
context-free tool result. Namespace tools are flattened to stable
provider-safe function aliases and restored on the Responses continuation.
Provider-hosted and dynamic-discovery tools are omitted rather than converted
into text. Budget and adaptive reasoning are opt-in binding capabilities;
`Native` bindings cannot declare a bridge reasoning mode.

A `ResponsesToGemini` binding sends a translated Responses request to Gemini's
native `generateContent` endpoint (or `streamGenerateContent` for SSE), with
the model encoded in the route and the source credential sent only as
`x-goog-api-key`. It is not a provider-name rule. The adapter covers text,
system instructions, validated images/files, JSON output schemas, function,
namespace, and direct custom tools, tool choice/allow-lists, budget/adaptive
thinking, native usage, and `previous_response_id` through the bounded local
continuation store. Namespace tools are flattened to stable provider-safe
function aliases and restored on the Responses continuation.
Gemini thought signatures and Vertex partial function arguments are preserved
through JSON and SSE tool-call turns. Provider-managed caching, hosted tools,
and WebSocket bridging remain intentionally outside this adapter. Native
Gemini usage is normalized as Responses usage without turning account
entitlement into API billing.

Relay exposes three client contracts: <code>/v1/responses</code> for
Codex/OpenAI Responses clients, <code>/v1/messages</code> for native Messages
clients, and <code>/v1/chat/completions</code> for text-and-image-only
OpenAI-compatible clients. Source discovery reads the source's model catalog
with the authentication required by each binding and applies the explicit
model assignment to that route. A <code>/models</code> response alone does not
prove that a provider accepts a completion on every protocol; when one source
has multiple routes, the binding assignment is the capability declaration and
must be verified against the provider's documentation or a safe operator test.
Cross-protocol access is explicit. A native Messages binding serves Messages
clients only; exposing the same model to Responses clients requires a saved
Responses-to-Messages binding. An explicit native Responses or Gemini route
keeps ownership of any overlapping model.
The same generic source catalog may optionally declare reasoning through
<code>capabilities.reasoning</code>, <code>reasoning</code>,
Codex-compatible fields, <code>reasoningEffortModes</code>, or explicit
<code>reasoningEfforts</code>/<code>reasoningEffortOptions</code> rows with
their values and default. Relay reads either OpenAI-style <code>data</code>
rows or a top-level <code>models</code> catalog. A bare
<code>supportsReasoningEffort</code> flag never invents levels, and an
explicit false flag suppresses stale option lists. For the verified model
contracts maintained in Relay's reasoning registry, the registry supplies the
model's exact default levels when a provider omits them; it never adds levels
outside that model's whitelist. Parsed levels remain source declarations for
models without a verified registry entry: they provide the default selector
only for the declaring route, never a global claim about another source. A Responses-to-Messages bridge removes efforts it
cannot translate and never advertises reasoning summaries. The generic source
catalog is cached separately from the provider's Codex-specific catalog, so
refreshing one endpoint cannot erase the other endpoint's declaration.
The native Messages route preserves successful JSON/SSE bodies verbatim. Chat
Completions rejects tool definitions and tool-call history instead of
pretending to translate them. Responses WebSocket remains native-only until a
separate bidirectional bridge is designed and tested.

For routed API-source rows, declared model defaults are enabled by default and
only their exact whitelist is shown. Source-declared modes are used for models
without a declared default. Model Rules exposes a manual
allow-list at the
model-group level: a saved empty list explicitly hides the API-source selector,
while a non-empty list publishes exactly the selected values, including
provider-specific values not present in the hint. This control is present for
every current pool model, including a native-account-only model with no parsed
declaration. Changes apply immediately. Relay does not send a reasoning-mode
probe: a selected value is forwarded as transport metadata and normal
pre-byte fallback remains responsible for provider rejection. A manual policy
replaces only the picker modes; native account requests still use their native
upstream contract.

## Usage and account value

Usage records safe operational metadata: request id, selected candidate, model,
request mode, success or classified error, status, token split, time to first
output, end-to-end time, and stream speed. It does not retain prompt or
response bodies in ordinary telemetry.

API-equivalent is an informational estimate, never a routing input:

- ChatGPT and other subscription-account usage uses an exact LiteLLM record
  within the account's declared official provider family, otherwise it is
  unpriced;
- an API source uses provider-discovered price evidence first;
- if discovery has no price, an exact LiteLLM provider/model record is tried;
- a canonical LiteLLM price is used only when the source explicitly declares
  the matching official provider family;
- a manual source price is used only when neither provider nor LiteLLM price
  exists;
- input, cached input, cache writes, and output retain separate price buckets;
- unknown or incomplete token splits remain explicitly unpriced;
- Fast and Standard request modes are recorded as observed service tiers, not
  multiplied by a universal factor.
- Fast is Relay's user-facing name for the upstream `priority` service tier.
  It stays separate from scheduler/source priority. Fast is a request-speed
  mode, not a second user-facing quota; provider Fast/priority metadata remains
  diagnostic and is not rendered as another account quota meter. A provider
  may report that the requested Fast tier was served as Standard.

Provider quota remains a provider-reported operational signal. It is rendered
as a percentage and reset boundary, never as money, an entitlement, a routing
input, or a billing value. Relay does not learn, calibrate, or claim a monetary
subscription quota from that percentage. It may render an approximate remaining
API-equivalent only from complete priced Relay usage recorded since the active
window began; it excludes activity outside Relay and is omitted for any
unpriced or incomplete interval. The optional account purchase cost is
user-entered presentation metadata used only to calculate payback against direct
API-equivalent usage; it never decides whether a request may use an account.

### Pricing catalog lifecycle

LiteLLM is the single reference catalog for token and request/image prices.
Relay keeps the last validated catalog in its local state for both desktop and
server deployments. A catalog refresh is an independent background operation:
the first snapshot uses the cached catalog when available, and an asynchronous
conditional validation is started once at process launch even when that cache is
still fresh. Network failure does not block startup or requests, and an offline
or expired catalog is marked stale. ETag and Last-Modified validators avoid
downloading an unchanged file; 304 responses update only freshness metadata. A
new payload is parsed and validated completely before an atomic replacement, so
malformed or oversized data cannot discard the last good snapshot.

After the startup check, normal validation is scheduled at the cache TTL
(currently 24 hours) with a small deterministic per-instance spread. A failed
attempt schedules exact bounded retries (5 minutes, 30 minutes, then 2 hours)
and wakes the scheduler when a manual refresh changes that deadline. The retry
state is in-memory because the last valid snapshot remains safe to use after a
restart.

Each usage calculation captures one immutable catalog snapshot and one policy
revision for its entire pass. Replacing the snapshot invalidates derived
API-equivalent totals without changing stored token facts. Provider evidence,
manual source overrides, account-family selection, and catalog revision all
participate in that invalidation. Prices never affect route selection, quota,
or request admission. Image/request-only records remain a separate price type
and are never converted into a zero-cost token quote. Missing prices are shown
as unpriced rather than `$0`.

## Applications and recovery

ChatGPT profile operations follow a scoped managed-state transaction:

~~~text
inspect -> preserve Relay-managed state -> repair history when the provider changes
apply or restore managed configuration -> verify -> discard or roll back history repair
~~~

Before the first managed ChatGPT change, Relay records only the configuration,
authentication, and catalog state that it will own. After confirmation,
**Restore** returns those managed fields, including the prior
<code>config.toml</code> state and managed authentication. It preserves
unrelated client settings and refuses to overwrite a newer manual sign-in.
ChatGPT does not provide named snapshots or full-profile recovery.

When a profile crosses the ChatGPT, Relay-local, or local API boundary, history
repair rewrites only the affected provider metadata for the target. The same
reversible repair runs in either direction; a failed profile operation restores
the repair backup. Windows extended path prefixes are normalized in repair
manifests and validation.

OpenCode has a single exact original configuration copy instead. Relay resolves
the user's `opencode.json`/JSONC path, copies it before the first managed write,
and restores it only after explicit confirmation. OpenCode recovery is
intentionally separate from the ChatGPT managed-state restore because the
files, lifecycle, and client ownership differ.

## User-managed server

The server is a personal single-deployment runtime:

- its management API uses a management token;
- ChatGPT/Codex receives a server-managed profile credential for <code>/v1</code>;
- encrypted secrets live in the server vault, while operational state and
  redacted usage live in SQLite;
- state, capabilities, usage, and source statistics responses stay redacted and
  never contain credentials, cookies, prompts, or raw provider bodies;
- migrations are append-only and protect interrupted upgrades with a
  pre-migration backup;
- backup and restore validate the database and encrypted references before
  activation;
- retention keeps operational usage bounded without discarding the information
  needed for current totals and diagnostics.

The desktop client negotiates protocol capabilities before it performs a
remote management action. It can manage the user's accounts, sources, proxies,
model/routing settings, usage, and profile attachment through that contract.
Moving a user-owned secret to the server is a separate explicit operation; the
contract has no path for Zenith production secrets or internal production
logic.
Server backup and restore use the standalone server CLI so they can validate
the database and encrypted vault while the data directory is locked. The
server is not yet documented as a multi-replica service: distributed leases
and cross-node prompt affinity remain future work.

## Future compatibility design

### Other subscription account systems

Relay should be able to accept other user-owned subscription systems, such as
Kiro or Antigravity, but no provider is assumed compatible just because it has
a browser login. Each proposed account connector must first prove:

1. a permitted and maintainable authentication path;
2. a way to keep credentials in the existing secret boundary;
3. normalized capabilities and model availability;
4. quota or health semantics that can be represented without false precision;
5. safe execution, refresh, revocation, deletion, and recovery behavior;
6. tests with provider-specific fixtures that contain no real secrets.

After those checks, the connector maps provider details into the existing
canonical account, capability, quota, and execution contracts. It does not add
a second scheduler or a provider-specific UI routing rule.

### Other client applications

ChatGPT/Codex profile attachment and OpenCode configuration are the current
shipped client integrations. Future client adapters are selected by user need
and only when their configuration can be inspected, changed reversibly,
verified, and restored. An adapter owns client-specific file discovery and
managed configuration; the pool endpoint, profile credential, usage, and
scheduler remain shared.

## Known limits

- Only the ChatGPT account connector is shipped today.
- The current ChatGPT/Codex profile integration uses the Responses client
  contract. A native Messages source model appears in that profile only when a
  Responses-to-Messages binding is explicitly configured. A native Messages
  client uses the original passthrough route.
- OpenCode integration manages the user's resolved `opencode.json`/JSONC path
  and writes one Relay provider entry from the prepared pool model snapshot.
  Its original file is copied before the first Relay write; OpenCode does not
  yet expose named multi-snapshot history or a native upstream WebSocket path.
- The Responses-to-Messages and Responses-to-Gemini bridges support namespace
  tools through stable aliases, but neither claims hosted or dynamic-discovery
  tools, structured custom results, native encrypted reasoning, or Responses
  WebSocket capabilities. Image support is limited to the validated data-URI
  formats described above.
- Bridge continuation state is volatile and bounded. Restarting Relay or
  evicting an entry requires the client to start a fresh turn.
- No live acceptance claim has been made for Claude, GLM, Grok, or any other
  provider until a real `codex.exe` tool-use run proves both the emitted
  function call and the follow-up tool result.
- The Computer mode stops with the desktop process.
- A self-hosted server requires real production acceptance with live accounts,
  proxy routing, streaming, and restart/recovery before it can be claimed as
  production-ready.
- No distributed multi-server lease system is implemented.

The ordered acceptance and future work are in [ROADMAP.md](ROADMAP.md).
