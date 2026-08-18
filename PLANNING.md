# Zenith Relay Planning

Last reviewed: 2026-08-11.

This document describes the implementation that exists today, its boundaries,
and the design rules for compatible integrations. It is not a historical task
log; completed detail belongs in code, tests, and Git.

## Product boundary

Zenith Relay manages a user's own accounts and compatible API sources behind a
single OpenAI-compatible endpoint. It has three explicit modes:

| Mode | Runtime | Data ownership |
| --- | --- | --- |
| This computer | The desktop process and a loopback endpoint. | The user's computer and OS credential store. |
| Choose API | A selected compatible hosted API. | The selected source and its local secret reference. |
| My server | A user-managed Relay server. | The user's server, encrypted vault, and SQLite database. |

The desktop app is not a public account marketplace, a Zenith billing backend,
or a way to move a user's accounts into Zenith inventory. Secrets remain on the
chosen device unless the user explicitly transfers them to their own connected
server through a management operation.

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

### Connections

Current account intake supports ChatGPT OAuth, an existing local profile, and
compatible imported session material. Compatible API sources are independent
records with their own address, protocol, credentials, models, priority,
recovery delay, discovered price metadata, and optional model-price overrides.
A source catalog also records the route-specific protocol binding and only the
reasoning options the source explicitly confirms. A proxy is optional and may
be shared; there is no one-proxy-per-account rule.

Discovery refreshes provider-derived catalog data for that source without
turning it into a global vendor assumption. Source pricing keeps provider,
official-catalog, and manual provenance separate; runtime resolution is
provider-discovered first, then the verified official catalog, then a manual
value when no trusted upstream or official price exists. Changing an endpoint
or protocol causes stale provider-derived prices to be rediscovered rather than
carried to a different source contract.

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
cost, the resulting payback ratio. It never turns a reported available
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
origin, category, and HTTP status without retaining raw prompts, secrets, or
provider response bodies.

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
context-free tool result. Provider-hosted, namespace, and dynamic-discovery
tools are rejected rather than converted into text. Budget and adaptive
reasoning are opt-in binding capabilities; `Native` bindings cannot declare a
bridge reasoning mode.

A `ResponsesToGemini` binding sends the supported text subset to Gemini's
native `generateContent` endpoint (or `streamGenerateContent` for SSE), with
the model encoded in the route and the source credential sent only as
`x-goog-api-key`. It is not a provider-name rule. The binding rejects tools,
images, reasoning, and `previous_response_id` until each conversion has its
own confirmed capability and regression coverage; it returns native Gemini
usage as normalized Responses usage without turning account entitlement into
API billing.

Relay exposes three client contracts: <code>/v1/responses</code> for
Codex/OpenAI Responses clients, <code>/v1/messages</code> for native Messages
clients, and <code>/v1/chat/completions</code> for text-and-image-only
OpenAI-compatible clients. Source discovery reads the source's model catalog
with the authentication required by each binding and applies the explicit
model assignment to that route. A <code>/models</code> response alone does not
prove that a provider accepts a completion on every protocol; when one source
has multiple routes, the binding assignment is the capability declaration and
must be verified against the provider's documentation or a safe operator test.
The same generic source catalog may optionally declare reasoning through
<code>capabilities.reasoning</code>, <code>reasoning</code>,
Codex-compatible fields, <code>reasoningEffortModes</code>, or explicit
<code>reasoningEfforts</code>/<code>reasoningEffortOptions</code> rows with
their values and default. Relay reads either OpenAI-style <code>data</code>
rows or a top-level <code>models</code> catalog. A bare
<code>supportsReasoningEffort</code> flag never invents levels, and an
explicit false flag suppresses stale option lists. Relay does not infer
reasoning from a provider or model name. It advertises the union of efforts
confirmed by eligible Responses routes; when the client explicitly chooses an
effort, routing excludes API-source candidates that did not confirm that
effort. If no fresh capability snapshot exists, normal transparent fallback
continues instead of excluding every candidate. A Responses-to-Messages bridge
further removes efforts it cannot translate and never advertises reasoning
summaries. The generic source catalog is cached separately from the provider's
Codex-specific catalog, so refreshing one endpoint cannot erase the other
endpoint's confirmed capabilities.
The native Messages route preserves successful JSON/SSE bodies verbatim. Chat
Completions rejects tool definitions and tool-call history instead of
pretending to translate them. Responses WebSocket remains native-only until a
separate bidirectional bridge is designed and tested.

