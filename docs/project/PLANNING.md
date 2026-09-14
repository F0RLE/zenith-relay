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
call providers, or implement routing. Closing the main window destroys its
WebView while tray/background runtime survives; exiting the process stops the
local pool. Distributed multi-server coordination is not implemented.

## Storage and credentials

Desktop state uses the platform local-data location: normally
`%LOCALAPPDATA%\Zenith Relay` on Windows,
`~/Library/Application Support/Zenith Relay` on macOS, and
`$XDG_DATA_HOME/Zenith Relay` (normally `~/.local/share/Zenith Relay`) on Linux.
Within it, `data/database`, `data/vault`, `data/catalogs`, and
`data/migrations` keep durable Relay state separate; `cache` holds temporary
imports, OAuth state, locks, and the WebView profile; `exports` holds generated
deployment bundles; and `recovery` holds application-specific backups and
history-repair operations. The credential-store implementation owns secret
access. Codex's directory is touched only for reversible integration.

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

Descriptive metadata merges models.dev identity/capabilities, OpenRouter
reasoning enums, and LiteLLM fallback flags. Native ChatGPT catalog rows retain
their native client capabilities, including levels absent in public metadata.
Exact/provider-qualified/unique-leaf matching does not collapse ambiguous IDs.
Unknown IDs use text/image input and text output without invented reasoning,
tools, or limits. This input fallback is client admission, not proof of upstream
image support. Model Rules may narrow known reasoning levels, never invent them.

Desktop/server load the validated local metadata cache, then independently
refresh sources roughly hourly with HTTP validators. Failed sources become
stale without discarding healthy sources or blocking requests. Replacement is
schema-checked and atomic. Backend company grouping and release/update ordering
preserve explicit manual order; React does not re-rank models. Model Rules are
operational; source prices remain in the source editor.

## Quota and execution

Monitoring can refresh enabled accounts outside the pool, while draining, or
with unknown quota. Provider windows/reset times are displayed as reported.
Backoff retains the actual safe failure reason; successful refresh can restore
health. Credential, proxy, auth, and capacity failures are not all quota errors.

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

Failures preserve `relay`, `account`, or `provider` origin plus safe category,
status, and timings across protocols, storage, UI, and exports. Raw payloads
and credentials never enter diagnostic records.

## Protocol adapters

Bindings declare client protocol, upstream adapter, assigned models, and any
bridge reasoning mode. Model listing alone does not prove endpoint support.
`Native` is passthrough and requires matching wire contracts.

- `/v1/responses`: native Responses or an explicit Messages/Gemini bridge.
- `/v1/messages`: native Messages JSON/SSE passthrough.
- `/v1/chat/completions`: text/image input; tools and tool-call history are
  rejected rather than silently approximated.
- Responses WebSocket is native-only; no bidirectional bridge is claimed.

Messages/Gemini bridges translate supported function, namespace, and direct
custom tools, validated image input, reasoning, usage, and JSON/SSE. Tool hosts
remain responsible for execution/validation. Namespace aliases are reversible;
tool continuations use bounded volatile state keyed to response and candidate.
Missing/mismatched continuation fails rather than dropping context. Restart or
eviction requires a fresh turn. Hosted/dynamic tools, structured custom results,
native encrypted reasoning, provider-managed caching, and WebSocket bridging
require separate implementations and acceptance.

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
Optional purchase cost/payback and remaining API-equivalent estimates are
display-only and require complete priced Relay usage for the relevant window.

## Profiles and user-managed server

Profile changes follow inspect, protected snapshot, attach/apply, verify, and
restore. Only Relay-owned config/auth/catalog fields are managed; newer manual
sign-ins and unrelated settings survive. Cross-provider history repair is
reversible and rolls back if profile application fails. Named ChatGPT recovery
points are separate explicit restores of configuration/authentication.

Codex attachment writes a validated bounded catalog from the selected live
endpoint/key with reversible model IDs. Native rows keep native capabilities.
Refresh writes before invalidating the client cache; failure retains the prior
verified profile and exposes a warning. OpenCode preserves the original JSON/
JSONC configuration and restores with a semantic merge of compatible user edits.
Relay-owned ChatGPT account calls start with a validated stable Codex fallback,
then asynchronously read the official OpenAI Codex GitHub release feed after
the host loads and once per hour. Only published `rust-v<semver>` releases
without a prerelease component are accepted; the selected version remains
process-local and is never written to a JSON cache. Direct client identity
headers remain client-owned.

Remote management negotiates capabilities and uses revision-checked operations.
Secret transfer is separate from profile/config publication. Backup/restore
uses the server CLI with a locked data directory. Real account, proxy, client
tool-use, streaming, restart, and restore acceptance remains necessary before
production-ready claims; mocks or a successful build do not establish it.
