# Zenith Relay architecture

Current implementation contracts. Code and focused tests define exact behavior.
Use [ROADMAP.md](ROADMAP.md) for open work, [CONTRIBUTING.md](../../CONTRIBUTING.md)
for development checks, and [Help](../help/en/README.md) for user steps.

## Contract map

This is the current architecture reference, not a delivery plan. The historical
filename remains stable for existing links. Use the owning section below rather
than treating release notes or planned Hosted contracts as runtime behavior.

| Question | Current contract |
| --- | --- |
| Which component owns the behavior? | [Scope and ownership](#scope-and-ownership), [core layout](#core-layout) |
| What runs locally, and how does the UI observe it? | [Desktop workspace](#desktop-workspace), [statistics and refresh](#statistics-and-refresh) |
| Where are credentials and private data kept? | [State and secret boundaries](#state-and-secret-boundaries) |
| How do sign-in, imports and model discovery work? | [Connections](#connections-discovery-and-metadata) |
| Which route may execute a request? | [Pool and execution](#pool-and-execution) |
| Who owns a continuation or failed request? | [Continuations, errors and redaction](#continuations-errors-and-redaction) |
| What survives protocol conversion? | [Protocol adapters](#protocol-adapters), [tool catalog](#tool-catalog), [BPS](BASIS_POINTS.md) |
| How does a proxy affect request context? | [Request timezone](#request-timezone) |
| What do usage, cache and cost values mean? | [Usage and prices](#usage-and-prices) |
| What does connecting or restoring a client change? | [Application profiles](#application-profiles) |
| What remains outside the current product? | [Implementation boundaries](#not-provided-by-the-current-implementation), [open Desktop work](ROADMAP.md) |

## Scope and ownership

Relay is a local-first desktop application, independent of production Zenith
Gateway and Control. Production credentials, customer data, inventory, billing
and routing stay in those systems. [Hosted Relay](../hosted/HOSTED_RELAY_ARCHITECTURE.md)
describes the planned service.

| Area | Owns |
| --- | --- |
| `src/src` | React UI, i18n, snapshots, typed Tauri calls |
| `src-tauri/src` | Desktop I/O, credentials, OAuth, profiles, process lifecycle |
| `crates/relay-core` | Shared discovery, routing, protocols, quota, scheduling, usage |
| `web` | Independent Hosted UI preview; backend integration is planned |
| `catalog` | Versioned model reference data and export contract |
| `scripts` | Build, setup, verification, release and maintenance commands; shared helpers in `lib` |
| `docs/project`, `docs/hosted`, `docs/help`, `docs/releases` | Current contracts and Desktop roadmap, future Hosted plans, localized user steps, release history; navigation in `docs/README.md` |

Desktop Rust owns secrets, provider calls and routing, using `relay-core`
in-process. Closing the desktop window leaves the local process
running; explicitly quitting stops it. LAN access shares that same local runtime.

### File organization

Rust uses `module.rs` with child files in `module/`; for example,
`src-tauri/src/codex_config.rs` owns `codex_config/auth.rs`,
`codex_config/provider.rs`, and `codex_config/tests.rs`. Keep module declarations,
imports, and tests aligned when moving a file. Compatibility readers and recovery
backups remain with their owners for supported saved data.

Desktop runtime assembly, live-policy synchronization and rollback live in
`src-tauri/src/local_pool/runtime.rs` and `runtime/`, shared by commands,
refresh readers and account operations. Gateway-key persistence lives in
`store/gateway_keys.rs`, with secret cleanup in `store/secret_store.rs`.
Call these owners directly outside `commands/`; runtime tests stay beside
their implementation rather than under the IPC command tree.

Client integrations live together under `local_pool/profiles/`: `codex/`,
`opencode/`, and `claude_desktop/` own their configuration, recovery and tests;
`profiles/catalog.rs` owns Codex catalog refresh and the shared refresh result.
Runtime calls the profile refresh coordinator directly. Desktop command
registration references the owning integration modules without changing IPC names.

Desktop UI lives in `src/src/features/relay`: `pages/` owns page-specific views and
models, `state/` owns shared state, `api/` owns typed IPC, and `shell/` owns
navigation. Cross-page domain helpers stay at the feature root. `src/tests/`
contains unit and browser tests; `web/src/` and `web/tests/` belong only to the
independent preview. `scripts/{build,setup,check,release,maintenance}/` groups
commands by purpose; helpers used only by one group stay in that group.

Browser tests keep the public mock helpers in `src/tests/e2e/tauri-mock.ts`,
scenario option domains in `tauri-mock/options.ts`, and the self-contained
WebView initializer in `tauri-mock/runtime.ts`. The initializer must remain
serializable without module closures; its imports are type-only.

## Core layout

Paths in this table are under [relay-core/src](../../crates/relay-core/src).

| Module | Responsibility |
| --- | --- |
| `gateway/` | HTTP/WebSocket ingress, body limits, request execution, streaming, error responses |
| `runtime/` | Configured sources and keys, authorization, candidate leases, model projection, session state |
| `scheduler/` | Capacity, selection, affinity, cooldowns, refresh and rotation policy |
| `protocol/` | Wire contracts, native forwarding, typed codecs, terminal validation, replay |
| `accounts/`, `providers/` | Token authority, account-route group policies and provider-specific account transport, including ChatGPT/BPS |
| `sources/` | API-source endpoint configuration, discovery, service profiles, statistics |
| `model_metadata/`, `catalog/` | Reference identity, capabilities, native-protocol and display policies |
| `quota/` | Provider quota observations and their freshness |
| `usage/`, `pricing/` | Request/attempt telemetry and API-equivalent estimates |
| `automations/`, `request_timezone.rs`, `tool_policy/` | Shared explicit background and request policies |

Desktop IPC DTOs, including `DesktopCapabilities`, live in `protocol/management/`.
They are separate from model and provider capabilities in `model_metadata/` and `sources/`.

Usage query normalization lives beside its DTO and takes an explicit observation
time. Desktop supplies protected route-identity facts to core policies and owns
credential access, dispatch fences, atomic persistence and rollback; automation
route remapping lives in `automations/`.

A pooled request follows this lifecycle:

1. Decode the client's contract; authenticate its request key and scope.
2. Resolve the real model, continuation owner, eligible routes, and admission.
3. Lease one physical member; prepare credentials, policy, and wire request.
4. Execute through the selected transport within the logical request's budget.
5. Validate the terminal outcome and retain known usage, including failures.
6. Release capacity; publish usage and only valid reusable continuation state.

The owning execution paths are `gateway/execution/request/`,
`gateway/execution/account/`, and `gateway/websocket/`. They share runtime,
protocol, authorization, and scheduler policy; transport-specific orchestration
does not grant a different retry or ownership contract.

`request/state.rs` owns request-local retry history and continuation/repair
state; candidate attempts borrow that owner rather than assembling independent
mutable flags. Identity, budget, leases and recorded outcomes keep their owners.
`runtime/models.rs` owns model registry and editable model policy while retaining
separate locks and the shared visibility handle observed by candidate leases.

## Desktop workspace

The desktop entrypoint is `src-tauri/src/desktop.rs`. Explicit source launches
retain read-only access to saved legacy keys. Profile backups and history
recovery belong to the pool integration.

Data pages use the workspace beside the sidebar. Card tracks remain compact;
settings, dialogs, and reading content have bounded readable widths. User-facing
labels and workflows belong in localized Help, not this architecture document.

Live activity keeps each candidate's newest event within its runtime generation.
Events from different candidates may arrive out of order; a late release still
updates that candidate without moving the latest global activity marker backwards.

The mode picker offers Computer and Relay Hosted. Hosted is a localized
development placeholder: no runtime snapshots, server connections, or inference
are started there. Switching modes does not stop the local API.
Development builds additionally expose an explicit Hosted demo action. It checks
the fixed loopback web preview and opens a separate incognito `hosted-preview`
window with no Desktop capabilities, no external navigation and no account/runtime
access. Release builds hide the action and reject the native command. This is a
UI-only demo, not Hosted activation or a server connection.
The planned replacement is the [limited Desktop Hosted console](../hosted/HOSTED_RELAY_ARCHITECTURE.md#desktop-hosted-scope),
not full web-admin parity. Its three-area scope and permissions remain future work
in the Hosted roadmap; the placeholder does not implement them.
The retired personal-server desktop mode migrates to Computer. Its transport,
IPC commands, background reconciliation, transfer state machine and UI are removed.
Saved server metadata and incomplete operations are archived without deleting
credentials. Affected local accounts remain disabled and outside the pool.
Explicit local recovery requires confirmation that the external owner is stopped;
it releases the saved ownership tag but does not enable the account or contact the
old server. No retired server runtime is shipped; Hosted is a separate future service.

Computer API binds loopback by default. Explicit LAN sharing binds IPv4 and
admits only private IPv4 peers with a literal private Host and no browser Origin.
A separate credential-store request key gates network inference; local profiles
keep their loopback URL and key. Both keys use the same runtime and scheduler.
Scope changes persist and restart a running listener with rollback on failure;
they never start a stopped API. No management API, firewall rule, port forwarding,
or public-internet exposure is provisioned. LAN HTTP requires a trusted network.
Automatic startup shares the settings transaction lock with stop and configuration
changes, then reads the committed enabled state, port and bind scope.
Stopping or pruning a finished listener withdraws its published runtime before
draining connections. Idle route receivers retain no runtime/storage state;
already-dispatched work keeps its own state while finishing or being cancelled.
The connection panel lists private IPv4 addresses by operational interface rather
than guessing from the default route (which may use a VPN). While visible, it
refreshes on focus, reconnection and every 15 seconds without changing the listener.

Startup paints a themed, localized HTML loader before importing the React UI.
It reports interface/data loading, not invented progress percentages. For an
existing setup, the shell is revealed after its first paint without waiting for
the runtime snapshot. Navigation, Settings and Help remain usable while data
loads. Pages restore their last successful data, without skeletons, dimming or
loading copy, and refresh quietly. With no previous data, unknown content stays
absent until a successful read or a retryable failure, never a false empty
inventory. First-run setup retains the HTML loading/reload fallback.
Bundle failures can reload without clearing settings, and render failures reveal
the existing error boundary. Secondary page code loads on navigation intent
(hover, focus or click), not automatically after runtime readiness. Unvisited
pages do not mount, fetch reports or restore their saved Usage previews.
Navigation keeps the actual current screen until the target JavaScript and CSS
are ready, then renders the prepared component directly without a first-use
Suspense frame. The latest click/mode wins; failed imports retain the current
screen with a retry action. The interactive metric still waits for a successful initial snapshot
and paint; revealing navigation does not record backend readiness or health.
Opening folders, exporting Usage/configuration, previewing a preset and copying
an API key do not reload the full runtime. Applying a preset, rotating a key and
other runtime mutations retain their refresh. Settings reuse immutable native
storage roots only within the current renderer; mutable diagnostic settings still
revalidate. Network observation does not restart for unrelated busy/feedback
changes, and an older network read cannot undo a completed sharing change.
### Diagnostics and native work

Memory monitoring is log-only and debug-gated. Startup, enabling debug and opening
Logs/Operations collect a native snapshot on a blocking worker; there is no polling
or renderer heap/DOM walk. `logs/operations/operations-*.jsonl` records only Relay
and descendant WebView process measurements, build type and fixed-name native
cache accounting, correlated by `sampled_at_ms`. It excludes command lines and
cached payloads and does not replace operation breadcrumbs.

OS memory, serialized bytes, allocation estimates and configured SQLite budgets
are separate measurements, not additive process totals or a complete heap profile.
Immutable payload accounting can be reused without retaining old catalogs;
mutable caches and OS memory are sampled fresh. Missing or busy measurements
remain unavailable, not zero.

Diagnostic JSONL appends synchronously, resumes the newest daily part and retains
1 MiB parts with 14-part error/operation retention. Interrupted-stage recovery
alternates two sequenced files: a torn newer slot falls back to the previous
complete slot after restart. Legacy stage files remain readable; clean exit clears
stage state. Writes validate their directory boundary. History uses SQLite WAL/FULL
durability independently of diagnostic files.

Native snapshots, renderer timing writes and blocking configuration, filesystem
and network-interface reads run off the window thread. Results include completed
validation and storage errors. Snapshot timing separates native preparation,
queue/worker waits, IPC, paint and interactive readiness; preparations of at least
250 ms also record fixed-name stages. A timing batch commits atomically, including
retention, and contains no account, source, model or credential values.

Each Desktop state admits one full runtime snapshot before reading its inputs;
clones share that bound separately from the setup mutation lock. Cancellation
retains the slot until blocking work finishes. Routing previews use the current
inventory/catalog, reuse admission only within that snapshot and never advance
live rotation; ambiguous ranks stop the preview. Presentation indexes and saved
model ordering provide no route, quota or authorization evidence.

### Metadata and query caches

Metadata caches persist validated source envelopes rather than the merged JSON
tree. Immutable catalogs, pricing and indexes can be shared across unchanged
revisions; refresh failure retains prior data and updates status. Freshness changes
recompute metadata winners even after HTTP 304. Within the exact provider/model
scope, fresh sources win and stale sources only fill missing fields.

Provider-specific models.dev, OpenRouter and LiteLLM records retain their hosting
namespace, including qualified IDs. Canonical presentation links do not import
another host's limits or enums. Provenance/conflicts retain source revision,
provider and scoped record ID, including legacy compatibility. Invalid limits
remain unknown. Independent reasoning evidence is preserved before merging;
price aliases and conflicts retain their established resolution rules.

The minute maintenance worker releases expired tool presentations and validators
without changing TTLs or limits; busy locks are skipped and request-owned handles
remain valid. Quota-equivalent caches are account/window scoped. Corrections
invalidate both old and new owners; price/window changes, retention, clearing and
purges invalidate reuse. Revision checks fence late reads. History clearing deletes
logs, attempts, rollups and weekly-capacity state in one transaction, then
invalidates caches only after commit.

### Usage loading and display recovery

Usage loads the selected sends page (50 by default) and period summary on demand.
Model/member tabs request their own aggregates; filter choices cover the whole
retained period and load when a filter opens. Neighboring pages and diagnostic
payloads are not prefetched. The renderer keeps the last selected live report.
One loader serializes native reads and collapses queued revisions/filter changes
to the latest active intent. Leaving cancels queued work; late results cannot
satisfy a newer revision or reopen a dismissed detail dialog.

History, retention and pricing events coalesce into a refresh of the visible query;
general runtime events invalidate cached pages for the next visit. Rolling periods
retain their freshness bound. Same-query refreshes retain the completed report,
including on failure; a new query shows absent content or a read error until its
own result arrives. Unknown counters never become zero or a false empty report.
Request-ID search commits after a 250 ms pause; clearing or leaving cancels it.

Native pagination reuses one exact-scope/history/pricing summary with a 2 MiB
retained-data budget, excluding diagnostics, events and filter inventories.
Oversized results return uncached. Writes, corrections, retention and clearing
invalidate summaries and revision/period-scoped filter choices. Sends-only counts
are lower bounds; the optional IPC `includeReport` flag defaults to full reports.
Cache-session queries select the exact chat, candidate and model before limits
and window functions, preserving earlier touches and TTLs.

Overview and Usage support all consumption, conversations, memory and other
background-work scopes. Explicit retained task markers establish scope, including
blocked memory operations; model names and prompts do not. Unmarked/unrecognized
records remain conversations. The scope applies to sends, outcomes, totals and
time buckets, participates in cache keys and is echoed by the desktop host.

IndexedDB retains one redacted local inventory, eight exact-query Usage reports
and one redacted proxy summary. Only the selected Usage preview is restored;
other pages remain on disk. Overview separately retains eight scoped previews.
Writes coalesce and flush on hiding/leaving; storage failure is non-fatal and
independent of native durability. Credentials, revealed identities and provider
error messages are excluded; activity and leases are not restored.

Saved previews are display-only. Runtime-dependent actions stay disabled until a
native snapshot succeeds; navigation, Settings and Help remain usable. Hosted
does not restore local inventory. Accepted reads/mutations fence late restoration;
history clearing invalidates saved Usage/Overview data and older in-flight reads.
Failed refreshes retain saved data and offer retry.

Pool and Connections share one routing observer across navigation, retaining its
pending read and fallback deadline. Activity changes during a read queue one
revalidation; a five-second fallback follows each attempt, including failure.
Leaving both pages stops observation. Hidden windows pause polling/countdowns and
keep only the latest per-candidate activity, including zero-count tombstones.
Visibility restores one ordered update and reconciles deadlines with wall time;
queued background reads recheck visibility before starting.

### Localization

Desktop uses i18next with English as the complete key/fallback source and an
explicit BCP-47 registry at `src/src/i18n/registry.json`. Dictionaries are lazy;
only a selection's declared fallback chain is required. Script/region identities
and explicit aliases remain distinct. A late load cannot override a newer choice;
failed selection preserves the current locale, while startup can recover in English.
Preference-storage failure is non-fatal. The i18next store retains at most four
language bundles unless the active fallback chain itself needs more; ES-module
caching remains browser-owned.

Document `lang`/`dir`, setup and Settings use the same registry. Help has an
independent registered fallback; untranslated guides do not prevent UI languages.
Translated startup/native text is a bounded public-copy projection of the active
dictionary, not another translation catalog. Native labels persist locally for
tray-only restart and refresh on selection; they cannot alter account or routing
policy. Runtime errors/diagnostics remain English, localized UI uses stable keys.
The [contributor workflow](../../CONTRIBUTING.md#desktop-localization) owns adding
languages, translation checks and separate startup/resource budgets.

## State and secret boundaries

- Credential access belongs to the Desktop credential store and protected vault.
  Database, recovery, export, cache, and log areas are separate.
- Desktop's Relay-owned agent-carrier key persists as a typed protected-vault
  record, not as account credentials or message history. Startup restores it
  before runtime admission; corruption fails closed. The exact lifecycle and
  limit for previously lost volatile keys stay in [BPS](BASIS_POINTS.md).
- Snapshots, usage, and diagnostics are redacted. Account exports are explicit
  credential-bearing documents, not support bundles.
- Provider credentials and inference request keys are different credentials.
- Local database migrations are append-only. Recovery validates protected
  references before activation.
- Runtime replacement fences delayed dispatches. Already dispatched streams can
  finish; a save acknowledges durable storage and runtime publication, not a
  later client-catalog refresh. Late UI reads cannot undo a successful command.
- Publishing the same runtime instance is a no-op and never retires it. Policy
  reload compares source snapshots by unique ID, independently of ordering;
  duplicate IDs or changed transport identities require the normal rebuild path.

## Connections, discovery, and metadata

### Connection checks and sign-in

Explicit source connection checks use saved endpoints, authentication and proxy
settings without generation, catalog mutation or capability promotion. HTTP
headers and Responses WebSocket upgrades have separate redacted observations;
WebSocket uses its own HTTP/1.1 client rather than an HTTP/2 catalog connection.
Elapsed values include management admission and request/upgrade time, not model
inference. Timeouts retain their known classification; HTTP throttling suppresses
the extra WebSocket check. No redirects or direct proxy fallback are allowed.

Connections stores accounts, API sources, and proxies. Saving a connection does
not enable pool membership. Access owns listener/key settings and account
request policies; Pool owns membership and per-model rules.

`DesktopCapabilities.accountConnectors` publishes stable connector IDs, provider
families and sign-in flow kinds for future connection and pool UI. Only the
existing OpenAI Codex, SIWC and Excel/BPS methods are advertised. Reserved
provider-family identifiers do not add a login, credential authority, entitlement
or pool route. Other account connectors and their UI remain deferred.

Connections offer Codex (`codex`, the default), ChatGPT/SIWC (`siwc`), and
Excel/Basis Points (`excel_bps`) in the Tauri sign-in window. Each supports new sign-in,
resume and reauthentication with its original issuing client; no connection is
deleted, relabeled or silently converted to another login method.
Each embedded sign-in opening uses a fresh incognito WebView. On Windows an
exclusively created per-opening cache directory isolates WebView2 from other
private windows; cleanup waits for WebView2 processes using that directory,
then applies bounded file-lock retries off the UI thread. Process inspection is
limited to this cleanup, and no command-line data enters diagnostics. Leftovers
after an interrupted cleanup are never reopened.
Linux/macOS use the engine's new ephemeral context/data store. Codex, SIWC and
BPS never reuse browser cookies or website storage from an earlier opening, including resume
and reauthentication. OAuth state, saved credentials, account Notes and Relay
settings retain their existing authorities; external-browser sessions are not
cleared. Existing on-disk sign-in caches are not opened or deleted by login.
SIWC uses the existing effective proxy.
Its exact redirect is captured by the same native callback validator as manual
submission, without routing the loopback callback through the proxy. State,
PKCE, nonce, issuing-client checks and cancellation fences remain unchanged;
external pages receive no Tauri IPC permissions or saved-login automation.
This embedded SIWC flow is a deliberate Desktop deviation from the official
system-browser guidance linked below, not a provider compatibility guarantee.
The copy-link action retains manual system-browser recovery; live WebView
acceptance is tracked in ROADMAP.
Both new sign-in and reauthentication expose optional account notes. New notes
stay in memory under the login ID, survive folding the section, and use the
existing notes command only after account completion. Cancellation clears the
draft. Reauthentication edits the targeted account's existing notes. Normal
sign-in remains manual. On Windows, macOS and Linux, right-clicking an
Excel/BPS account's sign-in-required action immediately starts an automatic
attempt without a menu, using its saved
email/password and, on an identified authenticator challenge, a fresh TOTP.
The fresh private WebView, effective proxy, OAuth callback validation
and target-identity checkpoint remain authoritative. Notes stay native until an
exact HTTPS auth.openai.com login form needs one value; no external-page IPC is
enabled. Each evaluation carries a deadline checked in the page before filling
and submitting, so a delayed evaluation cannot submit after its native timeout.
An empty hydrating form gets a bounded five-second grace per page/transition;
CAPTCHA and explicit errors still stop immediately. Each stage is submitted at
most once. CAPTCHA, errors, consent, social
login, unknown forms or a 45-second limit stop automation and reveal the same
window. Manual resume stops automation without replacing that window; cancel,
replacement and window destruction fence the task. No unattended completion is
guaranteed and no new provider authentication API is introduced.
Native manual-fallback notifications carry only login ID and redacted automation
state through the existing OAuth event listener. They cannot rewind callback or
terminal status; early events survive start completion. Manual continuation also
clears its progress hint locally after a successful resume.
Persisted loopback attempts retain the issuing client's exact redirect host:
`localhost` for Codex and `127.0.0.1` for SIWC, including after restart.
Early status events are matched by login ID, so another attempt cannot hide a
callback received before start returns. Targeted reauthentication uses the
account's effective proxy or direct override for token exchange; the common
proxy does not replace that route.
OAuth progress uses the same status event with closed callback, token-exchange,
identity-validation and saving stages, per-stage elapsed milliseconds and safe
failure codes. Progress cannot restart completion or rewind terminal state; the
first failure remains visible. Diagnostics exclude credentials and raw replies.
Account JSON imports reject duplicate object fields at every depth, including
escape-equivalent keys, before materializing values. JSONL keeps malformed rows
separate from valid accounts; access-only tokens and existing formats remain valid.
Transient provider token-refresh failures persist a credential-scoped retry budget
under the existing process lock: delays grow from five seconds to five minutes.
Valid access tokens remain usable; successful rotation or a new login clears the
budget. Terminal grant failures keep their reauthentication semantics. SIWC's
uncertain-rotation checkpoint remains authoritative, not an automatic retry grant.
SIWC persists a protected registration (issued client ID, verified subject,
host ID, granted scopes, expiry and rotating tokens) using the existing vault.
The initial dynamic client ID is never used for exchange or refresh. Callbacks
use PKCE/state/nonce and 127.0.0.1; RS256/JWKS verification checks issuer,
audience, expiry and nonce. Reauthorization stays with the selected registration.
Returning sign-in may use its verified email as an attempt-bound `login_hint`;
retained ID tokens stay out of browser URLs. Public signing keys use a bounded
in-memory cache with serialized retrieval and key-rotation refresh. A key-service
outage remains retryable; expired keys and invalid signatures are rejected.
Missing plan permission preserves sign-in but prevents pool admission/inference;
an identity-only reply saves verified identity without access or refresh tokens.
Management request preparation checks this permission before registering tokens
with the shared authority and again after preparing the current credentials.
An unexpired token alone cannot enable plan usage.
An explicit later sign-in requests consent. Refresh timing accepts Unix seconds
or RFC3339 dates and retains omitted grant scopes on renewal. SIWC rotation
checkpoints fence an uncertain refresh reply. Received replacements are persisted
before fallible identity verification, outside usable credentials and authority.
Restart or cancellation resumes verification of the same protected reply, bound
to the current registration and credential generation, without refreshing twice.
An unresolved rotation outcome or invalid checkpoint persists the existing
reauthentication state with `token_rotation_recovery_required`; it stops automatic
refresh and pool admission until a newer sign-in. A temporary vault I/O or
write-verification failure remains retryable and preserves the checkpoint and
credentials. Request diagnostics distinguish recovery from credential-storage
unavailability.
Hard deletion attempts provider revocation before removing local credentials;
identity-only sign-ins and unactivated imports have no owned renewable session
to revoke. Revocation selects the latest protected rotation replacement under
the credential lock without activating it. Unknown or invalid rotation outcomes
retain the connection rather than revoking a consumed token and claiming success.
Trash is reversible and does not revoke. Remote revocation cannot be undone if
a subsequent local delete fails: rollback restores local data but closes the
gateway; that connection needs reauthorization before restarting the gateway.
SIWC uses account-scoped public `/v1/models` and `/v1/responses` HTTP/SSE,
`store=false`, complete input history, and terminal `response.completed`.
An upstream JSON reply to a streaming request goes through the existing bounded
response collector and terminal/model validation. Valid completed or incomplete
Responses bodies use the shared SSE encoder and client-protocol adapter; provider
errors retain their failure semantics. This compatibility path never redispatches
merely to obtain SSE, and synthetic events do not establish upstream token timing.
Client `previous_response_id` continuations use the same bounded, owner-validated
native replay as BPS before HTTP preparation, including the client WebSocket
bridge. The upstream receives the full saved input, completed output and new
items, never an HTTP response reference. Missing, foreign-key or wrong-model
replay is rejected before dispatch rather than dropping context or changing
accounts. The [SIWC inference contract](https://developers.openai.com/siwc/token-sharing-open-source/models-and-inference)
requires the public API, not ChatGPT backend endpoints; its
[preview limitations](https://developers.openai.com/siwc/token-sharing-open-source/preview-limitations)
distinguish stateless HTTP from same-authenticated-connection WebSocket state.
Native replay and buffered responses preserve completed reasoning ciphertext
from `response.output_item.done`, including when the terminal snapshot omits or
replaces it. The shared snapshot policy also serves protocol conversion; item
index/identity must match before opaque state can be reused. See the
[Responses item contract](https://developers.openai.com/api/reference/resources/responses/subresources/input_items/methods/list).
The client WebSocket `generate` option is not forwarded as an HTTP parameter.
`generate: true` keeps normal generation; `generate: false` maps to the documented
Responses `prompt_cache_options.prewarm: true` without changing cache mode or TTL.
Only an explicitly requested prewarm accepts a completed zero-output SSE response;
provider failures, missing terminals and empty ordinary generations remain errors.
JSON and SSE clients retain the provider's warmup usage and can continue using
the same owner-validated local history. Relay never prewarms ordinary turns by itself.
See [WebSocket warmup](https://developers.openai.com/api/docs/guides/websocket-mode)
and [Responses cache prewarm](https://developers.openai.com/api/docs/guides/prompt-caching).
Unsupported semantic options/tools are rejected, not stripped. SIWC compaction
uses Relay's existing text-summary bridge on the same `/v1/responses` account
route, for both compaction triggers and `/v1/responses/compact`. Summary requests
omit tool declarations and return a Relay-owned portable checkpoint; subsequent
turns unwrap only that checkpoint. Foreign encrypted compaction remains an
explicit incompatibility. Native Codex compact endpoints are not used by SIWC.
Bare function/custom declarations move to `additional_tools` with the required
`role: "developer"`, without renaming calls or results. Existing tool-loading
history stays in place: unchanged definitions are not duplicated, and new or
changed root definitions append after history, preserving current-root precedence.
Without tool-loading history, the initial catalog still precedes the input;
legacy calls missing a declaration retain a prefix definition before replay.
Relay does not invent a hidden cross-request catalog or remove client tools.
Large SIWC catalogs (at least 24 client tools and 24 KiB of declarations) use
Relay-local discovery through the ordinary native `relay_catalog` function.
The initial request retains built-ins, a bounded eager set and a deterministic
name index. Lookups append the requested original definitions using
`additional_tools`; executable function/custom calls retain their native names,
namespaces, schemas, IDs and raw custom inputs. No BPS tool envelope, metadata,
attachment upload or endpoint is used. Explicit historical declarations and
media remain in place; replayed client calls receive their definitions before
the call, and current overrides follow history. Cache options and reasoning
remain unchanged. Required, specific or allowed-tool choices, small catalogs,
reserved-name collisions and insufficient dispatch budgets keep native forwarding.
The local catalog helper must not shadow a client's historical bare `relay_catalog`
declaration or call, even if the current root catalog omits it. Those requests
keep native forwarding; a namespaced client tool with that leaf name is distinct.
Local lookups share the request budget, lease and exact authorization incarnation;
they cannot change accounts, replay executable calls, or execute client tools.
Malformed local reads can be corrected within eight bounded lookup rounds.
Usage is recorded for each actual generation; hidden lookup usage is not free
and missing cache-write counters remain unknown. Native requests gain no BPS
idle timeout. Streaming withholds executable calls until terminal validation.
The documented SIWC prohibition on Responses `tool_search` also applies to
client-executed search: this local function is not provider tool search.
Clients that replace instructions, definitions or earlier history can still
invalidate the provider prefix; this conversion does not guarantee cache hits.
SIWC shares the account cache-key policy without inheriting Codex normalization:
an omitted `prompt_cache_key` is derived from the local key, model and identified
client context for HTTP/SSE and the WebSocket-to-HTTP bridge. Explicit caller
values remain untouched; absent context does not create a global cache key or
authorize continuation. This key does not guarantee a provider cache hit; on
GPT-5.6 and later it separates cache accounting rather than changing cache routing.
Cache modes, breakpoints, TTL and reasoning context stay caller/provider-owned;
Relay does not force a paid longer TTL, change reasoning effort or drop history.
For stateless Responses, replay complete output including compatible encrypted
reasoning; current OpenAI APIs return it by default with `store=false`.
References: https://developers.openai.com/api/docs/guides/prompt-caching
and https://developers.openai.com/api/docs/guides/reasoning
Client WebSockets use the existing HTTP bridge; native upstream WebSockets are
not enabled. An initial SIWC/BPS handoff retains the selected account and its
unstarted reservation; changing transport does not select a second pool member
or advance rotation twice. The HTTP executor rechecks compatibility and identity,
and normal dispatch fences still enforce live scope and membership.
A structured `subscription_sharing_usage_limit_exceeded` rejection before generation
uses the existing cooldown and bounded pool fallback, for HTTP 429 and an early
SSE failure alike. Diagnostic prose, native Codex quota codes, and bare HTTP 429
do not authorize SIWC replay. Known SIWC discriminators take precedence over
message text; unknown SIWC codes retain status-based handling. Explicit invalid
or unknown discriminator fields fence fallback fields and error envelopes;
duplicate JSON keys cannot establish SIWC replay permission. Selection still
respects configured membership, model/key
scope and continuation ownership; Relay never injects another billing route.
Other SIWC failures remain terminal. Any generation evidence or uncertain
execution forbids replay, even when the terminal error reports exhausted quota.
The rejection does not establish a whole-plan balance or a reset timestamp.
At EOF before any SSE frame, bootstrap preserves a recognized JSON error body
up to 64 KiB, including its actual HTTP status, code and safe parameter. Other
early EOF diagnostics retain only content-type/body-shape classes and byte
counts, never body snippets. This observation does not establish safe replay.
Reference: https://developers.openai.com/siwc/token-sharing-open-source/errors-and-recovery
The structured `subscription_sharing_user_not_eligible` refusal has its own
diagnostic category. It ends the request without credential repair, account
cooldown, health penalty or automatic account switching; it does not revoke a login.
Runtime projections retain a quota/rate wait as `quotaWait`, rather than
collapsing it into unavailable. Account cards show the waiting state even when
SIWC does not provide counters; this does not create a zero quota measurement.
Configuration/auth failures and transient provider outages remain distinct.
Codex quota endpoints/headers, Agent Identity, direct native-profile writes
and Hosted transfer are not SIWC capabilities. Plan
limits are not a new independent allowance; absent quota evidence stays unknown.
Missing SIWC quota counters are displayed as not provided, not as a queued check.
Keep optional plaintext plan/expiry claims only from a verified ID token; neither
their presence nor quota counters are promised by the public SIWC token contract.
Encrypted access-token metadata stays opaque. Do not substitute Codex quota
endpoints or infer a plan from model access, scopes, email or another connection.
Reference: https://developers.openai.com/siwc/token-sharing-open-source/token-reference
Live provider acceptance remains separate from automated local checks.
Contract: https://developers.openai.com/siwc/token-sharing-open-source/sign-in
and its token, models-and-inference, preview-limitations and recovery references.

### Account bundles and linked authorizations

Zenith account bundles preserve all three OAuth client kinds. SIWC exports retain
the issued client ID, subject, issuer, host ID, scopes and refresh timing alongside
the tokens; Codex-only and unqualified third-party formats reject SIWC rather
than substituting the bootstrap client. Import detects the kind and checks
registration consistency. File metadata is not proof of authorization: every
SIWC import requires a fresh sign-in before token use, refresh, model
discovery or pool admission. Reauthorization reuses the issued client with this
installation's distinct host ID and verifies the selected subject. Import cannot
overwrite an existing SIWC connection; deleting an unactivated import does not
revoke the source host's grant. This conservative Relay policy prevents copied
rotating grants from becoming two active sessions; it is not a provider ban on
sharing a client registration. Host/client contract:
https://developers.openai.com/siwc/token-sharing-open-source

Credential authorities remain separate even for the same principal. Local account
cards can explicitly associate up to one Codex, SIWC and Excel/BPS connection.
The client badge selects the route in both Accounts and Rotation; only that
connection joins the pool. Selecting another method first reuses its grouped
connection or a single eligible saved connection. Rust validates identity and
rejects ambiguous saved matches; OAuth starts only when no eligible match exists.
After OAuth completion, association finishes before the renderer refreshes the
account snapshot. A failed association retains the saved credentials, closes the
completed sign-in and reports the error; retrying selection reuses the saved
connection. Association does not copy tokens, infer entitlement,
combine quota ledgers or automatically match accounts by editable login notes.
Codex/BPS association checks the saved principal and workspace; SIWC association
checks the original sign-in identity, not a workspace claim or shared allowance.
Association checks every existing connection, so SIWC cannot bridge mismatched
Codex/BPS workspaces or principals.
The user must select the intended workspace during authorization. Existing
connections are not silently merged. Each route keeps its models, quota,
authorization and renewal. Common login notes reference one retained credential
record; other saved notes are not erased. Pool policy, weight and concurrency
follow the selected connection. A switch fences dispatch and rejects while any
affected route has active requests; continuation identifiers are never rewritten.
Group membership and selected route are saved atomically with account records.
Explicit account selectors in automations follow the selected route in the same
transaction, without moving wake history or reset-credit fingerprints. A switch
rejects an enabled automation whose explicit model is unsupported or excluded by
the destination; edit or disable that automation first. Runtime rollback restores
task targets while preserving newer wake history.
Refreshing an inactive connection persists its own metadata without rebuilding
the active gateway or publishing another dispatch candidate.
The existing detach command retains credentials outside the pool; each card
resumes its own saved notes, without copying or overwriting credentials. It is
not exposed by the current account-card UI.
Shared notes remain with their original owner; the other card may need its login
notes entered again. Account-card deletion/export
explicitly includes all grouped connections; exported registrations remain
separate, and local card associations must be re-established after import.
Account and pool cards show the plan separately from the issuing-client badge:
Codex, SIWC or BPS. Missing legacy client metadata displays as Codex.
When SIWC omits its plan or quota windows, explicitly grouped Codex/BPS
observations fill the existing plan badge and quota meters without an extra section.
Stored SIWC metadata remains unchanged. Fresh primary/secondary windows from
the newest usable linked Codex or BPS connection also rank SIWC in automatic
rotation while SIWC has no own quota evidence. The reference retains its source
and expires at the oldest window's freshness bound, any reported reset, or
subscription expiry. Missing counters, failed reads, recovery, unavailable
credentials and reauthentication cannot supply this ranking signal.
Without a scheduler snapshot, automatic pool-card ordering uses the same linked
display observation when fresh; live scheduler ranks and resource evidence remain
authoritative. Reference observations never transfer provider credits or access.
Association does not verify SIWC's workspace or equivalent client allowances.
Search, filters and groups use the selected connection's own metadata. A missing
subscription date can display a linked Codex/BPS date for the same displayed plan,
with its source in the tooltip; it never changes the saved subscription or access.
Each OAuth connection displays only its own token expiry, including Codex and SIWC.
SIWC without its own counters or a reported limit has no quota-polling status.
Legacy quota-polling failures do not color its route indicator; linked-reader
failures remain visible on the reference meters. Own model, authorization,
runtime failures and explicit quota evidence keep their existing precedence.
Display plan and quota sources are chosen independently;
quota recency uses the oldest displayed window observation. Windows are not added
or mixed. Own SIWC zero quota, reported limits and
reauthentication remain authoritative. Recovery, unavailable credentials and
reauthentication exclude reference donors.
Source, observation time and the workspace/allowance caveat are available on
hover and keyboard focus, and in accessible labels, without extra info icons.
Reference data is marked stale
after 20 minutes (the core default observation bound), or earlier at a reported
reset/subscription expiry, failed/stale refresh, or missing/future timestamp.
Neither displayed references nor ranking hints grant execution capacity. Stale values retain
their warning and historical-data tooltip; local timers do not poll providers. Manual quota
refresh uses the owning Codex/BPS reader without switching the selected route,
including when the group has no saved counters yet. Bulk refresh deduplicates
reader accounts. Active group traffic keeps its linked readers on the same
active refresh cadence; the existing shared jobs, backoff and revision fences apply.
Reference refreshes update ranking without rebuilding the gateway or changing
the selected execution route. Zero reference quota only withdraws its ranking
advantage; it does not exhaust SIWC. Own quota, auth, cooldowns, model access and
opaque continuation ownership remain authoritative. References never copy
credentials, balances, reset credits or execution eligibility, and introduce no
extra background requests.
The route selector contains only the three clients. Missing connections start
their sign-in flow directly, without a connection-confirmation dialog. Existing
connections switch directly. The account-card menu has no connection-management
entry or dialog. Association still follows completed, validated route sign-in.
Route sign-in notes read and save the original card's existing note owner,
separately from the OAuth reauthentication target. This does not reuse another
client's grant or copy notes into an unverified account. Ordinary new-account
sign-in keeps its independent blank draft.
On Windows, macOS and Linux, ordinary selection of an unconnected Codex or
Excel/BPS route requests the existing protected automatic-login driver. The
route selector has no right-click sign-in shortcut.
The native command validates a separate notes-source account, never treats it as
the new client's reauthentication target, and keeps its proxy/profile context
through start, resume and token exchange. Pending sessions are additionally
scoped by that source account; legacy snapshots default to no source. Missing
email/password falls back to manual route sign-in. The sign-in dialog omits the
automatic-login description and retains its manual continuation action. SIWC
uses manual sign-in. Source removal/recovery stops automation
before another form fill.
Refresh, reauthentication, import, export, and transfer preserve the issuing
client. Legacy credentials without a client hint remain Codex; captured BPS
headers do not establish an OAuth client.
The connection determines the transport; there is no global BPS switch.
Legacy global flags are ignored on read and omitted from settings, presets,
and runtime snapshots. Per-account BPS facts remain credential-derived.

### Native Codex access

Native Codex connections support direct account switching, explicitly linked
SIWC plan/quota display, and explicit Desktop pool membership, image-only by default.
An account-card or pool-picker membership action persists `imagePoolEnabled`;
sign-in, import and presets cannot grant this opt-in to a new connection.
Reauthentication preserves existing explicit consent. Startup strips legacy
Codex membership without consent, while retaining saved credentials and notes.
Desktop sets the core host restriction `native_codex_images_only` unless the
persisted `codexTextModelsEnabled` opt-in is true (missing/legacy values default
to false). Without it, native executors publish only the image model and reject
every text dispatch, including after catalog refresh or model-policy edits.
**Settings → Pool data → Text models through Codex sign-in** enables the opt-in
only after an explicit risk dialog and a red confirmation button's 10-second
delay. Cancelling or reopening does not grant consent; reopening resets the
delay. Disabling needs no dialog. IPC requires the expected current value and
explicit risk acceptance when enabling. The setup guard, dispatch fences and
existing runtime-rebuild/rollback transaction publish the change; no accounts
are enrolled, removed or reauthenticated, and image access and degraded-route
blocking remain unchanged. The opt-in applies to existing and future explicitly
added Codex members, not SIWC, BPS or API-source routes. Shared core defaults are
unchanged. This is experimental Relay behavior, not an OpenAI support guarantee.
The [official Codex sign-in guide](https://github.com/openai/codex#using-codex-with-your-chatgpt-plan)
describes ChatGPT sign-in for Codex and separate API-key setup; it does not
establish third-party pool support or a specific model-substitution policy.
Leaving the pool, restoring from trash, or switching routes clears image consent.
Selecting Codex removes the group's pool membership and routing policy;
it can then be added explicitly, with text subject to this separate opt-in.
Enabled account-targeted automations must be edited or disabled first.
SIWC and BPS are pool-only and have no direct-launch button. They do not write
native Codex authentication. Plan names, including Free, do not block sign-in,
import, pool membership, discovery, or dispatch. BPS execution still requires
fresh provider access evidence; a plan name alone does not prove access.
Plan metadata updates invalidate cached access without disabling the account.

Client-login observations use only an unambiguous loopback CDP endpoint. Each
snapshot exchange has one connect/send/reply deadline; control frames and
unrelated responses do not renew it. Observations affect warnings and direct
launch, never saved credentials or pool admission.
CDP discovery bypasses system proxies and does not follow HTTP redirects.
Managed ChatGPT catalog reads also reach the local gateway directly, without
system proxies or HTTP redirects; provider proxy routing is unchanged.

### Model discovery and route evidence

Discovery keeps model inventory and executable-route evidence separate. Names,
`/models` rows, groups and reference entries describe inventory; endpoint access
requires its own evidence. Refresh reads metadata rather than generating answers.
Endpoint declarations retain their origin; address/key changes invalidate evidence.

The renderer preserves Rust's resolved routes, including empty model lists.
Only an unresolved sole legacy native binding may inherit the source catalog.
Invalid adapters, duplicate routes, unavailable model references, and cache TTLs
on non-Messages routes cannot become native-launch or pricing capabilities.

Reference metadata supplies model identity, limits, modalities, tools, reasoning,
service tiers, and native-protocol policy. Declared, confirmed, unsupported, and
unknown evidence stay distinct. Missing limits/capabilities remain unknown;
participant metadata does not become universal model truth.

Within the same provider/model scope, a declared effort enum takes precedence
over individual positive effort flags. Nested reasoning and option enums survive
source enrichment and cache reloads; a scalar effort is not a complete enum.
Partial flags remain a fallback when no enum is published. Primary explicit
enums, supported defaults, unsupported evidence, route restrictions and operator
policy are preserved; levels are neither hardcoded by model name nor borrowed
from another host.

Model Rules exposes read-only reasoning diagnostics even when a non-image
model has no effort selector. The existing snapshot supplies the reference
source/model, support flag, control method, reference/runtime levels and pool
policy projection. Unknown evidence, explicit unsupported evidence, non-effort
controls and an empty policy remain distinct. A reference/runtime discrepancy
does not invent a cause or establish provider acceptance. Opening diagnostics
does not probe, refresh discovery or modify model settings; an open dialog
follows updated snapshots. Missing fields remain distinct from empty lists.

Validated source `/models` token limits are endpoint-local restrictions, not
canonical model metadata. `contextWindow`/`context_window`/`context_length`,
`maxInputTokens`/`max_input_tokens`/`inputTokenLimit`, and
`maxOutputTokens`/`max_output_tokens`/`outputTokenLimit` accept positive integers;
conflicting aliases and inconsistent limits stay unknown. Gemini's separate
[input/output limits](https://ai.google.dev/api/models#Model) do not imply a
combined context window; OpenRouter's
[context field](https://openrouter.ai/docs/guides/overview/models) is distinct
from a generation default. These restrictions may narrow a known reference
maximum or fill an unknown one, never enlarge it or grant an endpoint/feature.
Fresh complete discovery replaces old restrictions; address/key invalidation
also removes them. Admission and adapter preparation reject excessive explicit
output limits without clipping requests. API Codex cards use the most restrictive
known limits among the key's available routes; native account cards retain their
own context policy. Input token counts are not guessed from character counts.

Disjoint nonempty source/reference reasoning effort lists are a metadata conflict,
not evidence that reasoning is disabled. No effort is advertised or substituted;
explicit effort controls (including `none`) fail with `adapter_metadata_conflict`
before execution, while an otherwise compatible request without an effort keeps
provider defaults. Explicit unsupported evidence still denies reasoning. This
stage does not ingest participant reasoning/tool capability flags.

Explicit endpoint configuration and the source's per-model endpoint catalog
take priority over manufacturer defaults. When the source declares the client's
protocol, use native forwarding before conversion, without a preliminary failed
conversion or generation probe. Hosted tools and their options stay native;
declaring a protocol does not independently establish hosted-tool availability.
Exact negative evidence and an endpoint pin remain authoritative. Missing native
support uses an available declared upstream through the shared adapter; it does
not silently remove requested features or emulate search as a client function.
Only without exact source evidence do known identities use the manufacturer
default from `model_metadata/protocol_policy.rs` and `catalog/protocol-policy.json`:
OpenAI Responses, Anthropic Messages, Google Gemini, and xAI/Z.ai Chat Completions.
Service profiles and model names cannot recreate an explicitly rejected endpoint.
Unknown identities use their exact endpoint evidence and configured
preferences/fallback. Existing continuation ownership cannot change protocol
because source declarations or preferences change. Several bindings share one
physical member, quota, and capacity.

Display identity never rewrites route IDs. Exact qualified IDs precede a unique
leaf; ambiguous leaves stay separate. [Catalog policy](../../catalog/README.md)
owns display order and public export. Saved manual order overrides the default;
inventory order and member rotation order are independent.

Standard is the baseline speed. Fast/Ultrafast require model and transport
support; explicit source tiers may narrow the set. BPS carries Standard only.
The pool default is captured once per logical request, explicit client tiers
are preserved, and temporary health does not rewrite configured choices.

## Statistics and refresh

Desktop usage persistence runs through one shared, bounded 32-entry writer per
host state. Its callback acknowledges after the existing SQLite, account-state
and notification work finishes, not at enqueue. On the multi-thread Tokio runtime,
waiting hands the request worker back to the executor; synchronous/current-thread
callers retain their blocking acknowledgement contract. Queue pressure never
drops accounting. The last owner drains and joins the writer; an uncertain failed
write is reported, never replayed. Failure to start the worker retains synchronous
persistence. Runtime quota application and revision guards keep their owners.
Observation time is captured before enqueue; queue wait never makes quota data newer.
Delayed callbacks cannot move account, source or key last-used times backwards;
timestamp strings are compared as instants, not lexicographically.
Account observations with a known older credential generation remain telemetry-only:
success, quota and failure results cannot modify the newer account or complete
its natural-use wake state. Successful access-token use does not clear a persisted
terminal refresh-grant or rotation-recovery sign-in requirement.
The same generation fence protects runtime health and HTTP/WebSocket quota
observations. Observation commits hold the existing authority slot and revision
guard; a busy refresh skips state changes without waiting for provider I/O or
dropping usage telemetry. Terminal sign-in requirements cannot become a healthy
runtime candidate because an in-flight request completes. Pending passive quota
is consumed for persistence under that same fence, so an obsolete completion
cannot take a newer login's unsaved observation.

Successful native Codex Responses HTTP/WS handshakes may supply `X-Models-Etag`.
A first or changed version schedules the existing fenced account-model refresh;
it never grants model access or overrides reference capabilities. Hints accept
only the native HTTPS/WSS backend and current credentials. Only digests are
retained, bounded by the runtime's account inventory. Changed versions coalesce
with a one-minute minimum scheduled interval; unchanged versions do not poll
again. Host startup, recovery and refresh eligibility remain authoritative, and
a busy host defers observation to a later response. BPS, SIWC and arbitrary API
origins do not participate.

- Balance, key allowance, subscription allowance, quota windows, and provider
  credits retain their own units and observation times. Unknown is not zero;
  failed reads preserve the last value as stale. Statistics alone grant no route.
- Access-token expiry and subscription expiry are separate metadata. Imports
  retain supplied access-token expiry and authenticated subscription dates;
  an ID token's expiry does not describe the access token. Missing dates stay
  unknown. Passive quota updates cannot discard an independent subscription read.
  Quota-only polls and failed metadata reads do not advance subscription freshness.
  Desktop reads subscription metadata once after authorization recovery, preserves
  the last known date on failure, and records redacted diagnostics without treating
  an optional metadata failure as a quota or account-access rejection.
  Failed subscription reads pause for at least one minute when transient, or
  30 minutes otherwise, preserving a longer Retry-After. This pauses only
  subscription reads for the current registration, including manual retries,
  not quota polling or a replacement login/configuration. Polls never re-import
  a subscription date from old token claims after the provider has cleared it.
  A persisted successful metadata read also keeps the 30-minute interval when
  the provider omits an expiry; ordinary quota reads cannot reset this floor.
  Subscription failures record the fixed endpoint stage (`accounts_check` or
  `subscriptions`) and content-type category alongside the HTTP status, without
  retaining response bodies, URLs or authorization headers. An HTML refusal
  remains a refusal, not evidence that the account lost model access.
  The documented `cf-mitigated: challenge` response header adds a fixed
  `cloudflare_challenge` diagnostic; absent or unknown values remain unreported.
  HTML alone does not establish the refusal's origin. See Cloudflare's
  [challenge detection contract](https://developers.cloudflare.com/cloudflare-challenges/challenge-types/challenge-pages/detect-response/).
  Subscription diagnostics include the OAuth client kind, total probe elapsed
  milliseconds (including management admission and optional retries), and a
  validated Cloudflare Ray id when present. Arbitrary header values are omitted.
  HTTP error status and Retry-After take precedence over an unread or truncated
  error body; a broken refusal body cannot trigger a transport retry.
  Subscription metadata must match the requested account's canonical id or an
  alias present in the authenticated response. Canonical matches take priority;
  a missing account never falls back to another account's ordered subscription.
  A failed optional `subscriptions` lookup retains the authenticated account-check
  fields and its separate diagnostic, without refreshing a retained expiry date.
  A confirmed plan change clears the previous plan's expiry and freshness; explicit
  account ids in fallback payloads must match the selected canonical account.
  Transient fallback retries repeat only that lookup and honor Retry-After.
  Per-account quota locks own single-flight; the shared subscription-floor mutex
  is released before network I/O and persistence. Management HTTP admission
  continues to bound concurrent requests across accounts.
- Model, balance, quota, and usage refreshes are independent resource-scoped
  jobs. Readers join in-flight work. Manual refresh does not cancel shared work.
  Configuration/credential/delete revisions fence late results.
- Passive window headers do not refresh credit-ledger evidence. A reset time
  without a remainder does not refresh that remainder. Traffic cannot postpone
  a required full poll indefinitely; stale credit-only admission expires.
  Queued and persisted snapshots reconcile present windows and complete credit
  ledgers by their own observation times; equal-clock conflicts keep current
  evidence. A newer authoritative snapshot can still remove absent windows.
  Preserved unsaved observations retain their persistence hints, and a delayed
  exhaustion report cannot zero a more recently observed primary remainder.
- The last positive 1% schedules a quota verification after one minute, or at
  an earlier reported reset, through the same shared refresh job. Fresh headers
  do not postpone it. Zero remains exhausted; provider Retry-After, backoff,
  concurrency limits and account revision fences still apply.
- Reset verification retains its stable 5–15 second per-account delay even when
  the countdown reaches zero during a read. An overdue, unverified window is due
  immediately; a window observed at or after that verification deadline consumes
  the check, including when it still reports zero. The ordinary cadence and
  failed-read backoff remain in force; no per-card polling or inferred refill.
- Shared credit totals count one provider ledger once, using its newest valid
  observation, not emails or the newest unrelated header timestamp.
- Saving or enabling a countdown automation schedules its selected eligible
  accounts through the shared quota refresh owner, retaining provider backoff.
  Successful reads recover already-full primary windows with their persisted
  cycle identity. A still-full window gets a new cycle only with a changed provider
  cycle or a new reset after the previous deadline; relative drift creates none.
  Canceled unclaimed reservations release their identity, while claimed work
  retains same-cycle replay protection. Failed reads do not start countdowns.
- Service profiles own endpoint roots and supported statistics readers. An
  unsupported billing reader does not probe unrelated APIs or block inference.
- Native model discovery has a bounded timeout and one paced transient-read
  retry inside the same refresh job. HTTP rejection, malformed input, permission
  failure, and provider Retry-After follow their own policies.
  Initial OAuth discovery and later refreshes share account-state transitions:
  a catalog 403 records `models_forbidden` and degrades an otherwise healthy
  account without inventing models or blocking the whole account. Independent
  authentication, verification and account-disable failures retain precedence.
  Legacy `blocked` + `models_forbidden` records are repaired to `degraded` on
  load and discovery, retaining the catalog error until a successful accepted
  read. Native Codex/BPS empty reads retain their last good inventory. A successful
  SIWC read with no visible models clears current discovery, including through
  reauthorization's settings merges; its saved baseline remains intact but is
  not routed. Failed discovery preserves the last observation, including an
  empty SIWC catalog. Repair preserves authentication failures, inventory,
  membership and operator choices; it never supplies models or bypasses BPS access checks.
  Failed login/import/refresh model probes use the existing error log to retain
  the attempt time, static endpoint stage, configured-proxy/default transport,
  safe code and HTTP status. Default transport is not proof of bypassing system
  proxies. No proxy URL, credentials or response body is logged.
  Account error JSON separates the current route configuration from unknown
  historical request routes; unrelated quota timestamps never date model errors.
  An empty local ChatGPT catalog returns `pool_models_unavailable`, independently
  of quota availability, and does not install an empty client profile catalog.
  Requests to `/v1/models` with `client_version` always retain the Codex
  `models` envelope, including while BPS access is denied, throttled or not yet
  ready; only ordinary OpenAI discovery uses `object`/`data`. The next successful
  access probe restores the catalog in the same runtime, retaining the existing
  bounded access cache and provider Retry-After policy.
- Token refresh/invalidation is revision-bound. A late 401 or invalid grant
  cannot overwrite a newer login, delete replacement credentials, restore a
  deleted account, or change operator disable/drain/membership choices.
  Desktop model/quota reads and reset-credit authorization retain the exact
  prepared token revision through 401 recovery, including when a replacement
  session reuses a generation number or identical token bytes.
  Refresh-error JSON is limited to 32 KiB and rejects duplicate object keys.
  An explicit nested `error.code`, then top-level `code`, takes precedence over
  OAuth `error` and `error.type`; a malformed explicit code stays unknown.
  Ambiguous envelopes preserve credentials, and `refresh_token_reused` remains
  transient because another refresh may already have rotated the grant.
  Access-only expiry applies to the rejected token snapshot and preserves an
  existing terminal sign-in reason. The five-minute renewal window applies only
  when a refresh grant exists; access-only tokens retain their actual lifetime.
  Native OAuth rotations use the credential store's existing protected checkpoint
  mechanism. A known successful rotation survives storage failures in the shared
  authority's pending-persistence state; retries save that result rather than
  exchanging the old grant. A completed checkpoint can recover it after restart.
  If storage never accepted the rotated pair and the process exits, the retained
  pre-exchange barrier requires sign-in instead of reusing the consumed grant.
- Account trash retains credentials and usage but excludes the registration
  from routing, monitoring, exports, and presets. Restore returns it disabled
  and outside the pool with a new revision. Permanent deletion is explicit.

## Pool and execution

### Native image operations

`POST /v1/images/generations` and `/v1/images/edits` use the existing image
lane, key scope, credential authority and physical-send usage pipeline.
Desktop users first add the selected Codex connection to the pool for images;
this does not require launching the Codex application. Membership alone does
not enable text requests; the separate `codexTextModelsEnabled` opt-in does.
Native Codex accounts call their `/images/*` endpoints directly with the image
model; no text-model Responses tool request is inserted. Text-model inventory
or pricing changes do not choose an image model. The legacy `imageBaseModel`
setting remains import-compatible but has no effect on native image execution.
Native account requests carry a Relay-owned `x-codex-image-turn-id`, stable
through authorization recovery; API sources do not receive this account header.
Hosted Responses `image_generation` and native Images access are separate
capabilities. A native Images 403 is not retried as a text-model tool request.
Excel/BPS and SIWC are not image executors. Published native image routes do
not prove account entitlement or available image quota, and plan names alone
neither grant capability nor exclude Free accounts.

JSON and multipart edits preserve image ordering and masks. API-source
multipart requests replace the public model alias without changing uploaded
bytes. GPT Image's legacy `response_format` is handled locally: nonstreaming
`url` results are data URLs, not hosted files. Streaming requires `b64_json`.
Decoded requests and buffered responses are limited to 128 MiB; a buffered SSE
event is limited to 64 MiB. No new account generation timeout is imposed.

SSE partial images are forwarded as they arrive. The response body owns its
lease and finalization guard. Completion, missing output, invalid/truncated
streams, provider errors and client cancellation are distinct; accepted or
uncertain execution is never replayed. Already observed usage survives errors
and cancellation and is recorded once, not once per preview.

`imageUsage` stores text/image/cache token subsets and completed-image counts
on retained sends and in diagnostics. Subsets are included in ordinary token
totals, never added a second time. Missing counters remain unknown. Current
token-only price aggregates cannot price a mixed-modality image request and
keep it unpriced rather than apply a text rate; API equivalents are not actual
subscription credit debits. The detailed split follows existing request
retention; archived aggregate token totals remain available.

The public wire reference is OpenAI's image-generation guide:
<https://developers.openai.com/api/docs/guides/image-generation>.
Native OAuth routing is observed, not an official account API guarantee:
CLIProxyAPI `8289ec4c71ad914d0612c2776352b76ba3d355a6`,
`internal/runtime/executor/codex_openai_images.go`, and codex-proxy-rs
`31974bc289b13e65856cbfefc082941a84dc3a88`,
`backend/crates/providers/openai/src/transport/endpoints.rs`.
Synthetic tests do not establish entitlement or charges on a real account.

Explicit cache keys remain stronger than session hints. Observed Claude Code
child/parent metadata can share soft parent affinity while keeping each child's
context separate. Without an explicit key or session, HTTP and client WebSocket
requests may derive a caller-scoped hint from the exact initial instructions and
opening user turn. This bounded (64 KiB), process-salted hint requires at least
32 bytes of user text and uses the existing model-scoped scheduler bindings.
It stores no prompts, creates no provider cache key, and grants no continuation
ownership. Arbitrary compacted tails without the opening turn are not inferred
to be the same conversation; stable client/root hints remain necessary there.

Automatic selection first filters compatible, enabled, available members, then
compares normalized load. Independent work uses the least-loaded members. A
warm preferred member remains in that group when subtracting one of its existing
requests puts it at or below the least normalized load. This bounded affinity
margin does not add a wait, bypass shared physical capacity, or change member
limits. Fresh positive quota takes precedence within the group, including the
explicit linked-connection ranking reference for SIWC with unknown quota; candidates
less than five percentage points below the best fresh quota remain in the same
selection band. Only the primary warm preferred member has a ten-point quota
margin: it yields at a lead of ten points or more. Independent work and warm
alternatives retain the five-point band. The wider primary margin does not
bypass quota freshness, positive remaining quota, availability, load or health.
Fresh provider credits are the fallback. Soft health/first-output latency and request weights
resolve remaining choices. Wallet prices do not rank members. Missing latency
does not exclude a member. Soft affinity can yield to a better eligible member;
opaque response ownership cannot.
Routing diagnostics also capture the selected candidate/model/operation's
first-output latency before the request. A bounded in-memory window retains at
most 64 successful physical attempts from the previous ten minutes; missing
measurements have a separate count. Nearest-rank P50/P90/P95 require 20 measured
attempts. Unknown, insufficient and expired samples stay distinct, and freshness
is relative to the saved selection time. These diagnostic quantiles leave the
existing EWMA selection policy unchanged and reset with route replacement/removal.
HTTP SSE diagnostics additionally record the first nonempty decoded body read,
first complete parsed SSE JSON and first meaningful upstream output during
bootstrap, before conversion or tool buffering. These are dispatch-relative
offsets, not additive CPU durations or client receipt times. Heartbeats,
metadata-only events and output are separate observations. A preserved
non-identity Content-Encoding leaves the decoded-body boundary unknown; no raw
pre-decompression or network-arrival boundary is invented. JSON, native
WebSocket and old records do not acquire synthetic HTTP SSE phases. Known
observations survive bootstrap failure without changing retry certainty.
Proxy checks retain their existing HTTPS-only elapsed time and expose separate
queue, response-header/body offsets and full-check time (including optional
location/address work). Header time includes client setup, the shared HTTP gate
and upstream wait; it is not a standalone DNS/TCP/TLS measurement. No additional
provider requests or inference probes are introduced for these diagnostics.
Soft prompt affinity follows the admitted native route of its physical member
when protocol aliases are deduplicated. It cannot restore an excluded member
or move an opaque continuation to another route.
After a successful automatic resource rebalance, the durable prompt-cache
binding follows the warmed member when its fresh quota or credit evidence has
the same material lead required by admission. Temporary load spillover and
small resource differences leave the owner unchanged; an older owner's late
response cannot reverse the move without a new material resource lead.
Prompt binding writes, invalidations, and lazy restores are ordered per opaque
key across runtime replacements without holding the scheduler lock during
storage I/O. Late refreshes cannot shorten a newer expiry, lazy restoration
cannot replace a live owner, and retired runtimes cannot change prompt bindings.
Cache-write rejection clears only the failing candidate's own prompt binding;
a spillover or former owner's rejection cannot discard the current warm owner.
Successful load spillovers also retain up to four volatile alternative-member
hints per prompt binding, ordered by their last successful observation. If the
primary owner is outside the admitted group, Automatic prefers a previously
successful alternative within that same load/resource/health group before
weighted rotation. Alternatives receive no extra load margin, do not replace
the durable owner, and cannot override explicit routing or opaque ownership.
Hints use the binding TTL without extending the primary owner's expiry; owner
replacement/expiry, candidate removal and candidate-scoped cache rejection clear
the corresponding hints. They contain only candidate IDs and expiry times, not
prompts or provider state, and are not restored from durable storage. These hints
guide route reuse. Cache usage remains provider-reported, including zero-cache
sends after a switch.

Requests with a prompt-affinity key also retain bounded `cacheAffinity` admission
evidence in routing diagnostics and the existing technical details/support report.
It distinguishes the preferred member, a previously used alternative, and another
selection, and records the actual availability/load/resource/health exclusion stage.
Fresh quota and credit observations, physical in-flight/capacity counts and the
applicable switch margins are captured before reservation. Missing or stale evidence
stays null. Candidate IDs, cache keys, prompts and credentials are not included.
This is a read-only explanation of the existing policy, not provider cache-miss
diagnostics; old rows without the optional field remain supported.

Automatic response-owner yield probes the same cache-aware admission policy with
the current physical owner preferred within the bounded load margin. Load,
fresh quota, independent credit evidence,
and soft health therefore cannot disagree with ordinary request selection.
A recovery trial or another protocol alias is not a reason to move ownership.
The executor must still materialize reusable history before clearing an opaque
binding; active generations and unknown execution are never moved or replayed.

Runtime snapshots publish `resourceHeadroom` from the same quota and credit
freshness checks used by admission, including protected quota reserves. Desktop
cards use this admission evidence for routing capacity. A provider ledger must
explicitly be usable; its reported
balance alone is not spendable capacity. Snapshots also publish `selectionRank`:
a read-only fallback preference, excluding all aliases of each preceding physical
member through the same selector. Ranking stops where text model/protocol routes
disagree or lack a candidate; it never reserves work or advances rotation.
The preview synchronizes its private scheduler clone once, then prepares every
model/protocol route from that fixed projection. Live dispatch still synchronizes
at admission; this does not cache routing decisions across snapshots or requests.
Cards show members with active requests first, then follow this common preference
left-to-right, then down, ahead of waiting, unavailable and disabled/draining
members. Card contents retain their normal text direction. Only members with
active requests get a subtle card outline, without an activity badge; next
candidates remain neutral. Unranked cards
use fresh resource headroom in automatic mode and saved order otherwise. Timed
waits sort by retry time; quota waits, retry cooldowns and occupied capacity keep
distinct labels. Bare unavailable telemetry alone does not override a healthy
inventory status. New activity invalidates ranks and queues a coalesced lightweight
scheduler refresh; periodic polling remains the fallback. Active highlighting is
independent of readiness, so draining requests remain visible until released.
Recovery and retry details stay in status-icon hints, not duplicate lines in
account cards on Pool or Connections.
Actual selection depends on the request's model, affinity and current load.

Pool card selection is separate from the member editor and uses kind-qualified
identities. Select-all and selected count share the existing toolbar with bulk
refresh, enable/disable and confirmed removal. Mutations use the existing local
commands; membership removal never deletes stored accounts, sources or secrets.
Refresh targets selected eligible members only, deduplicating linked account quota
readers. Partial failures refresh persisted state and retain selection for retry;
removed or replaced routes are pruned from selection.

Round-robin mode follows saved member order, skips unavailable/full members,
and wraps. Equal cycling ignores saved weights and soft affinity. Optional
weighted cycling uses physical-member request shares; shares are not token or
spending targets. Equal-credit ties use saved order, not protocol-alias IDs.
Failover selects the first ready member in saved priority order, without cycling.
A due lower-priority recovery route does not displace a healthy primary; a due
primary recovery still follows the bounded recovery admission policy.
A member limit covers all its protocol routes; zero means no extra member cap,
not unlimited provider capacity. Policy saves are serialized and identity-checked.

Rotation policy version three is advertised as `rotation_v3`. Known version-one
and version-two policies upgrade without changing inventory, member order,
weights, concurrency limits, or their equal-cycling semantics. Stored `in_order`
and `round_robin` remain equal cycling; `weighted_round_robin` explicitly opts
into shares, and `failover` explicitly opts into primary/reserve behavior.
Unknown versions and invalid legacy pairs are rejected, not normalized.
Portable preset imports may migrate validated legacy policies; direct local
updates stay version-exact. Retired server compatibility is not retained.

Presets restore equal request shares and no extra member concurrency limits.
Strategy-only changes preserve those controls. Recommended and even-distribution
preserve member order. Accounts-first places accounts before API sources while
preserving relative order inside each group; its API reserve may spend balance.
Presets rebase onto current membership and never restore a removed member.
Policy-only updates retain active leases, continuation owners, resource evidence,
and cooldowns. No application or gateway restart is required.

Runtime strategies use explicit names: Automatic, RoundRobin,
WeightedRoundRobin, and Failover. Legacy saved `in_order` maps to equal
RoundRobin without changing the stored policy contract. Round-robin cursors
survive member changes while their route remains configured; removing the last
route discards its cursor, including during model-inventory replacement.

Admission queues, waits, and generation attempts are bounded. One logical
request owns its dispatch, transport, auth-repair, retry, and irreversible-send
budgets across HTTP, SSE, WebSocket, and account paths. Retries require:

- a proven not-sent outcome or pre-execution rejection;
- repeatable input and valid continuation ownership;
- remaining request budget and no client-visible response bytes;
- provider Retry-After/reset deadlines respected without shortening them.

With diagnostics enabled, tracked HTTP request/attempt observations include
optional `requestTiming.waiting` counters for local capacity admission, scheduled
recovery, and event-only pool waits. They use the same monotonic wait budget,
survive retry/repair handoffs, and include an active wait when a request is
cancelled after a recorded attempt. Counts and durations are cumulative per
logical request, not additive across attempt snapshots. The last proposed
recovery delay is scheduler evidence, not a raw provider Retry-After or proof of
why every route was unavailable. A missing field is not a measured zero. Fixed
size counters retain no identities, prompts, headers, or provider bodies; queue
full/expired outcomes are enums. Physical attempt latency, TTFT, usage and retry
policy are unchanged. Cancellation before any send still does not invent an
attempt or a usage record.

An uncertain or completed generation is not safe to repeat. Positive reported
output or reasoning usage proves generation even without visible content and
blocks retries and body repairs; it does not establish first-token timing.
Local saturation does not vote against provider health. Request-preparation
incompatibilities stay excluded for that logical request without consuming a
generation or cooling the member. Recovery cannot loop over the same unchanged
invalid route.
Quota, cooldown, health, access, and per-model blocks remain separate states.
Account indicators in Connections and Pool share that distinction: failed quota
or model monitoring warns without marking an otherwise routable account broken;
connection cooldowns warn, recovery probes show recovery, and terminal account
or proxy blocks remain errors. Quota transport/timeouts identify the failed
refresh in the tooltip.

## Continuations, errors, and redaction

An opaque continuation retains its physical owner unless validated local replay
materializes the complete history before dispatch. Complete portable call/result
pairs need only soft affinity; an opaque predecessor is validated separately.
Automatic same-model owner yield uses the same native replay contract as hard
quota recovery, not the plaintext-only model-switch repair. A locally retained
predecessor restores ordered tool calls/results and original reasoning blocks,
including opaque ciphertext without inspecting or removing it. The current
request keeps its instructions, tools, cache key and cache policy. Local replay
checks do not establish provider acceptance across credentials; upstream errors
remain subject to the existing rejection and retry rules.
Missing/foreign/expired replay, unresolved references and explicit context
management or truncation cannot release this binding. Model changes still
require plaintext replay. This transfers conversation history, not provider
cache storage; reported cache usage and owner-scoped turn-state guards stay unchanged.
Historic pairs from earlier owners do not override the predecessor's route.
An unrelated tool-call affinity collision cannot override an exact owned
Responses predecessor when caller-scoped completed-output ownership or saved
replay proves the call/result pair. Hashed output-call ownership shares the
bounded volatile route-affinity store independently of a full materialized replay,
without retaining a second copy of history or transferring it. A reused response
ID replaces its call-set generation; old calls do not accumulate permissions. This proof is
loaded only for a conflicting hint; missing, foreign, expired or model-mismatched
evidence does not release ownership or authorize a result.
Partial histories, unpaired results, unresolved references, or foreign ciphertext
cannot release ownership. Unknown response references return
`409 response_continuation_unavailable`. Compaction checkpoints explicitly
replace the predecessor; they are not discarded to force a retry.

Desktop routing-graph replacements reuse the host's bounded volatile adapter
and native replay stores together with route fingerprints. Late completions
write into the same stores; ownership checks still reject another key, model,
endpoint, or API-source credential. A new host/process starts empty: conversation state
is not persisted to recover a restarted application.
Native replay retains only input history and completed output, not prior tool
catalogs or request options that replay never reuses. Current-turn instructions,
tools and controls remain authoritative. Lookups share immutable snapshots;
only materializing an outgoing request copies their history. Shared ownership
does not extend lookup TTL or discount serialized bytes from the store budget.
Native replay entries may use the shared
16 MiB serialized-history budget;
the total budget, 64-entry cap, one-hour TTL and key/model/route isolation remain.
Under either native replay limit, evict superseded full-history snapshots before
unrelated conversations' latest responses. Supersession requires the same local
key, candidate and model plus an exact prefix containing both the prior input
and completed output; it is a retention hint, not permission to reuse another
response ID. Rejected oversized snapshots do not retire retained state. Active
histories that together exceed the budget still require explicit client recovery.
The converted-adapter store retains its existing entry bound. BPS materializes an owned predecessor
before route preparation, so its lack of provider-side response storage does not
exclude the correct account. Missing local history fails before dispatch rather
than dropping context or reporting another candidate's ownership mismatch.
WebSocket errors preserve typed local adapter failures across the internal HTTP
bridge. Missing local replay is exposed as `previous_response_not_found` with
`param=previous_response_id`, allowing a client to reconnect and resend complete
history; diagnostics retain Relay origin and `adapter_continuation_missing`.
This does not retry an orphan delta, invent history, or reclassify ownership
mismatches. See the [Responses WebSocket contract](https://developers.openai.com/api/docs/guides/websocket-mode)
and [Codex recovery-code handling](https://github.com/openai/codex/blob/f29d55ff0697a88226e8a9811b3c007e5f719cba/codex-rs/codex-api/src/endpoint/responses_websocket.rs).
Full-history continuation keys compare parsed function-argument objects
independently of member order, including nested objects in arrays, following
the [JSON object contract](https://www.rfc-editor.org/rfc/rfc8259#section-4).
Comparison also normalizes exact integral decimal/exponent spellings within
the existing lossless integer bounds and joins adjacent same-role text across
message boundaries. Only comparison copies change; outgoing arguments, retained
native blocks and opaque reasoning remain unchanged. Strings, negative zero,
nonintegral values, array order, call IDs/names, roles, model and route ownership
still distinguish history. Inexact client rounding cannot restore another call.
Adapter compatibility failures before candidate reservation are recorded as
Relay-local request failures for HTTP and WebSocket. WebSocket continuation
rejections at the final pre-send guard use the same diagnostics, including on
reused connections where no in-flight usage event exists yet. Such records have no
provider attempt, token usage, cooldown, or account-health side effect.
Desktop SQLite accepts these attempt-zero local rejections as logical request
outcomes, without inserting a physical send or candidate consumption. Support
exports treat their empty attempt history as complete, not missing telemetry.

An explicit pre-output `invalid_encrypted_content` rejection can repair one
reasoning item on the existing account route. Multiple items require a unique
`input[N].encrypted_content` parameter or the exact observed four-character
prefix/suffix diagnostic. Conflicting locators, duplicate identities/ciphertext,
malformed/duplicate error fields and encrypted compaction preserve the original
history and failure. Visible summaries, unrelated items and tool pairs survive.
The exact legacy code-less `Encrypted content for item rs_...` rejection is
accepted only when it uniquely identifies an existing reasoning item; conflicting
codes or locators remain terminal.
The existing single-item fallback, one-repair bound and owner fences remain;
uncertain execution or already emitted output never authorizes a replay.

Open WebSocket connections apply the same Automatic owner-yield policy as HTTP.
Only validated materialized history can transfer; opaque state keeps its owner
and live connection. Completed native WebSocket turns bind tool-call IDs through
the shared bounded capture, independently of full-history replay retention.
Incomplete and failed turns do not establish durable tool ownership.
Native WebSocket sends carry tool and reasoning diagnostics with the prepared
payload after the final authorization and payload guards. Connection setup,
bridge installation, and subsequent turns reuse those diagnostics rather than
rebuilding the request for telemetry. They are not cached across routes or turns;
continuation validation still runs immediately before payload preparation.

An optional native WebSocket handshake has a 2-second fast-path budget, shared
across authorization refreshes. Only new requests without continuation or
response-affinity ownership, outside half-open health probes, may fall back to
HTTP when that budget expires. No `response.create` has been sent: this is not
a failed generation, account penalty, or permanent HTTP-only route decision.
Required continuations and probes retain the existing connection budget.
Two optional-handshake timeouts within 30 seconds defer further optional
WebSocket attempts on that endpoint/model/authorization scope for 30 seconds.
Requests use the existing HTTP bridge during this interval; a successful native
handshake clears the observation. The bounded in-memory observation is separate
from confirmed HTTP-only capability, account health, and provider quotas.

Direct HTTPS generation requests with buffered bodies opportunistically use HTTP/3 after a successful
background HEAD probe of the same origin. The first request keeps HTTP/2 or
HTTP/1.1 without waiting for the probe. Probes have a 2-second deadline, carry no
credentials or request content, and are bounded to four concurrent probes and
256 candidate/origin pairs. Positive and negative observations expire after five
minutes. QUIC reuses each route's HTTP client and preserves account pool isolation.
Explicit and automatically selected proxies retain TCP; QUIC never bypasses
them. WebSocket upgrades and metadata requests keep their existing clients.
An HTTP/3 send failure or HTTP 421/505 disables it for later requests for five
minutes; the submitted generation is not replayed on TCP. Authorization, cookies, payload
compression, cache keys and dispatch budgets retain their existing owners.
TLS verification stays enabled, 0-RTT is disabled, and Relay adds no QUIC idle
deadline to quiet model generations. Because reqwest 0.13.5 does not apply its
TCP connect timeout to H3, Relay separately bounds H3 startup to 10 seconds until
the transport first polls the request body. The guard ends at that point; it is
not an upload-completion measurement or a response/generation deadline. A startup
timeout is terminal with unknown execution, never a replay or account penalty,
and disables H3 for later requests through the existing failure observation.
Diagnostics report the observed response
HTTP version. `ZENITH_RELAY_HTTP3=0` disables the experiment on the next start.
The current [reqwest HTTP/3 API](https://docs.rs/reqwest/0.13.5/reqwest/struct.ClientBuilder.html#method.http3_prior_knowledge)
requires `--cfg reqwest_unstable`; provider support is established per origin,
not inferred from a model or API name.

TCP generation transport recovery is scoped to the candidate and upstream origin.
A connection-level HTTP/2 error or interrupted TCP/TLS connection retires that pool
for future requests. Two failed pool generations within two minutes temporarily
select HTTP/1.1 for two minutes, then negotiate normally using a fresh pool.
Concurrent errors from one retired generation count once; late errors cannot
replace a newer pool. Recovery retains configured proxies and TLS validation,
does not restart active streams, and never replays the failed request. Stream-local
resets, client cancellation, provider HTTP errors, and response deadlines do not
trigger recovery. Observations are bounded to 256 routes and expire after ten idle
minutes once no request holds that pool generation. Quiet active requests retain
their recovery owner; HTTP/3 probing is deferred during HTTP/1.1 recovery.

WebSocket writes, including control frames and HTTP fallback output, have a
60-second per-message deadline. This local I/O limit does not time out model
generation. A stalled write ends the affected connection; uncertain upstream
execution is not replayed. Reused connections register the next attempt before
writing so send failures retain request diagnostics and release the lease.

Native ChatGPT turn state is scoped to the key, session, account, model, endpoint,
credential, and authorization incarnation. Client-supplied state cannot establish
that provenance. Soft root/thread and prompt-cache hints are separately bounded
and namespaced; they neither authorize state nor transfer an opaque owner.

A generic HTTP 400 is terminal, not evidence for rotation or history repair.
Buffered local errors derived from upstream adapter output carry
`x-should-retry: false` without changing their status or accounting, so clients
that honor this header do not regenerate automatically. This cannot prevent
an explicit resend or retries by clients that ignore the header.
Repair needs a specific validated response-reference/tool-link rejection.
`turn_rate_limit_exceeded` ends the current turn with the known retry delay,
without account-wide cooldown or automatic rotation.
An upstream `429 rate_limit_exceeded` about TPM remains a rate-limit refusal,
even when its message says `Request too large`; that text alone does not prove
a terminal 413 or context overflow. Existing retry and ownership guards still apply.

For ordinary API-key sources only, a relayed ChatGPT `usage_limit_reached` or
`subscription_sharing_usage_limit_exceeded` does not establish that key's own
subscription window. Its reset cannot impose an account-wide cooldown: apply
short model-scoped backoff, preserving explicit `Retry-After`/`retry_after` and
configured source recovery delays. Native account deadlines and other providers'
quota errors keep their existing scope and resets; no stored cooldown is cleared.
The official [rate-limit guide](https://developers.openai.com/api/docs/guides/rate-limits)
defines rate headers, while the [reasoning guide](https://developers.openai.com/api/docs/guides/reasoning)
requires preserving opaque reasoning history. Diagnostic-based cleanup is a
bounded Relay compatibility policy, not a provider promise of portable ciphertext.

Recognized Cloudflare 1010/browser-signature blocks do not mutate account
authorization state; ordinary route backoff remains. Structured recognition is
bounded to error wrappers, not echoed input. An exact compatible-relay report of
an internal WebSocket handshake rejected with 403 has its own terminal transport
category: no credential penalty, automatic replay or capability/HTTP downgrade.
For a validated simple JSON error followed by SSE, only the first error owns
classification. Duplicate/unknown error fields and malformed tails do not enter
that compatibility path; genuine account/workspace denials retain their policy.

BPS generation rate limits cool only the rejected model unless the provider
explicitly reports an account-wide allowance or quota limit. Millisecond retry
hints retain a five-second minimum pause. Explicit organization/model TPM or
RPM errors establish an opaque quota-group identity, scoped to the upstream
endpoint. Only members that independently reported that group share its pause;
unknown accounts, other organizations, models and native connections remain
independent. Repeated rejection after a shared pause grows backoff from five to
sixty seconds without shortening provider deadlines. Successful recovery resets
backoff, not a newer active cooldown. Group observations are memory-only, bounded
to 4096 entries and ten minutes, and invalidated on removal or permission change.

After a proven pre-execution BPS rate-limit refusal, further dispatches and
recovery waits for that logical request share a monotonic deadline: the configured
recovery window, capped at thirty seconds, even in persistent-wait mode. This
does not interrupt an accepted generation, shorten provider cooldowns, or change
quota/auth policy. An independent ready member may still recover the request
within the existing dispatch budget. Exhaustion preserves the rate-limit refusal.

HTTP conversation telemetry retains physical-send latency, TTFT and consumption
separately from logical execution timing (including preparation and recovery
waits). Request-loop exit finalizes a pending retry observation without adding a
send, charging usage twice, or repeating account/health observations. Streaming
completion and client disconnection retain their existing owners. Legacy rows
without request timing are not backfilled from inferred timestamps. BPS adapter
errors preserve allowlisted validation parameters and bounded JSON coordinates;
request bodies and tool argument values are never included.
Local adapter rejections retain the same safe error details in HTTP and WebSocket
history, with Relay origin and no invented provider attempt, usage or cooldown.
Client cancellation has its own non-successful UI outcome. Historical synthetic
HTTP 502 cancellation rows receive the same label.
New client-cancellation telemetry uses HTTP 499 for both HTTP and WebSocket
requests, preserving reported consumption and the client-disconnected outcome.
A logical cancellation does not inherit the previous attempt's provider error;
that rejection remains on its physical attempt, with consumption unchanged.

Relay owns HTTP protocol retries, with reqwest's internal retries disabled.
HTTP/2 refusals follow the configured failure policy, candidate limit, provider
cooldowns and uncertain-execution rules.

Native generation HTTP dispatches can opt into
`ZENITH_RELAY_HTTP_HEADERS_TIMEOUT_SECONDS` (1–1800; unset, zero or invalid disables
it). The request budget retains one monotonic deadline across authorization
repair, account changes and HTTP handoff; later attempts cannot extend it.
This covers connection, upload and waiting for response headers, not an isolated
upload-completion measurement. Receiving headers releases the guard; response
bodies and long SSE generation retain their existing behavior. Expiry returns
`http_headers_timeout` with HTTP 504 and Relay origin, does not cool the account,
and cannot authorize replay of possibly accepted work. Expired budgets do not
start another dispatch. BPS progress guards and native WebSocket are separate.

Routing JSON optionally retains the bounded model ID declared by the raw
upstream response before protocol or BPS translation. Request details distinguish
requested, routed and declared models. The declaration is not independent model
verification, never changes routing or pricing, and is absent in older records.
Frozen upstream sessions (`session_blocked_by_cyber_policy`) are terminal policy
refusals, not an account-wide credential failure or a reason to rotate.
Encrypted-history repair refuses ambiguous multiple reasoning ciphertexts and
encrypted compaction checkpoints without modifying history.

A structured validation refusal that the service does not retain past Responses
uses the existing missing-response recovery, including terminal SSE errors.
Only a complete local replay owned by the requesting key and original route may
repair that continuation before client output. Generic unsupported parameters
or ambiguous execution do not establish a safe replay.

Errors retain origin (`Relay`, `Account`, `Provider`), category, status, timings,
and safe provider IDs. Prompts, raw bodies, arguments, credentials, and
authorization headers do not enter diagnostics. Stable codes live in
`error_codes.rs` and both localized Help tables.
Adapter diagnostics retain static field names, value types and object-field counts,
never arbitrary payload keys or values. Known output-limit/filter terminals remain
incomplete responses with their usage, even when no visible output was generated.

## Protocol adapters

### Shared conversion architecture

Four wire contracts: Responses, Chat Completions, Anthropic Messages, and Gemini
Generate Content. Matching contracts use native forwarding. The twelve converted
directions use one typed pipeline:

`client codec → translation/model.rs → upstream codec`, with the inverse return path.

Gemini Interactions is an additional **native-only** API-source contract at
`POST /v1beta/interactions`, using `gemini_interactions` and Google API-key
authentication. Configure an explicit Interactions source/endpoint; Generate
Content discovery does not grant this protocol. Model JSON/SSE, original tools,
opaque state and request extensions stay native. Cross-protocol conversion,
agents, background execution and remote retrieval/cancellation are not provided.
`previous_interaction_id` requires a locally recorded owner bound to the caller,
model and exact source/authorization. It cannot fall back to another account or
source, and unknown state is rejected. Lifecycle-only SSE is not generation;
completion requires a valid terminal, with known partial usage retained on failure.

Dispatch is in `protocol/adapter/contracts/source.rs`; preparation distinguishes
`PreparedAdapterRequest::Native` from `Translated`. Portable content is separate
from opaque provider state in `AdapterContinuationState` and the bounded volatile
`AdapterContinuationStore`. No pair-specific runtime or second replay cache is needed.

Native JSON retains object order and arbitrary-precision numbers, not original
whitespace/escaping. Eligible repeated headers retain their bytes; credentials,
hop-by-hop fields, and ambiguous state headers are excluded. Native SSE/WS frames
retain wire bytes except explicit terminal validation, bounded-frame rejection,
and safe error rewriting. The first valid terminal ends consumption without
waiting for EOF or trailing data.

Relay bounds each incoming SSE event and the buffered pre-output prefix to
64 MiB. This is a local memory safeguard, not a provider limit or a cap on total
response length. Coalesced events share no total stream budget. Oversized events
fail explicitly, retain known usage, and do not authorize replay of uncertain
execution or penalize account availability. BPS keeps its separate 16 MiB event
limit.

Shared SSE parsing preserves
[event-stream framing](https://html.spec.whatwg.org/multipage/server-sent-events.html#parsing-an-event-stream):
CR/LF/CRLF, blank-line dispatch, fragmented/coalesced input and per-event limits.
Native forwarding retains byte-exact frames; each protocol owns event and terminal
validation. Responses text/tool deltas still require validation, while usage,
model identity, terminal events and opaque compaction retain their full checks.

Native WebSocket forwarding checks message size, served model, late events,
usage/replay and safe errors. Committed output disables inapplicable retry parsing,
not served-model validation. Converted text, reasoning, refusal and argument
snapshots change only after append-only/identity checks and successful emission.
Closed Messages blocks reject mutations; deferred Gemini and tool-search calls
stay behind their output barriers. Numeric normalization preserves exact values
and does not release unfinished arguments. Service tiers, effort and cancellation
do not change as a side effect of parsing optimizations.
The ChatGPT WebSocket setting controls client ingress and the managed profile,
not provider transport. Catalog changes do not override it. BPS and converted
routes use the HTTP executor behind the same client socket; usage records the
client transport. A failed client restart does not roll back a saved setting.
`routing.transport` separately records the executor's upstream HTTP/WS boundary,
native/conversion/BPS mode, HTTP-bridge reason, and observed response format.
This is dispatch provenance, not proof of provider acceptance or execution.
HTTP content type is taken from the response.
WS handshake time covers only the upgrade attempt, not auth preparation, queueing,
or content TTFT; a reused socket and older records leave it absent, not zero.
These fields share existing routing JSON storage and add no retry or transport policy.
Optional `codexServerTiming` records allowlisted numeric native
`responsesapi.websocket_timing` values separately from Relay timings: backend
overhead, inference, engine TTFT and inter-token intervals. They are not additive
phases, cache-hit evidence or end-user latency. Missing/invalid measurements stay
absent. Only an active response may receive a measurement; reused connections
require an exact response ID, while an ID-less first-response event is accepted.
Unmatched and late events never attach to the next response. Raw events and IDs
are not retained in this diagnostic object. BPS does not synthesize these values.
Messages streams finalize native text/thinking snapshots when their block closes
and append signature fragments in place. Portable deltas still stream immediately;
only a fully closed response can supply replayable native continuation state.
Converted Messages tool arguments may stream incrementally, but their block-stop
is withheld until the complete upstream response passes validation. A failed
terminal cannot release an executable tool call or establish continuation state.
BPS search progress alone does not mark its output item as delivered; a terminal-only
search item is materialized before the shared protocol bridge validates completion.

An output limit may truncate a tool's JSON object. Responses and Chat clients
retain that incomplete argument string, the limit reason, and known usage;
Responses marks the function-call item incomplete. Only an end-of-input parse
failure in empty input or an object prefix is accepted, never malformed completed
calls or non-object input. Object-valued Messages/Gemini output and custom/tool-search
conversions still reject arguments they cannot represent. Incomplete turns have
no reusable continuation or history key, and Relay never closes the JSON for them.

Converted Chat Completions streams validate `stream_options.include_usage`.
Only an explicit `true` adds `usage: null` to ordinary chunks and a final
usage-only chunk. Upstream usage collection is independent of this client flag.
Native usage capture ignores data after its terminal marker, including later
usage, tier, and model changes. Gemini candidate endings do not end capture:
its separate trailing usage remains observable until the stream ends.
Chat and Messages tool choices are validated using their own wire shapes and
per-variant fields. Messages `none` carries no parallel-use modifier: no tools
can execute, so a separate parallel restriction is redundant. Native forwarding
keeps its existing extension policy; Responses selectors use their own resolver.
Responses custom-input deltas and missing suffixes in final text snapshots append
in place after the existing identity, prefix, and closed-block checks.

Completed streams move existing history and native continuation state into the
saved checkpoint without cloning the entire prepared request. Validation and
response encoding precede the transfer; final client emission keeps its checks.
The validated checkpoint is saved before final tool/terminal frames reach the
client, so an immediate tool-result request cannot outrun continuation storage.
Usage and lease settlement remain tied to client-stream completion.
Terminal native output moves into the checkpoint without copying its opaque
payload. Tool-argument normalization borrows unchanged input or an unchanged
buffered prefix; its owning string is retained/truncated in place, and a new
output buffer is allocated only when numeric spelling must change.
Within quoted JSON strings, normalization uses vectorized quote/backslash search
instead of visiting every payload byte. Existing escape handling, exact numeric
bounds and withholding of unfinished numbers remain unchanged. Differential
tests compare Unicode prefixes, escaped strings and malformed fragments against
the previous scalar scanner; final JSON validation stays with its existing owner.
The streaming gateway takes the completed translated response by ownership,
without cloning its body or continuation. Taking the result does not reopen a
terminal bridge or repeat its output.
Gemini native Parts retain their original order and signature/thought boundaries.

Responses stream interruptions emit one explicit `response.failed` at a safe
SSE boundary, even after visible output. An already forwarded partial frame
cannot be replaced or have another event inserted into it. Interrupted streams
retain known usage, never fabricate `response.completed`, and do not authorize
replaying an execution with unknown effects.
Transport diagnostics retain fixed timeout/body/decode and I/O categories, not
error strings, URLs, credentials, or provider payloads.

Every meaningful converted field is represented, handled locally under a stated
contract, or rejected. Conversion does not invent capabilities, weaken a schema,
disable reasoning, or turn unsupported controls into prompt text. Official
references and the [conversation acceptance matrix](../../.agents/skills/relay-protocol-adapters/references/acceptance.md)
own field-level review; third-party proxies are comparisons, not specifications.

Completed Responses items remain immutable between `output_item.done` and the
terminal snapshot. Only documented nullable fields may alternate between omission
and `null`: message `phase`, reasoning `encrypted_content`, and tool `caller`.
Completed reasoning's string `encrypted_content` comes from
`response.output_item.done`, as required for replay by the official Responses SDK.
Before comparing the terminal snapshot, retain that exact opaque value even if
the terminal omits, nulls or replaces its copy. Invalid terminal ciphertext types
still fail. Other non-null values, content, identities, arguments, and lifecycle
states must match. Without a completed reasoning item, the terminal remains the
source of continuation state. Mismatch diagnostics name only fixed protocol
fields, never provider keys or values.

### Conversation and routing contracts

- Messages `metadata.user_id` is opaque user attribution, not prompt content.
  Gemini conversions send its stable SHA-256 pseudonym (first 63 hexadecimal
  characters) as the documented `labels.safety_identifier`; null remains absent
  and an empty ID remains empty. Native requests stay unchanged. Arbitrary
  OpenAI metadata is not safety attribution and still requires a compatible
  endpoint rather than being discarded or relabeled.
- Responses client-executed tool search keeps its call/result kind, IDs,
  namespaces and original catalog across JSON/SSE and both continuation modes.
  Foreign models receive a callable search function; the client still executes
  the lookup and receives native `tool_search_call` items. Deferred functions
  become callable only after discovery or an explicit choice. Historical calls
  remain replayable but cannot reactivate a tool removed from discovery results.
  Exact schemas stay in the catalog/checkpoint; foreign search results carry
  loaded names rather than a second schema copy. MCP, file search and code
  execution remain native service features, not fake client functions.
- Converted Responses client tool-search items use `tsc_` item IDs, independently
  of their `call_id`. A precise pre-execution input-ID rejection can normalize
  legacy search item IDs once, updating item references without touching call
  links or opaque reasoning. Existing IDs are reserved to avoid collisions.
- Native Messages preserves inline tool additions/removals, their ordered system
  messages and the original tool catalog. On converted Responses/Chat routes,
  Relay validates and applies client-owned changes to the final callable catalog,
  preserving exact historical declarations as ordered system text. This is local
  tool-availability lowering, not native inline-tool support or native cache
  preservation. Schemas and forced choices remain enforced; unknown references,
  MCP changes and incompatible late-system targets fail explicitly.
  Contract: [Messages inline tools](https://platform.claude.com/docs/en/build-with-claude/mid-conversation-system-messages).
- Responses `web_search` / `web_search_2025_08_26` can use the native Messages
  `web_search_20250305` tool. JSON and SSE retain search IDs, queries, URL sources
  and URL citations, including mixed hosted searches and client functions.
  A citation covers its Messages text block; `cited_text` is a source quotation,
  not an offset into the answer. Original results, encrypted handles and citation
  blocks replay through the existing route-scoped continuation, for both
  `previous_response_id` and full history. Public URLs cannot reconstruct missing
  native state. Domain allowlists and approximate user location are mapped;
  unsupported controls, non-direct callers, failed/incomplete results and other
  server tools fail explicitly. Gemini/Chat search conversion remains unsupported.
  This bridge does not emulate `pause_turn` (see below). Native routes are unchanged.
  Endpoint references: [Responses web search](https://developers.openai.com/api/docs/guides/tools-web-search)
  and [Messages web search](https://platform.claude.com/docs/en/agents-and-tools/tool-use/web-search-tool).
- `POST /v1/messages/count_tokens` and Gemini
  `/v1beta/models/{model}:countTokens` (also `/v1/models/...`) forward to native
  API sources only, under existing key, model and source permissions. They do not
  generate a reply, estimate tokens locally, convert between tokenizers, use
  account transports or record generation usage/health success. Request bodies
  retain existing gateway bounds; upstream execution is bounded to 30 seconds
  and count responses to 256 KiB. Missing/invalid counts fail, never become zero;
  upstream error bodies are not exposed. Custom API prefixes and source-model
  aliases are retained. The maintained official references are the
  [Messages token-count API](https://platform.claude.com/docs/en/api/messages/count_tokens)
  and [Gemini token-count API](https://ai.google.dev/api/tokens).
- Base64 PDF inputs preserve bytes and labels across the twelve conversions.
  Responses/Messages also exchange HTTPS PDF URLs without Relay fetching them
  and preserve PDF tool results. Chat/Gemini URL conversion, provider file IDs,
  document citations/context and non-default rendering controls require a
  compatible native endpoint rather than lossy extraction or invented metadata.
- Native Gemini `CONTINUATION` is an incomplete generation with a required opaque
  candidate token. JSON/SSE retain it and trailing usage. Its route owner is
  bound through the existing affinity store to the local key, model and exact
  original `contents`; unknown/changed contexts cannot rotate to another owner.
  Relay never resumes a generation automatically. `cachedContent` stays native
  and provider-owned; cache creation/expiry are not emulated by conversion.
- Messages `pause_turn` means an unfinished server-tool loop, not token
  exhaustion. Converted routes reject it with `stop_reason` rather than invent
  a token-limit result; matching native routes keep their continuation contract.
- Converted JSON responses reject supplied non-assistant Responses output roles
  and invalid/nonzero Chat choice indexes instead of relabeling their identity.
- Converted Messages replies validate supplied assistant roles in JSON and SSE.
  Chat history accepts both the `refusal` field and refusal content parts,
  preserving the text once and rejecting conflicts or non-assistant refusals.
  Responses-to-Chat history retains refusal parts instead of ordinary text.
- Historical Chat `function` results pair by function name, not an invented call
  ID. Ordinary Chat participant names have no lossless mapping here and fail
  with `messages[].name`; matching native requests retain them unchanged.
- Converted Chat/Messages terminal reasons must agree with their calls: a tool
  stop needs a tool call, while ordinary completion cannot contain one. The
  shared JSON/SSE completion boundary rejects contradictions with `finish_reason`
  before publishing a successful terminal or reusable continuation.
- Required or named tool choices need an actual call before a converted reply
  can complete normally. A plain-text success is rejected with `tool_choice`;
  refusals, content filtering and token-limit outcomes retain their own meaning.
- Preserve instruction/block order and protocol-specific roles. Responses/Chat
  conversion retains distinct `system` and `developer` roles in their original
  positions, including continuation identity. For targets with a top-level
  system field, initial system/developer attachments before the first
  assistant turn join that field in instruction order; user content order stays
  unchanged. Do not hoist instructions past an earlier assistant turn.
  Nullable aliases cannot erase explicit limits;
  conflicting non-null limits fail before transport.
- Preserve call kind, namespace, declared name, item ID versus `call_id`,
  parallel grouping, arguments, and results. Only argument deltas concatenate;
  repeated identity fields do not. Tool names retain their declared identity;
  new calls require distinct nonblank call IDs, preserved without normalization.
  The shared conversion boundary rejects whitespace-only IDs in history, JSON,
  and streams. Malformed Chat call lists, mixed legacy/current call fields, and
  calls attached to tool-result messages fail instead of being silently discarded.
  Fresh calls to the same tool, including repeated stateful observations, are
  not suppressed by name or argument equality; replaying an old call is distinct.
- An explicit client serial-call policy remains binding. HTTP, compaction and
  WebSocket default to full Responses regardless of native catalog Lite flags.
  Client-facing catalogs and managed Codex profiles publish
  `use_responses_lite: false`, including compatible cached rows; original account
  manifests stay unchanged. An upstream Lite preference does not disable full
  Responses parallel calls: the owning native card must still confirm parallel
  support, and explicit reference exclusions remain binding.
  [Codex selects its request format from this flag](https://github.com/openai/codex/blob/5b792948f9414288b9196ae0559f218439affe20/codex-rs/core/src/client.rs#L1016-L1118).
  Only an explicit client Lite header or WebSocket metadata selects Lite; its
  omitted parallel policy defaults to serial after route selection, and a
  conflicting parallel policy fails before dispatch. WebSocket reuse includes
  the negotiated Lite/full mode; a changed mode reconnects before the next turn.
  Its marker does not serialize BPS or API-source calls. Ordinary tool support
  does not establish parallel support. Native catalog transport exclusions and
  reference exclusions win.
- Responses custom outputs accept strings and typed content lists, including
  representable images. Empty lists mean no content, not literal `[]`. Untyped
  custom results fail. Chat tool messages cannot carry images.
- Function and custom-tool output text stays literal when converted to Gemini:
  JSON-looking stdout is wrapped under `output`, or `error` for an explicitly
  failed Messages result. A source-file `error` key does not change execution
  status, and whitespace, duplicate JSON keys and numeric spellings survive.
  Native Gemini structured results and Relay-owned discovery remain structured,
  following the [FunctionResponse contract](https://github.com/googleapis/googleapis/blob/bc7e3baa28fbb223fa93782e130260fab8205bfc/google/ai/generativelanguage/v1beta/content.proto).
- Responses function-result names, when supplied, must match their recorded call; a
  name or item ID cannot replace `call_id`. Standalone outputs without a call
  have no foreign tool-history mapping and fail with `input.call_id`; matching
  native Responses input is preserved, not converted to invented calls or text.
- Equivalent `caller: direct` / `async: false` history is accepted. Async/program
  execution and an unexpected upstream namespace cannot silently become direct
  calls. Native Responses retains those fields unchanged.
- Schemas retain constraints. Gemini's OpenAPI and JSON Schema alternatives are
  mutually exclusive; Part payloads and supplied IDs/arguments are validated.
  Unsupported ordering/name restrictions fail instead of disappearing.
  Converted foreign tools with omitted `strict` explicitly use `strict: false`
  on Responses so its automatic strict normalization cannot change optional
  fields or object openness. Explicit booleans and schema contents are preserved;
  native Responses keeps its documented normalization default.
- Known model exclusions for temperature use the client's wire location,
  including Gemini `generationConfig.temperature`, before route admission;
  absent/null values do not request a sampling override.
- Missing reasoning settings keep provider defaults. Effort/budget rendering
  uses model evidence; a budget-based model needs a budget. Messages requires an
  output limit: an omitted limit defaults to at most 8,192, capped by a known
  model limit. Explicit incompatible limits fail rather than being rewritten.
- Provider ciphertext, Messages signatures/redacted blocks, and Gemini thought
  signatures are replayed unchanged only to their compatible owner. Public
  summaries are not signatures. Missing opaque state cannot be reconstructed
  from visible reasoning or an invented sentinel.
- Image history fingerprints retain non-default `detail`. Omitted detail and
  `auto` share the documented default; other changes cannot reuse an older
  native checkpoint. Wrong detail types and misplaced controls fail before
  conversion instead of silently becoming the default.
- Output formats validate fields by variant. Text/JSON-object modes cannot
  silently discard schema controls; nested and flat schema envelopes cannot
  compete. Invalid schema names and Gemini MIME types fail before conversion.
  Absent/null optional controls retain their defaults. Messages/Gemini encoding
  does not build an unused OpenAI-format copy of the response schema.
- Chat plaintext `reasoning_content` / `reasoning` aliases must agree. Messages
  clients may replay translated summaries with absent/null/empty signatures;
  nonempty foreign signatures fail. A new ordinary user turn can carry completed
  portable history, not an unfinished foreign tool loop. Without a native
  checkpoint, unsigned Messages summaries before that new user turn become
  visible assistant text on Responses/Gemini; they are never fabricated native
  reasoning. An available checkpoint still restores its original opaque state.
- Preserve supported URL citations and Unicode offsets. Unsupported citations
  or meaningful output extensions fail rather than disappearing.
- Metadata and advisory cache hints follow the target contract. Valid foreign
  cache hints do not promise equivalent TTLs, hits, or pricing. Required context
  edits and unsupported hosted tools are not advisory and must fail.
- JSON, gzip, and zstd share request decoding. Compressed bodies and identity
  JSON above 64 KiB are decoded and parsed in blocking jobs bounded by available
  CPUs; small identity JSON stays inline. Cancellation retains the worker permit
  until parsing finishes. Encoding errors, field order and exact numbers are preserved.
  Completed, Incomplete, and Invalid remain separate across JSON and streaming. SSE/WS lifecycle and
  terminal status must agree; EOF or bare `[DONE]` is not proof of completion.
- Incomplete/refused/invalid responses retain known usage but do not establish
  reusable converted continuation or successful route health. Temporary native
  WS continuation is limited to the same live connection and owner.

Gemini conversion supports one candidate and drains late usage after a finish
reason under a deadline. The native WS bridge supports one response and at most
one named lane per connection; full multi-lane multiplexing remains open.
HTTP-only transport observations are bounded, expiring, and credential/route-scoped.

### Excel / Basis Points

BPS has a dedicated [executor contract](BASIS_POINTS.md): access and request
context, executable-call validation, diagnostics, batch replay, catalog discovery,
cache boundaries, attachments, and capability limits. Read it when changing that
executor; shared protocol and runtime contracts remain in this document.

## Request timezone

Timezone rewriting uses the assigned proxy's validated, observed IANA zone
automatically. Startup removes the retired `requestTimezone` settings from
saved gateway state without changing other controls. Unverified legacy proxy
zones are discarded; older successful IP checks can supply observation evidence.

Proxy observations join the shared bounded refresh service as Metadata jobs.
Startup, import and configuration events register them without mounting a view.
Manual checks join the same single-flight resource. Good observations expire
after 15 minutes, with a refresh scheduled 30 seconds before expiry; failures
and missing timezone evidence retry no faster than one minute. Retry floors,
background-session pause and proxy identity/result ordering fences still apply.
Expiry is checked in the request path without network or disk I/O; unknown or
expired evidence preserves the client's timezone, never guesses a region.
Backend events coalesce redacted proxy-summary reads in visible views, not probe
requests; a newer saved check replaces an older manual result in the UI.
The HTTPS exit-IP probe
uses `https://www.cloudflare.com/cdn-cgi/trace` through the selected proxy without
account credentials, redirects, or a direct-network fallback, bounded to 12 seconds
including HTTP admission. Its displayed duration excludes optional geolocation
and is labeled as an HTTPS check, not ICMP or provider latency.

After primary success, a best-effort probe checks the other IP family through
the same proxy using `api.ipify.org` or `api6.ipify.org`. Its address and geography
share a 3-second budget and do not change the displayed primary duration.
An unavailable second family does not fail a working proxy. Conflicting countries,
regions or known timezones clear the single region/city and automatic timezone;
conflicting countries also clear the country. Both addresses remain in details.

Exit country is taken from the same Cloudflare trace as the observed exit IP.
It remains available if optional city/timezone discovery fails. No local geography
database, file import, license key, or user setup is required. The bounded optional
lookup uses `https://ipwho.is/{observed-ip}` through the selected proxy, with only
IP, country, region, city, and `timezone.id` fields and a separate 3-second budget.
The returned IP must match the original exit; a conflicting country discards city
and timezone rather than mixing locations. This also handles rotating proxies.
The proxy gateway hostname and login selectors are never treated as observed
geography. Cards show only exit location, IP, and HTTPS duration; timezone and
lookup details are in the diagnostic tooltip. Only a valid IANA identifier is
stored, never a guess from a country,
IP prefix, or numeric UTC offset. A failed check retains the known zone; a
confirmed changed exit without a valid zone clears stale state. Proxy identity
and check timestamps fence stale results. Persistence precedes a hot update of
the running bound accounts, including common-proxy inheritance; the scheduler,
listener, and applications are not restarted. There is no renderer polling or
per-generation geolocation request.

Proxy cards also expose an explicit speed-test icon between the connectivity and
account actions in the unchanged bottom action strip. The middle uses a divided,
centered two-by-two grid: location and HTTPS-check time above, visible speed and
an account icon/count below. Diagnostic IPs, check time, and failure details stay
in hover/focus tooltips. Optional proxy names persist with the stored entry;
renaming never changes its identity, endpoint, or assignments.
The native command downloads a fixed 5 MiB Cloudflare sample through the selected
stored proxy, without account credentials, redirects, or direct fallback. One
measurement runs at a time; concurrent requests wait in FIFO order instead of
failing as busy. Cards show queued/measuring state through an operation-scoped
channel. Queue waiting does not consume the measurement's 12-second budget,
which includes HTTP admission and connection setup after its turn starts.
Only a complete uncompressed binary sample yields payload-transfer Mbps; failed
or incomplete samples never become zero-speed results. Measurements are transient
and do not modify proxy assignments, geography, usage billing, or model requests.

Only existing unambiguous timezone fields in user `environment_context` and
web-search location fields change. Ordinary text, dates, non-user messages,
missing fields, OS settings, and API-source bodies stay unchanged. Every attempt
uses its selected account's current proxy evidence. Updates are fenced and do not
rebuild the scheduler.

## Tool catalog

Native routes forward the client's catalog without local relevance search,
hidden tools, or legacy optimization flags. Catalog telemetry grants no permission.

Tool-search diagnostics reflect an explicitly supplied search control together
with deferred definitions, not an inferred provider capability. A text answer
without a tool call is normal for `auto` or `none`, not a missing-tool warning.
BPS renders shared invocation/schema guidance once and indexes merged tool
definitions without changing current-over-history precedence or schema contents.
Its compact/discovery presentation and bounds are owned by the BPS executor
contract above, not by native-route relevance filtering.

Converted Responses catalogs use explicit root declarations as current execution
permissions, including an empty array. Historical discoveries and call/result
pairs remain replayable but cannot restore omitted tools or namespace children.
Without a root catalog, declarations remain incremental. Nested namespaces have
reversible opaque aliases and retain descriptions. Custom tools use one `input`
string. Unsupported hosted or client-search execution fails explicitly. Only
direct client execution is representable; empty/non-direct caller policies are
not widened.

Forced/allowed tool choices cannot enable undeclared tools or broaden a restricted
set. Choices reset per request; a continuation omitting the catalog retains its
definitions. Historical call kind/namespace remains independent of later catalogs.

Messages client-owned discovery validates the complete catalog, then exposes
non-deferred, explicitly referenced, historically called, or forced tools.
References must resolve to root definitions in successful paired user results.
Relay supplies exact schemas once and preserves the discovery marker; it does
not search, execute tools, emulate hosted search, or promise foreign cache placement.

Parallel calls stay in one assistant turn. Every result must match an outstanding
call before ordinary user text or the next model turn. Unknown/duplicate/missing
results fail. A disabled parallel policy rejects a second generated call, including
on protocols where this must be enforced locally.

Tool results preserve text, supported media, and error meaning. Gemini IDs are
mapped separately from client IDs, and signatures remain on original Parts.
Custom input is unwrapped once; exact integral function arguments may be rendered
as integer syntax without changing fractional, string, negative-zero, or large values.

## Usage and prices

### Diagnostic collection

Detailed request telemetry is opt-in through the existing Desktop debug mode.
Without it, request context/schema byte counting, cache-context fingerprints
and comparisons, BPS catalog size diagnostics, and preparation-stage clocks
are skipped. The runtime reads the host's
setting without rebuilding the listener; admitted tool-policy captures retain
their original mode. The UI hides route, context, and tool diagnostic panels
outside debug mode, including those in historical rows; history is not erased.
Token/cache usage, pricing, total latency, TTFT/generation speed, outcomes,
attempt history, errors, and all authorization/schema/size guards stay active.
No debug setting changes routing, provider caching, or tool execution policy.

New request diagnostics retain only JSON byte counts for client and prepared
instructions, conversation, tool catalog, and whole body, across all four APIs.
The counts measure serialized JSON bytes; provider-injected context is outside
their scope.
BPS measures the final dispatched body, including its embedded catalog after
preloading/fallback.
Cached tokens still occupy context; never subtract them from the displayed
window or remove valid instructions/tools merely to reduce the counter.

### Request records and retention

Usage retains one final result per logical client request and one consumption
record per physical send (`requestId`, `attempt`). The Usage list shows sends
directly, including retries and tool discovery, with each send's observed tokens
and API equivalent. Repeated observations replace the same send; late corrections
do not replace the logical outcome. Summary request counts, success and timing
use logical results; consumption, pricing, model/member attribution and quota
equivalents use all sends. Attempt elapsed times are never added together.
Schema 36 adds retained earlier sends to existing rollups once, preserving archived
aggregates. Missing historical sends or unknown usage are not reconstructed.
Request filter choices come from distinct values in retained physical-send
history within the selected period, not the current page or a fixed list.
Requested/routed models, pool members, protocols, transports and error categories
share this history projection across report tabs. Current inventory supplies
labels/catalog ordering, not extra filter values; retained removed members stay
selectable. Selections do not hide alternatives, and expanding the advanced panel
does not issue another report query. Period and outcome choices remain fixed
query modes.
Older Desktop hosts that omit model/member filter fields use an unfiltered,
events-free aggregate read for the same period. This compatibility path does
not substitute current inventory or one page of events; supported empty arrays
remain empty, and concurrent fallback reads for the same period join in flight.
Desktop reports run on a blocking worker. A read-only WAL connection pins one
snapshot/history revision under a short writer lock, then computes without holding
up request/state writes. Its 24 MiB pager and the writer's 8 MiB pager share a
32 MiB budget; in-memory tests retain one connection. Pagination decodes only
selected rows; aggregates and filter choices use narrow indexed projections.

Logical totals/timings remain separate from physical-send consumption/pricing.
Price groups preserve member/model/tier/context boundaries; bucket rounding is
separate from report totals. Every path keeps the `(requestId, attempt)` legacy
exclusion. Account quota windows use the same read snapshot and pager budget,
with pricing after snapshot release. Cache publication retains the revision
captured before reading, so concurrent writes cannot relabel stale results or
mix totals from different snapshots. Append-only index migrations do not rewrite
retained requests or sends.
Deleting/expiring usage removes corresponding attempts; missing earlier history
is not reconstructed. Actual retry/stop decisions are recorded when known.

Retention captures at most 256 oldest expired request IDs in one transaction;
archiving physical sends and deleting history use exactly that captured batch.
Startup, every 256 new requests, and a once-per-minute Desktop worker use bounded
batches. The worker skips a busy writer; aggregate
totals survive expiration and caches are invalidated after a committed batch.

Usage corrections and mixed state/purge batches are atomic with WAL/FULL durability.
Unchanged values can skip redundant updates, never request/attempt observations
or account purges. A bounded eight-statement cache excludes dynamic report queries.
Per-item inventory observations persist changed accounts/sources/keys and automation
state in one transaction before publishing memory. Full inventory replacement
merges and clears these observations atomically, preventing removal, reorder or
reimport from reviving older records. Registration revisions and usage identity
fences remain authoritative when notifying eligibility changes.

### Diagnostic exports

Desktop request details can explicitly save a privacy-safe diagnostic JSON file.
On explicit export only, `reasoningLatencyComparison` compares successful
single-send observations on the same recorded key/source/candidate/account,
requested/resolved model, protocol, client/upstream transport, execution mode,
and requested/observed service tier. The read examines at most the 1,000 logical
request records at or before the selected record and excludes observations more
than seven days older. It neither scans all retained history nor runs on page
entry, and does not send model requests or change reasoning defaults.
Groups use the recorded effective reasoning level; unknown settings and missing
TTFT stay excluded, not zero. Counts, nearest-rank p50, p95 (20+ samples), input
size and observed cache fractions expose sample limitations. Two groups with
five samples each are only a display threshold, not statistical significance.
This is observational evidence, not a controlled same-prompt experiment or an
inventory of currently available levels. Prompt, context, cache and provider-load
differences remain confounders; no automatic quality/latency recommendation is made.
Schema 3 includes a summary of the latest outcome, step counts, reported
timing and observed token consumption across retained physical sends. The final
request projection is not added again when its attempt is already present.
Missing counters remain null, and each subtotal reports its observation coverage;
missing/truncated history never claims complete consumption. Intermediate,
retrying, failed, disconnected, uncertain and conflicting outcomes are distinct.
The allowlist includes up to 64 newest attempt observations, numeric usage,
known Relay error codes, protocol classifications, tool/context/catalog sizes,
routing classifications, cache comparisons and transport timings. Arbitrary
provider error text and unrecognized service-tier text are not exported; the
latter retains only a report-local reference. References are salted per report.
Prompts, tool arguments, provider messages, credentials, URLs, raw identifiers
and configuration are excluded. Reports are capped at 256 KiB, mark
missing/truncated history, and drop older detailed observations before optional
final routing details. The summary still covers all retained sends read before
this detail-size limit, explicitly noting when detailed rows have been omitted.
No raw provider-usage record is invented or persisted for this export.
`observedAt` is the database observation/update timestamp in UTC, not request
start time. Tool call counts before and after client translation are separate;
batch expansion can make them differ without indicating a failure.
The export names the pre-transport tool-policy snapshot `tools.policyCatalog`,
not `forwarded*`: BPS embeds a catalog index and loaded contracts as prompt text.
`renderedSchemaBytes` counts parameter-schema text, not all prompt/wire bytes;
zero native `upstreamContext.toolCatalogBytes` is therefore compatible with
loaded prompt contracts. Transport `preparationAccounting` subtracts only observed
non-overlapping body/catalog/attachment phases from the elapsed preparation clock.
Its unattributed remainder includes unmeasured setup and rounding, not another
addition of nested schema validation; missing or inconsistent evidence stays explicit.
`relayHistory` remains a same-request client-to-upstream comparison, distinct from
the previous-completed-request comparison in `upstreamHistory`. New observations
can additionally align the body after leading developer/system messages. That
alignment records both skipped-prefix lengths without replacing the literal
shared prefix or claiming those messages were cache-reusable. Continuation deltas
are not aligned to materialized history; older reports do not invent alignment.
An interim successful-HTTP `tool_discovery` observation without error metadata
is pending, not a failed or completed logical request. Physical-send latency and
TTFT remain per attempt; `requestTiming` records elapsed logical execution time,
including preparation and recovery waits.
Failures after local BPS catalog lookup retain the bounded, redacted provider
error fields and observed upstream HTTP status in usage diagnostics, for both
buffered and streamed responses. Client-facing discovery errors remain generic;
retaining diagnostics does not authorize another generation or route replay.

### Reported usage and cache evidence

Counters, served model, tier, timings, and safe provider error IDs come from
upstream observations. Buffered SSE retains the last reported service tier even
after failure or truncated EOF; JSON discovery uses the shared nested-response
usage and tier readers. An omitted tier stays unknown for a new generation.
First output excludes heartbeats, empty deltas, lifecycle
events, terminal snapshots, and synthetic client SSE. Unknown timing stays unknown.
Buffered account SSE, including SIWC with a non-streaming client, observes the
first upstream output during bootstrap and collection, retaining it after a
partial failure. Its completion clock stops before client conversion. A JSON
snapshot records upstream completion, but leaves TTFT and generation unknown;
wrapping it in client SSE does not create timing evidence. Streaming SIWC clients
receive native or converted output before the upstream terminal event.
Each discovery generation owns its upstream timing: a follow-up interrupted
before any output keeps TTFT and generation unknown, even if an earlier lookup
already produced client-visible output. Client-output timing remains separate.
Partial cumulative counters merge without null erasure; checked derived totals
stay unknown on overflow, and reported totals are retained.
Request-size diagnostics reuse exact serialized value sizes for both body and
section totals.
Callers that already measured the unchanged tool catalog reuse that byte count.
Outer object and conversation-array delimiters contribute to body bytes;
section totals retain their item boundaries. All sizes are serialized JSON bytes.

Cache read/write counters remain per-request observations, including explicit
zeroes. Local cache activity is scoped by chat, account/source identity, and
upstream model. Only observed positive reads/writes move the touch; misses and
future or foreign retention records cannot supply its policy. The UI shows a
separate retention estimate, refreshes it through the existing usage revision,
and keeps previous observations after a failed read without extending them.
Model names do not prove an endpoint's TTL. Mixed prefix windows have no single
countdown, and an elapsed estimate is not evidence of provider eviction.

Managed account Responses requests preserve a client-supplied `prompt_cache_key`.
For Codex/BPS/SIWC, an absent key and identified client context supply a deterministic
opaque key scoped to the local key and resolved model, shared by HTTP and client
WebSocket paths.
SIWC does not upgrade the client's cache policy. A preserved routing-affinity
hint is not evidence of a provider cache hit; the UI labels it as affinity.
Unscoped requests and API-source bodies receive no generated key. Compact and
search endpoints keep their own contracts. Cache hints never authorize a
continuation or override quota, load, eligibility, or client cache controls.
BPS keeps explicit historical declaration updates at their original positions,
with root overrides and non-default tool controls after history; current schemas
and permissions win. Root-only catalogs retain their original stable prefix
placement. Changing a root-only catalog can
still invalidate that prefix. Default automatic selection adds no policy suffix.

### Request compression

HTTP request compression runs after final credential/continuation guards and
before dispatch admission. Only native Codex account POSTs to the official HTTPS
`chatgpt.com` Codex Responses paths opt into Zstd level 1: bodies from 64 KiB
through 8 MiB, only when encoding makes them smaller. Other origins, API-key
sources, BPS, SIWC, compact/search endpoints and already encoded bodies stay
unchanged. BPS rejects gzip/Zstd request bodies during JSON decoding; its
[observed request-encoding contract](BASIS_POINTS.md#http-request-encoding)
is independent of native Codex support. At most two encoding jobs run
concurrently; saturation keeps the identity body without queuing another job.
Encoding shares the reusable request body across the blocking worker and async
caller; the original request stays available until encoding succeeds. Local encoding
failure retains the original unsent body; remote rejection or uncertain execution
never triggers a new compression retry.
Debug diagnostics expose full/continuation payload mode, a transport/payload
reason for full input, post-guard JSON and encoded-body sizes, and encoding time.
`compressionUs` includes body preparation and blocking-pool scheduling; legacy
`compressionMs` is its whole-millisecond equivalent, not codec CPU time.
`httpSendToHeadersMs` begins at HTTP execute after encoding and authorization.
It includes connection setup, transfer and upstream waiting, not isolated upload
time; it overlaps the existing dispatch-relative headers/TTFT milestones.
These optional fields remain absent for unobserved stages and old records.
These exclude framing/TLS, do not prove delivery, and do not diagnose provider
cache misses. Missing observations stay missing. Native WebSocket setup already
opens only the selected authorized route before constructing/sending the payload;
connection reuse is recorded separately. No background all-account warmup,
`generate:false` model request, or provider cache prefill is introduced.

### Cost estimates and remaining quota

API-equivalent cost is a display-only estimate of token usage at API prices.
Price resolution uses observed provider prices, exact reference,
canonical reference, then manual fallback. Components, service tiers, and
published long-context bands stay separate; missing prices are not zero and do
not inherit another tier. Actual input usage selects the highest strictly
exceeded band. A model's context limit does not create a pricing threshold.
Canonical and image lookups use indexes built with each immutable catalog, not
full-catalog scans per projected model. The indexes enforce family isolation,
alias conflicts and image-match precedence, and are replaced with the catalog.
Desktop request details itemize input, cache read/write, and output using the
same query-time catalog snapshot and component rounding as the request total.
Reasoning cost is included in output. Missing breakdowns or
rates remain unavailable; a measured zero is distinct from an unknown cost.

Desktop remaining-quota cards share the persisted weekly-capacity calibration
below. Each current proven subscription ledger contributes at most its latest
valid paired measurement (at least 5% consumption and 80% priced tokens). Only
the priced component is used; missing prices are not extrapolated. Coverage is
persisted and shown. Older records default to full coverage because their
collector required it. Overview still excludes partially priced measurements.
Within a plan, weekly duration and pricing revision, each exact workload is
retained separately. Recent evidence is preferred within each profile, with the
existing 72-hour fallback. The card uses one median across the retained ledger
measurements, including mixed workloads, not a selected profile or the midpoint
of range bounds. One-subscription evidence remains explicitly labelled by sample
count, not promoted to a verified subscription allowance. This shared point is
scaled by the same remaining weekly fraction for comparable cards. Five-hour
quota observations and resets are not inputs to the weekly calibration.

Current inventory, membership and quota evidence fence persisted samples before
the collector catches up. Missing identity, unknown plan, reset, unvalidated prices,
quota corrections, expired or future observations cannot supply calibration.
Linked SIWC routes use the Codex/BPS subscription window, not an app ceiling;
credits-only SIWC routes are excluded. Snapshot projection performs bounded,
indexed anchor/latest-measurement reads, not request-history aggregation. Anchors
and history survive restart; there is no per-account cumulative fallback.
Catalog revisions trigger background revaluation, not a new quota baseline or
a requirement to consume another 5%. The collector prices the retained paired
interval using only its actual model/tier/context usage, then persists a separate
current-price calibration in the anchor. Unrelated catalog/metadata edits do not
discard that evidence. Real tariff changes update the estimate only when usage
is still complete and price coverage remains at least 80%; otherwise it is unknown.
New measurements retain a price-independent usage fingerprint, so repricing
rejects removed or corrected usage even when the coarse workload mix is unchanged.
Legacy anchors recover from retained intervals within 72 hours (raw sends are
retained for 30 days), skipping only compatible legacy pricing-change events in
a bounded lookup, never structural discontinuities. Original observation/reset
times and expiry remain unchanged. Historical chart rows retain their as-observed
prices; this revaluation neither rewrites history nor adds fictitious consumption.
Snapshot projection reads the persisted calibration and never reprices raw sends.
Cards show one approximate weekly remaining value or `—`; tooltips identify
workload/profile count, sample count and pricing coverage.
Forecasts expire at target reset, stale quota or the calibration's next expiry
even without a new snapshot. Provider-reported dollar balances remain distinct.
These display-only estimates do not change quota, admission, routing or billing.

### Weekly-capacity history

Overview's weekly-capacity history estimates the **full** weekly window.
A native coalesced worker
consumes existing state events even with the window hidden; it makes no provider
requests and does not project the UI snapshot. It pairs at least 500 basis points
of quota consumption with completely priced input/cache/output usage from that
interval. Shared OpenAI ledger identity deduplicates linked Codex/BPS/SIWC routes;
only Codex/BPS weekly evidence establishes the window, never an SIWC app ceiling.
SIWC registrations with subscription use disabled do not contribute usage.
Unknown ledger identity/plan, incomplete prices or missing observations produce
gaps. Subscription price does not determine a quota multiplier.

Capacity = interval API-equivalent microUSD × 10,000 / consumed basis points.
Comparisons separate plan, pricing revision and joint model/tier/context/reasoning
mix (10% token-weight bands), with cache-input bands of 25% and output-share bands
of 10%. Each ledger contributes one latest vote per bucket; charts show median
and empirical P10–P90 spread, not a confidence interval. Current estimates prefer
measurements wholly within 24 hours, fall back only through 72 hours and expire
at reset. Cycle changes, changed plans and unexplained quota increases break
calibration; price revisions separate historical valuations but do not break the
paired quota interval. Only positive cycle evidence is labeled a reset. External
usage, observation lag and coarse quota readings can bias the estimate, so a
change is not proof of a provider policy change.

The default **All models** view combines raw observations across workloads within
one subscription plan and pricing revision. It keeps one latest vote per shared
ledger in each bucket/current estimate, then takes the median and empirical
spread; it never averages model-group medians or combines Plus/Pro plans. Native
plan summaries are built before the model-choice limit and ordered by their newest
observation. The default view selects the latest observed pricing revision for
the chosen plan. The model selector contains each model name once, without
workload ordinals or mixed-model labels. Explicit model views aggregate raw
single-model observations across workload variants, keeping one latest vote per
ledger and separating plan/pricing revisions. The latest observed revision is
selected for that model; group medians are never averaged. Mixed-model intervals
remain in All models and are not attributed to an individual model. Detailed
workload groups remain available in the native report. New observations store
model identities explicitly; legacy joined labels supply a model only when
every recorded profile names the same one.

SQLite keeps anchors across restarts (up to 512 ledgers) and at most 50,000 history
rows/30 days. Existing maintenance and explicit usage-history clearing remove
these observations too. Permanent account-data deletion removes affected shared-ledger
estimates.
History stores scalar estimates and workload metadata, not credentials, prompts
or provider bodies. Collection and reports aggregate
through the WAL reader; anchor commits are fenced against history clears. Reports
return at most 32 newest workload groups, 32 model series and 32 plan/pricing
summaries. The chart after the speed charts refreshes on
capacity revision/range changes or local evidence expiry.
Selectors, range spread and observation details stay compact or in tooltips.

The Desktop Pool summary aggregates lifetime API-equivalent usage only for its
current members. Remaining value sums known estimates once per proven provider
ledger, not per linked connection and not API-source wallets; unknown identities
remain separate. Coverage counts independent ledgers; partial coverage is marked
as approximate. Actual per-connection usage is not discarded by deduplication.
Total payback divides the
usage of accounts with known purchase costs by the sum of those costs, never
averaging percentages or including API-source usage. Unknown remaining values,
zero total purchase cost, and wholly unpriced account usage stay unavailable.
Coverage details remain in tooltips. Usage-period filters do not change lifetime
account payback. Overview retains its four original counters and chart selection.

### Price provenance and service tiers

Source editors receive source-scoped resolved quotes and provenance from the
same query-time catalog and resolver as usage. A pool-wide model summary is
not a fallback for another source; it may describe a different member's quote.
Price provenance appears on hover or keyboard focus, not as row copy. Rows
show only editable prices and selectors with multiple available choices.
Manual fallback drafts stay separate from automatic placeholders, including
after reset. The editor shows USD per million tokens before the price rows.
Price schedule choices require explicit rates or saved manual evidence, and
input-token price thresholds are scoped to the selected schedule. Neither a
maximum context window nor an inference speed capability creates a tariff.
Missing schedules are not offered as universal Flex/Priority/Ultrafast options.
Generic and explicitly windowed cache-write prices can coexist in one row;
provider grouping does not hide a quoted component.

Applied-tier classification is shared by request details, SQL groups, quota
windows and archive keys. An absent/default/standard tier retains the legacy
base estimate; an explicit unsupported tier is unpriced, never Standard.
Keep each normalized provider name in aggregates. No other tier supplies its input,
output or cache rate, including a zero rate. Same-tier context overlays preserve absent base fields
and apply explicit zeroes identically in the editor and the estimator.
Local migration 035 reclassifies retained tier evidence transactionally and
idempotently; old migration decoding stays fixed. Earlier archived aggregates
without their original tier retain their historical classification rather than
inventing recoverable evidence. Ordinary catalog-rate refreshes need no migration.

Cache-creation prices and cache retention are separate metadata. Preserve a
provider's explicit generic creation tariff independently of Messages-specific
window tariffs; do not label it with an assumed TTL or derive other rates using
fixed multipliers. Source adapters interpret their published field semantics.
The LiteLLM adapter applies Anthropic's default-window meaning only to known
Anthropic provider contracts, not to other hosts or model names. Compatible
catalog discovery retains untagged creation prices as generic even on Messages
endpoints. A read-price TTL map without a matching usage dimension is usable
only when all supplied rates agree; it never selects the cheapest window.
A missing or mixed TTL can use a generic tariff only when no different published
window tariff makes the rate ambiguous. Otherwise the affected tokens remain
unpriced. Rates, tiers and context bands refresh from catalog data without a
usage-schema migration; observing a TTL never establishes a missing price.
Provider usage is normalized before pricing: Messages input excludes its cache
read/write counters, while Gemini prompt input and Chat Completions prompt input
already include cache hits. DeepSeek's `prompt_cache_hit_tokens` is a read-counter
alias, not additional input or a cache write. Gemini thoughts join generated
output once. This request-token estimate does not measure separately billed
cache-storage token-time, hosted tools, grounding, or other non-token charges;
it is not a complete provider invoice.
Optional provider usage diagnostics retain reported web-search, web-fetch and
code-execution counters and bounded cache-creation TTL buckets. Explicit zero
and missing remain distinct; cumulative snapshots merge by maximum within an
attempt, not by adding SSE frames. Native observations survive conversion and
partial failures through the existing usage JSON. TTL buckets are subsets of
cache-write totals, not additional tokens or inferred charges; arbitrary tool
names, arguments and provider fields are excluded.
Responses `cache_write_tokens_details` retains reported 30-minute and 12-hour
buckets, including zero. Nonzero buckets establish written lifetimes instead
of treating the echoed request default as every prefix's lifetime; later frames
without usage cannot overwrite those lifetimes with the default. Missing
write totals remain unknown rather than being reconstructed from buckets.
Catalog lookup attempts retain their own provider counters; the next generation
starts a separate observation. These counters do not measure subscription quota.
Source contracts: [OpenAI pricing](https://developers.openai.com/api/docs/pricing),
[OpenAI prompt caching](https://developers.openai.com/api/docs/guides/prompt-caching),
[GPT-6.1 Sol price schedules](https://developers.openai.com/api/docs/models/gpt-6.1-sol),
[Anthropic caching](https://platform.claude.com/docs/en/build-with-claude/prompt-caching),
[Gemini usage](https://ai.google.dev/api/generate-content#UsageMetadata),
[Gemini pricing](https://ai.google.dev/gemini-api/docs/pricing),
[DeepSeek usage](https://api-docs.deepseek.com/api/create-chat-completion), and
the [LiteLLM catalog](https://github.com/BerriAI/litellm/blob/main/model_prices_and_context_window.json).

### Context comparison

Debug context comparison uses bounded, salted in-memory fingerprints to compare
local request prefixes; cache hits come from provider usage. Native Responses,
SIWC and BPS Responses share this diagnostic path,
including the HTTP bridge for WebSocket clients; debug-off requests collect no
context fingerprints. BPS snapshots the actual dispatched body after catalog
preloads and attachment resolution, and refreshes it for each discovery send.
Completion compares the last dispatched prefix;
discovery usage remains separately recorded and does not complete the client
request's comparison baseline. Client cache controls and reasoning settings
remain unchanged; BPS diagnostics never add provider comparison/retention fields.
Serialization uses one reusable 8 KiB scratch buffer per capture, flushing at
each fingerprint boundary; salts, digests, exact byte budgets and item limits
are unchanged. This optimization remains debug-only, not provider cache storage.
In debug mode SIWC can request the provider's official prompt-cache comparison
against the last successful, non-overlapping response on the same local key,
client session/cache scope, account and model. This uses the existing bounded
in-memory comparison store; response IDs are never serialized into usage.
An explicit client comparison ID (including null) and cache mode/TTL are preserved.
Comparison is diagnostic only: it does not retrieve history, enable continuation,
change cache policy, add another inference, or authorize a retry. Provider
classifications and token counts are retained separately from local prefix
comparisons and observed usage; an unavailable comparison is not a cache miss.
Bounded debug snapshots also expose client/sent cache mode, TTL, prewarm and
key presence as allowlisted classifications, without key values. They distinguish
omitted controls from explicit settings and unknown values rather than inferring
model defaults. Content-block and function/custom-result cache markers are counted
with their last input-item position and TTL-override count; a bounded partial scan
is marked incomplete. Tool schemas and opaque state are not scanned as markers.
Native SIWC preserves explicit breakpoints (including per-block TTL), intentional
explicit-only requests with zero breakpoints, and appended configuration updates.
Relay does not insert markers, change TTL, or generate extra requests to raise
cache-hit counters. These controls do not prove cache eligibility or reuse.
Reference: https://developers.openai.com/api/docs/guides/prompt-caching
and https://developers.openai.com/api/docs/guides/prompt-caching/diagnostics
The manual `quality_probe` is opt-in, loopback-only, bounded, and separate
from background monitoring; see [operator steps](../../CONTRIBUTING.md#manual-quality-observations).

## Application profiles

Profile operations follow inspect → protected snapshot → apply → verify → restore.
Rollback changes only unchanged Relay-owned fields and preserves newer user edits
and sign-ins. Credential snapshots use protected storage. Invalid executable
overrides fail rather than launching another installation.

Switching a managed Codex connection synchronizes local chat provider metadata
before applying the new profile; users do not bind individual chats manually.
History discovery inspects SQLite files in the profile root and its `sqlite`
directory without depending on a fixed `state_N.sqlite` version. Only supported
history/catalog schemas and rows associated with inspected local rollouts are
rewritten. Rollouts, thread rows and the Desktop catalog follow the selected
provider together, with protected backups and rollback if profile attachment
fails. This local profile migration does not relax gateway continuation ownership.

The local ChatGPT application settings expose separate memory extraction and
consolidation controls. Their model selectors update `memories.extract_model`
and `memories.consolidation_model` respectively. Selection is limited to enabled
Responses-routable pool models and requires an unchanged, active local-gateway
profile. Each command checks the previously read model and atomically replaces
only that stage's key; reset removes it. Models need a manual ChatGPT restart.
Existing memory settings, the main chat model and foreign profiles remain
user-owned. Pool availability does not guarantee memory-task context/capability
compatibility, and no automatic model fallback is added.

The two switches are persistent **Relay admission policies**, both allowed by
default, not invented Codex config keys. They apply independently to new HTTP
and WebSocket requests without restarting Relay. Explicit Codex metadata
`request_kind=memory` identifies extraction even though its `thread_source`
also says `memory_consolidation`; the latter source/trigger identifies the
consolidation worker for other request kinds. Neither model names nor prompt
text identifies a memory task. Clients without these markers cannot be gated
reliably. Blocked memory requests return a local 403 and a zero-dispatch usage
event, never a fabricated empty success that could complete a memory job.
Allowed requests are labeled by memory stage in usage. Disabling a stage does
not cancel in-flight work, clear memory, change `generate_memories`/`use_memories`,
or disable ordinary chat/context compaction. Allowing a stage does not force the
client to enable its memory feature. Activity-summary/title policy is separate.
OpenAI source contract: [memory configuration schema](https://github.com/openai/codex/blob/86e17e98c7f901277aeeb4649cf1b01462b082f2/codex-rs/core/config.schema.json),
[extraction metadata](https://github.com/openai/codex/blob/86e17e98c7f901277aeeb4649cf1b01462b082f2/codex-rs/core/src/turn_metadata.rs#L127-L155),
[request-kind semantics](https://github.com/openai/codex/blob/86e17e98c7f901277aeeb4649cf1b01462b082f2/codex-rs/core/src/responses_metadata.rs#L167-L185),
and [consolidation worker](https://github.com/openai/codex/blob/86e17e98c7f901277aeeb4649cf1b01462b082f2/codex-rs/memories/write/src/runtime.rs).

Context evidence and policy remain separate:

| Policy | Projection |
| --- | --- |
| Short | Published default, capped by a known hard maximum; only without a default use 25% of a known maximum, with a 1,024-token floor |
| Extended | Full known maximum; otherwise preserve an available provider default |
| Custom | 1,024–2,000,000 tokens, capped only by known hard model/route limits |

Native context comes from the owning account's exact model card, not reference
API maxima or an installed Codex default. API routes use their own reference
evidence. Unknown windows remain unknown; no guessed 128k default. Known
short/long windows are not replaced by percentages or pricing bands.

Compaction is projected at 90% of the selected window, capped by known input
and output/hard-limit constraints. Relay never silently truncates requests.
Managed root context overrides are recoverable; explicit newer user settings
remain user-owned and do not block subsequent catalog updates or reconnects.
Catalog changes publish atomically without stopping requests.
A client that only loads configuration at startup still needs relaunch; saving
policy never automatically closes/restarts an application.

ChatGPT, Claude Desktop, and OpenCode have separate profile ownership and recovery.
Claude uses a Relay-owned UUID profile in its `Claude-3p` library, preserves foreign
profiles/selections, and maps stable client-only Claude IDs back to real models.
An explicit reconnect can repair stale recovery fingerprints using the same
current-gateway proof as missing-recovery reclamation: endpoint, key, supported
generated settings and the owned entry must match. The original recovery point
is retained. Pending snapshots, foreign settings, background refresh and restore
do not gain permission to adopt an externally changed profile.
There is one entry per enabled Messages-routable model, not one per effort.
Desktop's native effort menu is capped at the highest pool-allowed level on a
Messages route. `maxEffort` cannot remove intermediate holes, and supported
higher choices are not hidden. Incoming effort selections remain strict:
Relay rejects unsupported choices before dispatch instead of replacing them
with a lower level. Exact route/key scope and pool policy still apply.
Disabled thinking and explicit budgets retain their separate contracts.
Desktop can render arbitrary `thinking.effort_options`, but the third-party
`modelCatalogUrl` accepts only Anthropic-signed catalog bytes, not Relay's
own unsigned metadata. Do not configure a non-working local catalog or claim
the current picker matches sparse Relay sets. Exact per-model Desktop menus
remain open work in [ROADMAP.md](ROADMAP.md).
The embedded Code engine has its own model-capability lookup. A Desktop picker
selection or saved session effort does not prove the effort sent over HTTP:
unsupported `max`/`xhigh` can become `high` inside that client. Relay records
and validates the received effort; it cannot safely infer a higher UI selection
from `high` or force every request to the profile's `maxEffort` cap.
Aliases grant no upstream capabilities, pricing, or context. Catalog overflow fails before
closing/writing rather than truncating. Client-owned MCP discovery is enabled
only in the owned profile.

The generated Claude profile sets `autoModeEnabled: false`: client-only model
aliases do not establish a separate safety-classifier route. Desktop otherwise
offers Auto mode by default, and a Fable-shaped alias can make Code fall back to
an unavailable Opus classifier. This leaves manual approvals available; Relay
does not enable bypass, override a user's permission mode, or substitute the
session model for the safety classifier. Enabling Auto requires a separately
qualified classifier integration, not an alias or an effort capability. See
the official [Desktop Auto-mode configuration](https://claude.com/docs/third-party/claude-desktop/configuration#automodeenabled)
and [Code classifier selection](https://code.claude.com/docs/en/permission-modes#default-classifier-model).
Owned older profiles gain the explicit flag on reconnect/catalog refresh;
missing-recovery reclamation still rejects explicit foreign capability settings.

OpenCode profiles explicitly disable standard SDK reasoning variants that the
model route does not advertise; omission would let OpenCode restore its built-in
defaults. Models without evidenced effort levels retain the provider's default
mode. Refresh can re-enable a newly supported effort, removes stale generated
variants, and preserves genuinely custom user variants.

Managed Codex Responses requests using Relay's reserved model aliases treat
`reasoning.effort: none` as a picker placeholder only when reference metadata
confirms thinking, publishes no explicit control method, and the configured
route union advertises no effort choices. HTTP and WebSocket ingress remove
only that placeholder and retain other reasoning fields. Ordinary API clients,
raw model IDs, unknown metadata and models with evidenced controls keep their
original request. This is client compatibility policy, not an inference that
missing metadata means a provider cannot reason or accept a control.
OpenCode's built-in `Default` is the provider-default mode, not an advertised
effort, and cannot be removed by the model variant configuration. Clients that
cache the profile at startup must reload before its new effort list takes effect.

Connect validates routes, key, catalog, ownership, and launch target before process
changes. Only the requested launch preference starts the client after attachment.
Overview opens the selected installed application with its existing configuration,
preserving running clients. It works with the local API stopped, an empty pool,
or a stale runtime/profile. Profile attachment, catalog refresh, credential changes
and API startup are separate actions.
Launch preferences take precedence over `OPENCODE_BIN` / `CLAUDE_DESKTOP_BIN`,
then installation discovery. Explicit overrides are validated even when a client
is already running; invalid overrides never silently select another installation.
Autodetected files pass desktop identity and executable checks before a restart.
Discovery continues past CLI entries on PATH and compares Claude version numbers
numerically. Process control requires a recognized desktop path and excludes
Chromium helpers. OpenCode and Claude launch success requires a running desktop,
not merely a successful spawn. Blocking launch/restart commands run off the UI
thread; the legacy saved-key launch leaves Relay visible when launch fails.
Automatic attachment rollback preserves newer user sign-ins and unrelated changes.
Named ChatGPT snapshots capture the current account/API authorization and active route.
Only root connection defaults, the selected named profile and its provider definition
are restored. Inactive profile overrides, reasoning/context preferences, MCP and auth
extensions remain current; shared defaults and provider definitions can still change
an inactive profile's effective connection. Model selections are restored when the
provider, endpoint or wire protocol changes, or the config is missing.
A referenced Relay-owned catalog and managed binding's protected undo data are
captured together. Restore recreates fresh undo references, preserving account identity,
token synchronization, catalog refresh and later detach; the snapshot remains reusable.
Payload v2 captures the effective credential document from `file`, OS `keyring`, or
`auto` (keyring first, file only when its entry is absent). It supports direct keyring
and `secret_auth_storage`: `secrets/codex_auth.age`, the Windows default, with only
`global/CODEX_AUTH` replaced. The shared encryption key, MCP namespaces and other entries
remain intact; a missing key for existing encrypted files requires recovery. Native
restore clears the stale fallback file. A locked store or process-only `ephemeral`
requires recovery rather than guessing credentials.
Legacy v1 file snapshots remain readable; v1 keyring snapshots lack captured authorization.
Codex's `auth.lock` coordinates credential changes. The profile undo journal includes
native credentials, files and protected binding secrets for conditional error rollback.
History repair uses the saved effective provider, including custom API providers, and
preserves rollout content and newer chats. History rollback follows a failed connection
restore. This in-process rollback is not a durable power-loss transaction.
OpenCode and Claude retain their separate recovery scopes. Source edits do not upgrade
an already running binary.

The retired personal-server runtime, management client, deployment templates and
account-transfer operations are removed. Existing ownership evidence is preserved
only for local recovery: affected accounts stay disabled and outside the pool
until the user confirms the external owner is stopped. Recovery never enables
an account or transfers credentials. Hosted is not a migration of that runtime.

## Not provided by the current implementation

Hosted tenants, customer identities/keys, wallets, payments, and billing are not
Desktop features. PostgreSQL checkpoints, encrypted payload
retention, SDK idempotency, and settlement are planned in
[Hosted architecture](../hosted/HOSTED_RELAY_ARCHITECTURE.md).

Distributed scheduling, account resale, fingerprint spoofing, sharing concealment,
and automatic transfer of opaque conversations between accounts are outside the
first hosted release. Open local acceptance and measured improvements belong in
[ROADMAP.md](ROADMAP.md), not in claims of completed behavior here.