For routed API-source rows, Relay does not infer or blacklist reasoning effort
names from a provider or model name. Model Rules may narrow a model to an
operator-selected allow-list of confirmed levels: an empty allow-list exposes
all detected levels, and one allowed level becomes the sole catalog choice.
This changes catalog availability only; Relay does not overwrite the effort
chosen by Codex or another Responses client. In automatic mode, `medium` is
the catalog default when that confirmed level exists; otherwise Relay advertises
no default rather than inheriting a provider-specific value such as `ultra`.
Native Responses routes preserve the current per-model source
declaration, including effort names Relay did not know in advance; when a
route does not explicitly confirm reasoning metadata, Relay does not advertise
a selector for that route. Claude's separate compatibility fallback remains
limited to the previously documented sparse Claude metadata case. Native
ChatGPT catalog rows remain authoritative and are not rewritten by this policy.

## Usage and account value

Usage records safe operational metadata: request id, selected candidate, model,
request mode, success or classified error, status, token split, time to first
output, end-to-end time, and stream speed. It does not retain prompt or
response bodies in ordinary telemetry.

API-equivalent is an informational estimate, never a routing input:

- personal account usage uses the verified bundled OpenAI price catalog only;
- an API source uses provider-discovered price evidence first;
- if discovery has no price, the verified bundled OpenAI catalog is tried;
- a manual source price is used only when neither provider nor official price exists;
- input, cached input, cache writes, and output retain separate price buckets;
- unknown or incomplete token splits remain explicitly unpriced;
- Fast and Standard request modes are recorded as observed service tiers, not
  multiplied by a universal hard-coded factor.

Provider quota remains a provider-reported operational signal. Relay does not
learn, calibrate, or extrapolate a monetary subscription quota from available
percentages. The optional account purchase cost is user-entered presentation
metadata used only to calculate payback against direct API-equivalent usage;
it never decides whether a request may use an account.

## Profiles and recovery

Profile operations follow one reversible transaction:

~~~text
inspect -> create or reuse snapshot -> apply managed configuration -> verify
restore -> verify restored state
~~~

Recovery lists snapshots, opens their real location when appropriate, restores
a selected snapshot, and removes only Relay-managed configuration if no
snapshot is available. During Relay-managed automatic detach/restore, a user
or Codex change to the global <code>model_reasoning_effort</code> is preserved.
An explicitly selected full snapshot restore may restore the snapshot as a
whole and is not covered by that preservation guarantee. Changes to the
managed provider, base URL, credentials/auth, or model catalog still block
managed recovery rather than being overwritten.

## User-managed server

The server is a personal single-deployment runtime:

- its management API uses a management token;
- ChatGPT/Codex receives a server-managed profile credential for <code>/v1</code>;
- encrypted secrets live in the server vault, while operational state and
  redacted usage live in SQLite;
- migrations are append-only and protect interrupted upgrades with a
  pre-migration backup;
- backup and restore validate the database and encrypted references before
  activation;
- retention keeps operational usage bounded without discarding the information
  needed for current totals and diagnostics.

The desktop client negotiates protocol capabilities before it performs a
remote management action. It can manage accounts, sources, proxies,
model/routing settings, usage, and profile attachment through that contract.
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

Codex profile attachment is the current supported client integration. Future
client adapters are selected by user need and only when their configuration can
be inspected, changed reversibly, verified, and restored. An adapter owns
client-specific file discovery and managed configuration; the pool endpoint,
profile credential, usage, and scheduler remain shared.

## Known limits

- Only the ChatGPT account connector is shipped today.
- The current ChatGPT/Codex profile integration uses the Responses client
  contract. Native Messages sources require a separate compatible client
  integration and are not exposed through the managed profile; a source must
  opt into the explicit Responses-to-Messages bridge to make a Messages model
  visible to Codex.
- The bridge does not claim hosted, namespace, dynamic-discovery, structured
  custom result, native encrypted reasoning, or Responses WebSocket
  capabilities. Image support is limited to the validated data-URI formats
  described above.
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
