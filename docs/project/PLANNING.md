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
Account-only attempts retain explicit outcomes for completion, returned failure,
repair and candidate-selection retries, and unknown execution. A collected invalid
or incomplete result stays a failure; an unreadable body stays execution-uncertain.
Repair exhaustion retains the logical request identity and each physical attempt's
own reported usage, without another dispatch after an uncertain result.

`execution/state.rs` owns request-local retry history and continuation/repair
state shared by HTTP and account-only controllers. HTTP selection, preparation,
dispatch, rejection, buffered completion and stream bootstrap borrow that typed
owner or its retry component. Rejection collection, repair and settlement carry
the same owner through their stages. Account-only stages retain prepared
continuation facts instead of deriving them again from a rewritten body.
Identity, budget, leases and recorded outcomes keep their owners;
the shared budget remains authoritative for physical dispatch counts.
With route recovery enabled, proven pre-execution throttling or model-capacity
rejections first retain the selected candidate in that shared request budget.
The existing admission queue waits without holding a lease, respecting provider
cooldowns and exponential request-local pacing with jitter. This preference lasts
at most 30 seconds, retains its original deadline through retries and transport
handoffs, and revalidates scope, route permissions and credential incarnation.
It neither refunds physical dispatches nor materializes opaque history merely
to escape a temporary rejection. On expiry or invalidation, portable routes
resume ordinary selection; SIWC requires an explicit structured transient code
and cannot gain a cross-account fallback from this temporary-error policy.
Quota, authentication, partial output and uncertain
execution keep their separate recovery contracts.
An internal HTTP materialization retry moves its existing state, retaining
consumed repairs and request-local preparation exclusions.
`runtime/models.rs` owns model registry and editable model policy while retaining
separate locks and the shared visibility handle observed by candidate leases.
`runtime/provider_state.rs` groups the remaining graph-local provider state:
`RuntimeSourceState` owns connectors, discovery/proxy clients, bindings and
recovery delays; `RuntimeAccountState` owns account executors, principal groups,
passive quota and breaker state; `RuntimeProviderMetadata` owns reference-catalog
handles and existing native-model, transport and refresh-hint observations.
Each existing lock, cache limit and invalidation retains its boundary. Policy
edits preserve these owners and shared catalog handles; model policy stays with
`RuntimeModelState`, and conversation stores remain with their existing owners.
Source-policy batches validate every participant before publishing scheduler
rules and recovery delays together. The commit holds the scheduler lock, then
the recovery-delay map lock, and notifies waiters after releasing both. Recovery
readers copy their delay and release that map before settling scheduler state.
Policy-only source/account, visibility and key-scope edits retain the listener
and runtime graph. Dispatched streams keep their leases and saved continuations;
pending sends revalidate the current policy. The dispatch's credential revision
survives policy edits, while a replacement login invalidates it even when token
generation numbers match.
Runtime reconstruction derives quota exhaustion and limiting reset timestamps
from durable quota snapshots. Legacy cooldown maps and failure counts do not
become hard admission evidence when rebuilding account executors.
Request routing diagnostics retain an optional `providerBackoff` with the
normalized provider deadline and its candidate-wide (`*`) or model scope.
When known, `observedAtMs` retains the local clock used to normalize that exact
deadline. Equal deadlines retain the earliest known observation. Storage times,
reads, membership refreshes and shorter hints never replace it; older records
without an observation time remain unknown.
HTTP, SSE, WebSocket terminals and deferred discovery retain the paired deadline
and observation from hint normalization through settlement. Re-reading an already
normalized `Retry-After` does not renew it; combined observations compare absolute
deadlines rather than relative durations from different clocks. HTTP error replies
derive `Retry-After` from the remaining interval. WebSocket body reset hints stay
with the shared parser instead of becoming synthetic provider headers.
WebSocket initial-rejection errors carry that normalized `retry_at_ms` through
retry selection and client error serialization. Subscription reset evidence stays
bound to account origins; API-source errors use only their explicit retry hints.
Existing hint parsing and bounds apply; local recovery floors and circuit
delays remain separate in the effective `retryAtMs`. Absolute `resets_at` values
retain millisecond precision in both Unix-second and Unix-millisecond forms,
including resets within the current second. Expired values may fall back to
explicit relative hints, without reviving an expired deadline. Missing or
expired provider evidence stays absent. The scheduler settles and reads this evidence under its
existing lock and credential-revision guards. Confirmed shared-rate buckets
retain their paired deadline and observation time, without borrowing other limits on the same
model or extending the clock with inferred group backoff. Desktop usage storage
preserves it for measurement, without credential provenance. Support exports
classify the scope and replace model IDs with report-local references; these
records are not an authority for restoring admission deadlines after restart.
Policy-only visibility, key-scope and source-model edits leave dispatched native
WebSocket responses and their leases intact; queued requests check current
permissions before dispatch, and restored access retains valid connection history.

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
Opening folders, exporting accounts/Usage/configuration, previewing a preset and copying
an API key do not reload the full runtime. Applying a preset, rotating a key and
other runtime mutations retain their refresh. Settings reuse immutable native
storage roots only within the current renderer; mutable diagnostic settings still
revalidate. Network observation does not restart for unrelated busy/feedback
changes, and an older network read cannot undo a completed sharing change.
Cancelling an account or Usage save dialog settles silently. The account export
editor keeps its description and selection for retry; success feedback follows
an actual file save or completed clipboard write.
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

The fixed `account-subscription-probe` operation remains readable in JSONL and
interrupted-stage markers. This exception applies only to the operation field;
detail values, extended account-like tokens, credentials and free-form text keep
their existing redaction.
Recognized credential patterns are redacted from error-code fields as well as
diagnostic text; canonical error labels remain readable.

Native snapshots, renderer timing writes and blocking configuration, filesystem
and network-interface reads run off the window thread. Results include completed
validation and storage errors. Snapshot timing separates native preparation,
queue/worker waits, IPC, paint and interactive readiness; preparations of at least
250 ms also record fixed-name stages. Every successful native snapshot separately
records `snapshot_lock_wait` with the fixed context `store`, including zero and
sub-millisecond waits. It measures acquisition of the local store mutex, excluding
work performed while holding it, SQLite locks and asynchronous worker queues.
This wait is a subset of the inclusive input stage and total, not an additive
projection stage. A timing batch commits atomically, including retention, and
contains no account, source, model or credential values.

Performance history retains the latest 128 samples per metric, or 256 for the
multi-stage `snapshot_stage` metric, for up to 30 days and 2,048 rows overall.
Frequent snapshots cannot evict rare startup, navigation or command timings
merely by advancing SQLite row IDs. Retention counts actual rows after pruning,
uses a metric/ID index, and remains atomic with each timing batch. Schema 45
applies these bounds to existing diagnostic samples without changing request
history or application settings.

Each Desktop state admits one full runtime snapshot before reading its inputs;
clones share that bound separately from the setup mutation lock. Cancellation
retains the slot until blocking work finishes. Input capture, including store
and credential-cache lock waits, runs in the same blocking worker as projection;
it does not stall the asynchronous command executor. Inputs are read before the
runtime handle, after admission and worker dispatch. Routing previews use the
current inventory/catalog, reuse admission only within that snapshot and never advance
live rotation; ambiguous ranks stop the preview. Presentation indexes and saved
model ordering provide no route, quota or authorization evidence.

### Metadata and query caches

Metadata caches persist validated source envelopes rather than the merged JSON
tree in the current version-2 bundle. Retired single-source caches and auxiliary
sidecar files are not restored; normal reference refresh rebuilds the cache.
An invalid cache does not prevent startup. Immutable catalogs, pricing and
indexes can be shared across unchanged
revisions; refresh failure retains prior data and updates status. Freshness changes
recompute metadata winners even after HTTP 304. Within the exact provider/model
scope, fresh sources win and stale sources only fill missing fields.
Reference refreshes validate every record's structure and nonempty bounded string
identity before replacing its source envelope. A mixed valid/malformed response
retains the previous source revision, models and efforts as stale, including
after cache reload; other reference sources can still refresh independently.
Unique scoped identities share the merged catalog's total model limit across
providers and fresh/stale sources. Overflow rejects the replacement before writing
the cache or publishing a snapshot; updating an existing identity at the limit
remains valid. An oversized cached combination retains its source envelopes,
records an invalid-cache state and schedules startup refresh without exposing a
partial catalog. Supported primary map/array forms, provider model-key defaults,
LiteLLM specification/alias records and unknown extension fields remain accepted.
Source contracts:
[models.dev](https://models.dev/#api),
[OpenRouter](https://openrouter.ai/docs/guides/overview/models) and
[LiteLLM](https://docs.litellm.ai/docs/proxy/model_cost_map).

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

Usage restores only the current exact query key; retired pre-`includeReport`
preview keys are not reused. Native history remains authoritative.
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

Explicit source catalog refresh returns a credential-free summary of model IDs
added and removed at the fenced commit, rather than comparing a stale renderer
snapshot. Partial reads report their committed changes and incomplete status;
failed reads and retained manual lists are distinct from complete refreshes.
Connections refreshes catalogs and balance without an additional result line.
Pool refresh keeps manual model lists and does not report incomplete discovery
as success.

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
most once. Submitter text, input values and accessible labels are checked before
filling and again before submission; account creation, recovery and authorization
actions require manual continuation. CAPTCHA, errors, consent, social
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
An unresolved SIWC rotation outcome stops further exchanges of that grant, while
an access token with a known future expiry remains usable in the existing
`degraded_access_only` state. The durable checkpoint survives restart; reaching
actual expiry or a confirmed access rejection requires reauthentication with
`token_rotation_recovery_required`. An invalid checkpoint or received identity
still requires reauthentication immediately. Provider renewal windows are checked
before claiming a grant: the authority schedules the next attempt at
`earliest_refresh_at`, keeps genuinely unexpired access usable, and does not count
the wait as a failed refresh. New sign-in replaces this scheduling state.
SIWC renewal diagnostics retain the stage, bounded request ID when supplied, and
next permitted attempt, without tokens, identities, URLs or response bodies.
A temporary vault I/O or
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
SIWC uses account-scoped public `/v1/models` over HTTP and `/v1/responses` over
WebSocket, `store=false`, and terminal `response.completed`. Text generations,
including summary compaction and local catalog rounds, require upstream
WebSockets independently of the client transport setting. Authentication, token
renewal and model discovery keep their HTTP transports.
This policy leaves image-generation operations and image endpoints unchanged;
images in a text request's input remain intact.
Identified conversations can retain one authenticated upstream socket per local
key, account and conversation. The store holds at most 32 retained lanes, expires
idle lanes after 30 minutes, and replaces sockets older than 55 minutes between turns.
At capacity, a new conversation reclaims the least recently used unlocked lane;
busy owners remain isolated and cannot be evicted. A renewed grant or authorization
revision discards the old lane state.
Retained idle sockets answer peer pings and send control-only pings every 20 seconds.
Their task stops on idle/connection expiry, invalid peer frames, transport failure
or owner removal; resuming a turn transfers the socket back to the same dispatch
owner. Heartbeats never create generations, refresh credentials or extend the TTL.
Continuation sends a provider `previous_response_id` and new items only after
the prepared history exactly matches the previous input and completed output,
with unchanged request settings. Comparison ignores JSON object key order,
preserving array order, opaque strings and the outgoing payload. A completed
reasoning item's omitted `status` is equivalent to `status=completed` only for
this comparison, matching Codex's typed replay. IDs, ciphertext, summaries,
unknown fields and non-completed statuses remain part of the proof.
The same comparison treats absent, null and empty-array reasoning `content` as
equivalent, matching Codex's `Option` serialization. Nonempty content remains exact.
Optional nullable message `phase`, reasoning `encrypted_content` and
function/custom-call `caller` share the completed-snapshot comparison policy.
For socket comparison only, null and omitted message/function/custom-call
`encrypted_content`, function/custom-call `namespace` and `async`, and function-call
`encrypted_function_args` also compare equally. Non-null values, including both
boolean `async` values and nonempty encrypted data, remain part of the proof.
This follows the [Codex input model](https://github.com/openai/codex/blob/340a18ed7cd0aea3bc218838d9ac1f0f30efb1ac/codex-rs/protocol/src/models.rs)
and the public [function-call](https://github.com/openai/openai-python/blob/main/src/openai/types/responses/response_function_tool_call.py)
and [custom-call](https://github.com/openai/openai-python/blob/main/src/openai/types/responses/response_custom_tool_call.py)
types. Normalization never changes the outgoing full-history input.
The comparison-only cache response ID does not
change that proof; all caller cache controls remain on the wire. Stored checkpoints
contain hashes and the response ID, not prompt/tool bodies. Streamed output
snapshots are reconciled with the shared bounded native-replay capture:
completed items supply sparse terminal output and retain completed reasoning
ciphertext. Replacing an indexed snapshot charges only the retained item, within
the existing 2 MiB and 1,024-item capture limits. Reconstructing sparse output
from indexed snapshots requires contiguous indices starting at zero; gaps,
mixed indexed/unindexed items and malformed indices need a complete terminal
snapshot instead. Interrupted or unmaterialized output cannot establish a checkpoint.
Buffered account JSON responses use the same sparse-output ordering: duplicate
indices contribute one unchanged item, while conflicting duplicates cannot
create repeated tool calls. Conflicting completed-item duplicates also
permanently disqualify streaming replay; a later valid snapshot cannot restore it.
Nonempty terminal output must agree with every completed snapshot, including
tool identity/arguments, message content and reasoning summaries. It cannot hide
conflicting duplicates or drop completed items. Completed reasoning ciphertext
keeps its precedence; terminal-only ciphertext remains when none was supplied
earlier. Unindexed items match unique IDs, or arrival positions for a complete
unindexed sequence without IDs.
Omitted replay terminal fields can use observed
state; explicit invalid IDs/output shapes and conflicting response IDs cannot
be repaired into a valid checkpoint.
Changed or unproven history sends the complete input over WebSocket. Context-free turns, busy lanes
and exhausted retained-lane capacity use isolated full-history sockets without
sharing continuation state. A failed upgrade returns an error rather than an HTTP
generation; provider authorization and quota rejections retain their existing error
semantics. Unknown frame-send outcomes, interrupted streams,
and failed/incomplete terminals discard the socket without replay. Lease, principal,
model and token fences still precede generation; handshake and generation debit
the existing shared budget separately. WebSocket events enter the shared SSE
pipeline, while diagnostics retain the actual upstream transport and payload size.
Fresh-socket diagnostics include a bounded reset reason for session isolation,
expiry, changed authorization, peer closure, pending frames or unproven output.
Full-history payload diagnostics distinguish missing socket checkpoints, changed
settings, edited prior input, mismatched output and an absent new suffix.
With diagnostics enabled, the first mismatched output within the first 32 items
also reports its output-relative index and fixed field labels. Field proofs stay
bounded and volatile; item values, client-defined names and hashes are not logged.
Execution `async` and `encrypted_function_args` differences have their own fixed
labels rather than being grouped under `other`; their values are never recorded.
The mismatch also classifies the completed item using fixed kind labels. Documented
execution, tool catalog, action and image-result fields have separate labels;
these labels do not make unsupported hosted tools eligible. Unknown fields retain
the strict comparison. Diagnostics report only added/removed/changed counts and
whether all observed differences are null omissions/additions, using at most 16
volatile key/value proofs per item. Truncated proofs explicitly mark incomplete
counts and cannot claim null-only differences. Neither diagnostic authorizes a
continuation or retains unknown field names or values.
Proof selection is independent of JSON key order. Truncated comparisons count
only proven changes; an unretained field is not reported as added or removed.
Continuation comparison ignores known client-only attribution/telemetry metadata
on messages, reasoning and function/custom calls, without rewriting wire input.
Absent, null and empty-array reasoning content compare equally; nonempty content
remains exact. Valid top-level `client_metadata` string maps are per-request
tracing rather than model-prefix settings and remain unchanged on the wire.
The documented boolean `stream_options.include_obfuscation` changes delivery,
not the cached prefix, and is likewise preserved on the wire but excluded from
the continuation proof. Unknown stream controls and malformed values stay strict.
Tool-result records and unknown extensions remain strict, alongside text, IDs,
arguments and opaque reasoning. This follows Codex's
[continuation comparison](https://github.com/openai/codex/blob/340a18ed7cd0aea3bc218838d9ac1f0f30efb1ac/codex-rs/core/src/client.rs#L429-L520)
and [reasoning serialization](https://github.com/openai/codex/blob/340a18ed7cd0aea3bc218838d9ac1f0f30efb1ac/codex-rs/protocol/src/models.rs#L1858-L1864),
with a narrower allowlist rather than discarding the metadata object wholesale.
HTTP/SSE and WebSocket clients keep their existing ingress protocols and shared
terminal/model validation. Their transport does not select HTTP upstream.
The socket replay checkpoint also follows the shared error/model guards: any
observed provider failure or rejected serving model permanently disqualifies
that response's socket from reuse, even if a later terminal claims completion.
This does not replace the original client-facing error or authorize a retry.
Client `previous_response_id` continuations use the same bounded, owner-validated
native replay as BPS before request preparation, including the client WebSocket
bridge. The executor restores saved input, completed output and new items before
proving a same-socket delta or sending full-history WebSocket input. Missing,
foreign-key or wrong-model replay is rejected before dispatch rather than dropping
context or changing accounts. The [SIWC inference contract](https://developers.openai.com/siwc/token-sharing-open-source/models-and-inference)
requires the public API, not ChatGPT backend endpoints; its
[preview limitations](https://developers.openai.com/siwc/token-sharing-open-source/preview-limitations)
distinguish stateless HTTP from same-authenticated-connection WebSocket state.
This is a Relay transport policy, not a claim that OpenAI has no HTTP prompt
cache. Conversation state and prompt caching are separate: HTTP requires full
history, while a retained socket can reference its own completed responses.
The restriction avoids an automatic switch to stateless full-history generation
after a socket failure, reducing the risk of unnecessary context reprocessing.
Provider-reported cache hits remain observed usage; the transport guarantees
neither cache hits nor quota savings.
Native replay and buffered responses preserve completed reasoning ciphertext
from `response.output_item.done`, including when the terminal snapshot omits or
replaces it. The shared snapshot policy also serves protocol conversion; item
index/identity must match before opaque state can be reused. See the
[Responses item contract](https://developers.openai.com/api/reference/resources/responses/subresources/input_items/methods/list).
The client WebSocket `generate` option enters the shared request policy as
`prompt_cache_options.prewarm`. The upstream socket maps explicit prewarm to
`generate: false`, preserving other cache controls and allowing a completed
zero-output warmup to establish a same-socket continuation checkpoint.
An ordinary generation can continue that exact prewarmed input with an empty
delta; it does not require inventing a new user item or resending the prefix.
Explicit `prewarm: false` matches the ordinary-generation default for comparison
only, while its wire value and all cache-lifetime/mode controls remain intact.
Only an explicitly requested prewarm accepts a completed zero-output response;
provider failures, missing terminals and empty ordinary generations remain errors.
JSON and SSE clients retain the provider's warmup usage and can continue using
the same owner-validated local history. Relay never prewarms ordinary turns by itself.
See [WebSocket warmup](https://developers.openai.com/api/docs/guides/websocket-mode)
and [Responses cache prewarm](https://developers.openai.com/api/docs/guides/prompt-caching).
Unsupported semantic options/tools are rejected, not stripped. Text route admission
checks SIWC controls before reserving a candidate. SIWC automatically omits a valid
client output-token cap, including caps translated from Chat Completions, Messages
and Gemini, because the SIWC preview does not support `max_output_tokens`.
There is no compatibility setting; the provider controls the actual response bound,
so a response may exceed the client's requested cap. A null output-token limit
means no client cap and is omitted without an omission diagnostic. Malformed caps
and other unsupported controls still fail validation; model and context checks
remain unchanged.
Native Codex, BPS and API-source preparation is unchanged; BPS already omits this
unsupported field as part of its transport normalization. Diagnostics mark actual omission as
`requestPreparation.siwcOutputLimitOmitted`. Continuation ownership and complete replay are validated before
dispatch. A SIWC-only incompatibility returns a local error
without generation, quota charges, or an account health penalty. SIWC compaction
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
Client WebSockets use the shared request executor, which opens SIWC upstream
WebSockets and preserves the client's lane identity. An initial SIWC/BPS handoff retains the selected account and its
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
cards can explicitly associate Codex, SIWC and Excel/BPS connections.
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
The user must select the intended workspace during authorization. Explicit method
selection can join an existing group when every saved grant's protected identity
is compatible, including older SIWC registrations of the same account. All grants
remain saved; an authenticated grant is preferred over a registration requiring
sign-in. Native workspace/principal checks apply to every member, so SIWC cannot
bridge incompatible native connections. Connections are not grouped merely by
editable email or display label. Each route keeps its models, quota,
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
SIWC cards can show reset credits from a usable, explicitly linked Codex/BPS
connection, independently of that connection's quota windows. The newest known
count wins, including zero. Manual reset confirmation identifies the native
connection; redemption and verification use its account ID and existing fences
without changing the selected SIWC route or copying counters into its record.
SIWC app limits remain separate. Native reset HTTP rejects SIWC credentials
before constructing a request; a standalone SIWC grant does not provide reset
credit availability or direct redemption support.
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
The existing top-left card indicator warns for a linked reader's fresh zero
primary/secondary window or stale data, without an extra visible quota label.
Its tooltip names the reader and distinguishes exhaustion, including seven-day
windows, from stale observations. Authorization errors, disabled and recovery
states keep priority. These display states never change SIWC admission;
its own reported limit and runtime quota wait remain authoritative.
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

Catalog rows require a nonempty string identity: `id` in the
[OpenAI](https://github.com/openai/openai-python/blob/main/src/openai/types/model.py)
and [Anthropic](https://github.com/anthropics/anthropic-sdk-python/blob/main/src/anthropic/types/model_info.py)
model types, or `name` in the
[Gemini Model](https://github.com/googleapis/googleapis/blob/master/google/ai/generativelanguage/v1beta/model.proto)
resource. Existing compatible `id`/`name` aliases remain supported. An invalid row
rejects that binding's catalog rather than producing a partial or empty inventory.
A genuine empty list remains a successful read; well-formed Gemini models without
`generateContent` support remain outside its generation inventory. Failed refreshes
retain stored models, bindings, prices and capability evidence.

Source discovery collects all pages of each binding before accepting its
inventory, prices or endpoint evidence. Messages uses `last_id`/`has_more` and
`after_id`, including the [official SDK's optional-flag fallback](https://github.com/anthropics/anthropic-sdk-python/blob/main/src/anthropic/pagination.py).
Gemini Models, also used by the Interactions catalog, follows `nextPageToken`
through `pageToken` under the [Models list contract](https://ai.google.dev/api/models#method:-models.list).
Its empty protobuf list may omit `models`; an error object is not an empty list.
All pages share a 4 MiB body budget and 30-second deadline per binding, with
at most 128 pages and 8 KiB per cursor. Repeated cursors and unsupported pagination
contracts reject the affected read instead of publishing truncated evidence.
Credentials, protocol headers and the configured origin remain unchanged;
cursors are encoded query values, never redirect URLs. Retry-After from later
pages remains part of the refresh result.

Mixed-source refreshes apply complete binding observations independently. An unread
scope retains only its previously known inventory and catalog evidence; a failed
empty binding stays retryable without inheriting another binding's models.
A successful read of the same upstream protocol supersedes stale alias inventory.
Prices survive only for wholly unread models, not as a fallback for missing or
conflicting fresh prices on overlapping models. Previously known route features
and limits survive absent fields on unread scopes; explicit fresh fields and
complete scope reads replace their own prior evidence. Operator exclusions remain
authoritative. Source creation and refresh retain an error status with a redacted
protocol/HTTP summary for partial results, so monitoring retries instead of marking
the entire catalog fresh. A later valid empty scope clears its old observations.
Model-only legacy getters use the same inventory retention without granting routes.
Root-to-`/v1` resolution rechecks earlier reads at the canonical path using their
remaining read-time budgets; a partial result cannot relocate the source or
combine evidence from different API roots.

The renderer preserves Rust's resolved routes, including empty model lists.
Only an unresolved sole legacy native binding may inherit the source catalog.
Invalid adapters, duplicate routes, unavailable model references, and cache TTLs
on non-Messages routes cannot become native-launch or pricing capabilities.

Reference metadata supplies model identity, limits, modalities, tools, reasoning,
service tiers, and native-protocol policy. Declared, confirmed, unsupported, and
unknown evidence stay distinct. Missing limits/capabilities remain unknown;
participant metadata does not become universal model truth.

The selected endpoint's generation controls use one typed projection in runtime
preparation and client catalogs. Function-tool, tool-choice, structured-output,
streaming, reasoning and interleaved-system flags match the merged feature map.
An explicit deny from either the reference or endpoint remains authoritative;
declared/confirmed feature evidence may fill unknowns only for that exact route.
Disabling function tools also excludes parallel calls. An evidenced effort list
can establish route-local reasoning support, but does not invent a default effort
or resolve a disjoint reference/route list by choosing a weaker level. The shared
reference and protocol-specific media contract are unchanged.

Pool catalog routes aggregate endpoint evidence only within the same model,
client API and upstream API. Each eligible member contributes its own reference/
endpoint projection; available alternatives union features and valid effort sets.
A feature is unsupported only when every eligible alternative excludes it;
unknown alternatives remain distinct from positive evidence. Disabled, draining,
out-of-pool, credential-unavailable and model-excluded members cannot revive a
capability. Empty resolved effort intersections do not fall back to reference
levels. These snapshot-local observations do not change model-wide metadata or
grant a physical route, and only the projected fields enter client catalogs.

Each projected protocol route carries the most restrictive known token ceilings
among its eligible alternatives. Limits stay scoped by model, client API and
upstream API; missing values remain unknown. They do not replace Model Rules'
reference limits or grant a route. OpenCode constrains its selected SDK group's
context/input/output with these ceilings while preserving smaller custom windows.
Sparse renderer catalog merges retain the same route tuple and existing known
ceilings, filling missing numeric fields without mutating the source summaries.

OpenCode's [generation path is streaming](https://github.com/anomalyco/opencode/blob/810c0eba1a7348036f8bdf61e6632d0c36c6876c/packages/opencode/src/session/llm.ts#L276-L290).
Its initial pool connection and catalog refresh therefore use a fresh streaming
view of the same endpoint observations: explicitly non-streaming alternatives are
removed before aggregating features, efforts or numeric limits. General inventory
and pool policy fields remain unchanged. Pooled profiles fill unknown generation
controls from the selected SDK client's resolved alternatives; reference denials
remain authoritative. Explicit effort lists honor the raw saved policy captured
with the same snapshot, even when Model Rules has no reference enum. Empty or
disjoint effort sets stay empty and do not create a default. Both snapshot modes
perform their projection within the bounded worker. Direct connections choose only
native, stream-capable SDK bindings; a non-streaming preferred binding cannot hide a usable
alternative. Unsupported SDK protocols and explicit streaming denials cannot return
through refresh. A new connection with no compatible streaming model fails before
replacing the client configuration; refresh can remove stale managed model rows.

The live Codex catalog uses the requesting key's Responses routes that do not
explicitly exclude streaming, because
[Codex requests streaming Responses](https://github.com/openai/codex/blob/4980f9585a0a2dea0cdc4bb0cee4e631b75fcf5c/codex-rs/core/src/client.rs#L1166-L1172).
Models without such routes are omitted from its picker, not generic inventory.
That same route set supplies API context bounds, efforts, service tiers, and
image/parallel-call restrictions. A feature is removed only when every remaining
alternative explicitly excludes it. This projection preserves other modalities
and native-card ownership; endpoint claims cannot grant image or parallel support
absent from the existing reference/native projection. Managed reasoning-placeholder
normalization resolves the key's model prefix and uses the request's streaming mode
before checking whether the applicable routes expose effort controls.

Direct Codex API profiles use the exact Responses endpoint's controls, effort
intersection, numeric ceilings and service-tier restrictions. They exclude explicit
streaming denials and retain independently declared non-image modalities. Generated
pool and direct catalogs carry their scoped numeric ceilings in Relay's existing
`relay_context_limits` metadata; Desktop context-policy projection preserves these
limits instead of replacing them with model-wide input/output maxima. Native cards
without an evidenced window do not acquire one from that transfer metadata.

An authoritative successful empty catalog refresh removes stale rows from the
Relay-owned managed catalog using its normal binding, ownership and atomic-write
checks. Malformed or failed reads preserve the previous catalog. Initial attachment
still requires compatible models; empty refreshes leave profile authentication,
configuration and recovery snapshots intact, and a later valid catalog can recover.

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
Discovery accepts the published `endpoints` path list alongside its existing
endpoint aliases, including OpenRouter's `/api/v1/responses`,
`/api/v1/chat/completions` and `/api/v1/messages` paths. Earlier explicit fields
and Gemini generation-method declarations retain precedence. Unrecognized paths
grant no protocol; endpoint declarations grant no tool, reasoning or cache feature.
The configured API origin/prefix remains unchanged. Contract:
[OpenRouter model endpoints](https://openrouter.ai/docs/guides/overview/models#endpoints).
Exact negative evidence and an endpoint pin remain authoritative. Retained
endpoint and feature exclusions win over later positive observations, including
after reload. Complete discovery replaces Catalog-origin evidence only; manual
exclusions remain until removed or the source identity is invalidated.
Feature-only Catalog or Manual observations do not establish endpoint support.
Missing native support uses an available declared upstream through the shared
adapter; it does not silently remove requested features or emulate search as a
client function.
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

HTTP, account-only, compaction, Images and native WebSocket usage retain the exact
OAuth authority revision as request-local provenance, including quota headers.
Equal bearer text or generation numbers do not let a replaced registration adopt
an earlier response. Unknown OAuth provenance remains telemetry-only. The Desktop
writer acquires the existing credential-writer lock before the revision guard and
holds both through secret/account/wake persistence; busy or retired ownership does
not change account state or schedule account-refresh activity. Source/key usage and
request telemetry remain recorded. Revision handles never enter persisted or
exported request records.

Runtime OAuth preparation failures retain the exact authority revision observed
under the token-slot lock. Health changes hold that revision's guard, including
invalid-header rejection. If a registration is replaced during renewal, preparation
may read its replacement once before dispatch; repeated replacement returns a
transient preparation failure. Replacement expiry checks include elapsed renewal
time rather than reusing the preparation's initial timestamp. The retired refresh
is not repeated and its result
cannot mark the new login unhealthy or require its reauthentication. Terminal
grant failures on the current registration retain their existing sign-in requirement.

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
  Recovery from exhausted windows uses the latest reset among all zero windows.
  If any exhausted window has no reset, the recovery deadline remains unknown;
  another window's earlier reset does not imply renewed capacity.
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
  A JSON 404 from the optional `subscriptions` lookup preserves the authenticated
  account metadata and unknown expiry as a partial observation. Its status and
  normal retry floor remain intact; it is an informational diagnostic rather
  than an account error. Primary-check failures, challenges and other HTTP
  refusals retain error diagnostics.
  The documented `cf-mitigated: challenge` response header adds a fixed
  `cloudflare_challenge` diagnostic; absent or unknown values remain unreported.
  HTML alone does not establish the refusal's origin. See Cloudflare's
  [challenge detection contract](https://developers.cloudflare.com/cloudflare-challenges/challenge-types/challenge-pages/detect-response/).
  Subscription diagnostics include the OAuth client kind, total probe elapsed
  milliseconds (including management admission and optional retries), and a
  validated Cloudflare Ray id when present. Arbitrary header values are omitted.
  Fixed `transport_failure` categories distinguish request/body timeouts,
  network/body failures, management admission and stale ownership fences.
  Stale scopes and invalid origins are not retried; transient failures retain
  their bounded retry policy. Raw transport error text is never retained.
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
  load and discovery. Catalog HTTP failures, including 404, malformed catalog
  replies and local catalog-preparation failures warn without marking the whole
  account unhealthy. Saved `unhealthy` records owned only by those catalog
  failures recover to `degraded` on load or discovery; actual authentication,
  verification, account blocks and quota restrictions retain precedence.
  The catalog error remains until a successful accepted read. HTTP 404 has a
  distinct management code and localized explanation; legacy generic HTTP
  errors do not invent a missing status. Native Codex/BPS empty reads retain
  their last good inventory. A successful
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

Explicit cache keys remain stronger than session hints. Nonblank provider keys
are opaque: leading/trailing bytes remain distinct in local affinity, matching
the unchanged upstream value. Whitespace-only values cannot create a binding.
Observed Claude Code
child/parent metadata can share soft parent affinity while keeping each child's
context separate. Without an explicit key or session, HTTP and client WebSocket
requests may derive a caller-scoped hint from the exact initial instructions and
opening user turn. This bounded (64 KiB), process-salted hint requires at least
32 bytes of user text and uses the existing model-scoped scheduler bindings.
It stores no prompts, creates no provider cache key, and grants no continuation
ownership. Arbitrary compacted tails without the opening turn are not inferred
to be the same conversation; stable client/root hints remain necessary there.

Automatic weekly reset redemption defaults to a separate reserve policy. It never
deprioritizes an account merely for owning a reset. Desktop consults a read-only
projection of ordinary quota on compatible account routes; missing/stale quota
is unknown, busy capacity still preserves the reserve, and paid-credit/API
fallback is not ordinary subscription capacity. Redemption requires a successful
fresh observation of an exhausted weekly window and a current enabled rule.
The existing quota cadence bounds expiry urgency and the natural-reset waiting
window. Fresh reset inventory is checked before POST; unknown inventory never
becomes a zero or a positive grant. A reset expiring before the next check can
restore an exhausted week, unless natural recovery comes first. Manual actions
retain their explicit user-controlled behavior.
Fresh paid-subscription expiry can also release this reserve when the entitlement
ends within a weekly window and before natural weekly recovery. The account must
still have an enabled matching rule and freshly confirmed weekly exhaustion;
inventory verification, credential fences and ambiguous-redemption protection
remain unchanged. A reset count is never treated as spendable quota in advance.

Weekly rules can instead opt into a remaining-quota threshold, stored as integer
basis points from 0 to 10,000. Existing rules and new forms leave this mode off.
Blank or 0 means exhausted only; 100% never redeems a completely full window.
Only a successful fresh general secondary weekly observation with a future reset
date can trigger it, never a primary or supplemental/model-specific window.
Threshold mode is independent of other accounts' ordinary capacity. Both modes
reuse the credential fences, fresh reset inventory and redemption coordinator.
Threshold dispatches persist an unresolved marker before POST. A fresh full
weekly ledger confirms recovery; ambiguous outcomes block further automatic
redemptions across restarts. A confirmed threshold reset is not repeated before
the originally observed weekly deadline, including after rule edits or drifting
relative reset values. The private consume endpoint chooses the redeemed credit;
Relay does not invent a client-side credit-selection contract.

Image-generation/edit response headers never update the Codex text-quota ledger.
Account model manifests are scoped to the authorization incarnation, subscription
plan and inventory revision that produced them. Late or stale manifests cannot
restore metadata from replaced credentials or a previous entitlement state.
Reauthentication retains operator settings and last-known quota before probing,
but its final settings merge preserves the new grant's quota result.

Request transport diagnostics keep observed upstream HTTP status separate from
the logical result status and, for returned HTTP streams, the constructed client
response status. These are not delivery acknowledgements. Connection failures
come from transport evidence, upstream failures from saved response evidence,
and local failures from their owning processing boundary. Missing evidence stays
unknown; status codes and free-form error text alone do not establish origin.

One redemption coordinator covers manual and automatic sends through the
post-reset quota verification. Persisted per-cycle fingerprints prevent a second
automatic redemption while a consumed reset's weekly recovery is unconfirmed,
including after restart. The cycle is recorded before POST; transport failures
and ambiguous server outcomes retain that fence, while definitive client
rejections release it. A proven 401 rechecks the reserve policy after renewal.
Quota exhaustion edges dirty other deferred weekly
checks through the existing refresh service; they do not introduce polling or
repeat a generation. Auth/config fences and rule eligibility are rechecked at
the action boundary. The provider owns actual restoration and reset dates.

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
For independent Automatic work, fresh active paid-subscription deadlines apply a
bounded 1–8x multiplier to the configured request share after load and health
gates. The signal compares current positive quota headroom with time remaining
over a seven-day horizon; it is not a demand forecast or a promise to exhaust all
saved resets. An urgent member can remain in the resource group despite lower
quota than ordinary alternatives. Primary affinity, ready warm alternatives,
opaque owners, manual modes, capacity, provider waits and unknown/stale evidence
keep their existing behavior. Subscription updates replace this soft evidence
without a restart; removal clears it. Deadline freshness uses the existing
subscription refresh interval, independently of OAuth expiry and quota headers.
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
HTTP SSE observations also separate the first nonempty upstream text delta,
first native function/custom call activity, and first complete native tool item.
Native item completion requires a done item with its IDs and string arguments/input;
it is not validated client-call readiness or evidence that a whole batch is complete.
Terminal snapshots do not invent these boundaries. Internal discovery generations
reset them, old records leave them absent, and no tool content is retained.
With diagnostics enabled, HTTP SSE and native WebSocket responses also retain
the first typed reasoning item/part or nonempty reasoning delta separately from
TTFT and first output. Empty reasoning structures establish activity, not visible
text, completion or retry eligibility. Missing observations remain absent; each
WebSocket turn and internal catalog generation owns its own observation.
Successful streaming/account HTTP responses with missing or mislabeled SSE MIME
types use a bounded 512-byte prefix probe. Recognition stops at an SSE field;
consumed chunks, response metadata, JSON bodies and body errors are preserved.
The existing BPS progress guard also covers probing. Undecoded encodings are not
sniffed, and structured-output validation retains its intentional buffering.
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
small resource differences leave the owner unchanged. Each lease captures the
prompt-binding revision at admission; a late completion cannot replace a newer
owner, even if resources changed again while it was running. Completion on the
current owner can still refresh its expiry. This fence affects affinity writes,
not dispatch eligibility or mandatory opaque-response ownership. A late successful
spillover may still become a volatile warm alternative without replacing the
newer owner or extending its expiry.
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
use fresh resource headroom in automatic mode and saved order otherwise. Among
waiting cards, a fresh exhausted secondary window follows temporary primary-window
waits, even when its retry time is earlier or runtime timing is absent. Stale or
unknown counters do not establish that category. Retry time orders members within
the category; this presentation does not change saved routing or admission.
Quota waits, retry cooldowns and occupied capacity keep
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
Weighted credit/debt is bounded to 32 current eligible physical-pool rounds,
both when choosing and when advancing a reservation. Soft sticky work still
contributes to ordinary weighted accounting without accumulating unbounded debt
for later independent work. Pool/weight changes use the current bound; equal
round-robin, hard ownership and recovery admission retain their own rules.
Failover selects the first ready member in saved priority order, without cycling.
A due lower-priority recovery route does not displace a healthy primary; a due
primary recovery still follows the bounded recovery admission policy.
A member limit covers all its protocol routes; zero means no extra member cap,
not unlimited provider capacity. Policy saves are serialized and identity-checked.

Rotation policy version three is advertised as `rotation_v3`. Known version-one
and version-two policies upgrade without changing inventory, member order,
weights, concurrency limits, or their equal-cycling semantics. Stored `in_order`
is read as `round_robin` and normalized on startup; `weighted_round_robin` explicitly opts
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

The renderer and IPC use only version-three policies; the native store owns
old-policy migration. Unsupported runtime versions remain non-editable.
Runtime strategies use explicit names: Automatic, RoundRobin,
WeightedRoundRobin, and Failover. `in_order` exists only as an input alias,
not a separate runtime mode. Round-robin cursors
survive member changes while their route remains configured; removing the last
route discards its cursor, including during model-inventory replacement.

Admission queues, waits, and generation attempts are bounded. One logical
request owns its dispatch, transport, auth-repair, retry, and irreversible-send
budgets across HTTP, SSE, WebSocket, and account paths. Retries require:

- a proven not-sent outcome or pre-execution rejection;
- repeatable input and valid continuation ownership;
- remaining request budget and no client-visible response bytes;
- provider Retry-After/reset deadlines respected without shortening them.

HTTP `Retry-After` accepts unsigned decimal seconds or a complete HTTP date under
[RFC 9110](https://www.rfc-editor.org/rfc/rfc9110.html#section-10.2.3).
Local recovery uses the longest individually valid field if an upstream repeats
the singleton, independent of field order; invalid fields do not mask valid hints.
Unrepresentable decimal delays saturate before the owning policy applies its
existing bounds. This conservative recovery floor does not authorize forwarding
ambiguous fields, widen failure scope, or make an uncertain request safe to retry.
Connection-local hints may delay Relay's own upstream connection without being
forwarded to its client. Token-count errors use the same singleton and connection
filters as other HTTP responses, retaining their redacted body and no generation
health or usage effects.

Each dispatched OAuth lease retains the revision of its latest actual send.
Authorization repair replaces that handle under the same request-budget and
credential guards as the dispatch debit; a rejected dispatch leaves it unchanged.
Settlement and cancellation retain the original execution certainty and release
capacity even after reauthentication. Only a still-current revision can update
rotation circuits, mandatory/shared cooldowns, first-output latency samples or
recovery credits. The revision guard spans the scheduler mutation and its
observation callback; replacement cannot interleave between validation and commit.

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
`409 response_continuation_unavailable`. Pre-dispatch continuation rejections
over HTTP, account-only compaction, and WebSocket record one Relay-local failure
without an upstream attempt, token usage, selected route, or account penalty.
WebSocket accounting covers the first and later requests, including HTTP-bridged
connections. The resolved model is retained when already known; diagnostics
exclude response IDs and tool results.
Compaction checkpoints explicitly
replace the predecessor; they are not discarded to force a retry.

Desktop routing-graph replacements reuse the host's bounded volatile adapter
and native replay stores together with route fingerprints. The runtime retains
these shared handles and its agent-message cipher in one `RuntimeContinuationStores`
owner; each store keeps its existing lock and lifetime boundaries. Late completions
write into the same stores; ownership checks still reject another key, model,
endpoint, or API-source credential. A new host/process starts empty: conversation state
is not persisted to recover a restarted application.

Graph-local cache comparisons, Codex turn-state observations and BPS catalog
preload hints belong to `RuntimeSessionState`. Policy-only updates retain this
owner; graph replacement starts fresh observations without moving late writes
from the retired graph. Each store retains its own locks, bounds and expiry;
host-owned continuation stores remain separate.
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
Terminal gateway errors flush before a bounded close handshake. Pending control
frames cannot force an immediate TCP drop that discards the error. Closing does
not dispatch queued input, replay a generation or retain a finalized request lease.
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

Native account sockets retain whether the first prepared request declared hosted
web search, including non-generating prewarm requests. First activation on a
socket opened without search reconnects before dispatch. A connection-bound
`previous_response_id` is replaced only with locally retained native history on
the same owner; missing history produces an explicit continuation error instead
of moving the opaque reference. Explicit current tool catalogs override historical
discoveries. API-source sockets, BPS and SIWC keep their existing transport rules.

While an HTTP/SSE fallback or connection setup owns a pending response, a valid
`response.interrupt` with `mode: discard_partial_items` receives
`response.interrupt.failed`, preserving its response ID and active stream lane.
This rejects the unavailable control operation while the original request keeps
delivering its actual output, terminal and usage. Client disconnect still cancels
the owned transport and releases capacity.

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
that provenance. Observed state from HTTP errors and rejected WebSocket handshakes
retains the same scope as successful responses; observing it does not authorize
another dispatch or bypass a credential refresh. Soft root/thread and prompt-cache
hints are separately bounded and namespaced; they neither authorize state nor
transfer an opaque owner.

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
Shared and account-only preparation carry typed adapter failures to the execution
owner; an immediate rejection or exhaustion of incompatible candidates records
one local failure, while a successful compatible fallback records no local rejection.
Client cancellation has its own non-successful UI outcome. Historical synthetic
HTTP 502 cancellation rows receive the same label.
New client-cancellation telemetry uses HTTP 499 for both HTTP and WebSocket
requests, preserving reported consumption and the client-disconnected outcome.
The same normalization covers cancellation before stream ownership is handed off
and finalization during a recovery wait. Known partial consumption stays recorded;
missing consumption remains unknown. Cancellation neither invents a send nor
changes a preceding provider rejection into a client error on its physical attempt.
A logical cancellation does not inherit the previous attempt's provider error;
that rejection remains on its physical attempt, with consumption unchanged.
Account-only compact, search and wake execution share the same request lifetime
and dispatch budget. Cancelling while waiting for upstream headers or a buffered
body retains the pending physical send once, with unknown consumption left absent.
Completed buffered responses retire this owner before client body consumption;
dropping that completed body does not create another cancellation or usage vote.
Valid native SSE usage observations amend only the current pending physical attempt
while account, BPS and discovery responses are buffered, and while streaming
responses await bootstrap before client handoff. This accounting remains active
when request diagnostics are disabled. Reported counters survive cancellation
without another emission,
clock reset, dispatch or account-state vote. An observation for an older attempt,
an absent send or a retired request is ignored. Unreported fields remain unknown;
partial JSON text is not interpreted as reported usage. Native usage snapshots
retain cache-write input counts alongside cache reads, output and reasoning counts.
Admitted text-generation responses carry `x-zenith-relay-request-id` for exact
correlation with the logical history record in both buffered JSON and SSE. This
Relay-owned header does not replace the provider's own request-ID headers or alter
the response body. Account-only compact, search and wake responses use the same
correlation header and request identity.

Relay owns HTTP protocol retries, with reqwest's internal retries disabled.
HTTP/2 refusals follow the configured failure policy, candidate limit, provider
cooldowns and uncertain-execution rules.

The explicit `--diagnose-http2-retries` mode qualifies the compiled client and
normal gateway dispatch against bounded loopback HTTP/2 prior-knowledge peers.
A default-client positive control exposes protocol-level resends; gateway cases
compare physical streams with retained usage attempts and one logical request,
including uncertain resets and HTTP 429 candidate limits. It runs before Desktop
storage or single-instance initialization, uses only synthetic credentials and
exports aggregate observations. The [offline check](../../CONTRIBUTING.md#offline-http2-transport-check)
owns invocation and exit codes. Source-level regressions additionally negotiate
certificate-verified TLS with `h2` ALPN, directly and through configured
source-specific HTTP CONNECT proxies. They compare physical streams with usage
attempts for refused and uncertain resets, HTTP 429 failover and candidate limits.
The positive control exposes default-client resends, while an unavailable proxy
cannot bypass to the direct TLS peer. Certificates, routes and keys are synthetic
and in-memory; the executable diagnostic retains its prior-knowledge scope.
Provider/proxy combinations and installed-copy acceptance retain their own
evidence requirements.

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

`ZENITH_RELAY_IMAGE_HEADERS_TIMEOUT_SECONDS` (1–1800) can extend an enabled
general header wait for image generation/edit endpoints and prepared Responses
requests that permit the native `image_generation` tool. Disabled or more
restrictive tool choices, historical tools, model names and prompt text cannot
enable this policy. An unset, invalid or shorter image value retains the general
limit; an image value alone does not enable a timeout. The selected request
budget keeps the original deadline across repair and handoff, even when a later
attempt requests a longer wait. Response-body and SSE progress limits are unchanged.

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
whitespace/escaping. Allowlisted repeated HTTP headers retain their value bytes
and per-name order. Both directions exclude credential fields and fields named by
any `Connection` option, including a value containing unrelated opaque bytes.
Responses omit duplicate singleton content-type, retry and request-identity fields;
JSON/SSE factories keep their own content type. Forwarding follows
[HTTP field ordering](https://www.rfc-editor.org/rfc/rfc9110.html#section-5.3)
and [connection options](https://www.rfc-editor.org/rfc/rfc9110.html#section-7.6.1).
Ambiguous client state headers remain excluded. Native SSE/WS frames
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

Structured context overflow requires HTTP 400 or an error event without an
embedded HTTP status; explicit authentication, throttling and server statuses
retain their own classification.
`invalid_access_program` and `unsupported_access_program` are terminal 400
request errors; `access_program_not_enabled` is a terminal 403 request refusal.
Status-less SSE/WS events share these categories. These program refusals neither
repair history, retry or rotate accounts, nor affect account health or cooldown;
the requested model/mode is never silently downgraded.

Structured provider `budget_reached` is a terminal session-budget refusal (400),
distinct from account quota. HTTP requires status 400; SSE/WS also accept an
error without an embedded HTTP status. Explicit 401/403/429 retain their own
classification. The first terminal ends the response, retaining known usage;
later completion events and retry hints cannot revive it. History repairs,
rotation, cooldown and account penalties do not apply to this refusal.

An explicit 403 `invalid_workspace_selected` is terminal for the request. The
structured `detail.code`, then `error.code`, takes precedence; only an absent
body code permits the bounded, validated `x-error-json` identity hint. Ambiguous
or duplicate-key envelopes and prose alone do not establish this failure.
Relay returns it without history repair, candidate rotation or account cooldown.
Transient authorization refresh failures preserve the existing revision-bound
credentials; this workspace refusal does not turn them into a generic expired login.

Messages identity checks include the `message_start` model, server-side
`fallback` block boundaries and `usage.iterations` entries of type `fallback_message`.
Diagnostics retain the last validated serving declaration in
`upstreamResponseModel`, without changing the requested/resolved route.
A fallback handoff proves execution even without text or token counters, so
rejecting a substituted model cannot replay the generation on another member.
Relay does not inject a fallback policy or enable model substitution. The wire
reference is [Anthropic refusals and fallback](https://platform.claude.com/docs/en/build-with-claude/refusals-and-fallback).

Native WebSocket ingress starts serialized Ping heartbeats immediately after
upgrade, including the wait before the first request. The 30-second heartbeat
does not extend the fixed 60-second initial-request deadline, dispatch work,
or consume quota; closure ends that loop without a detached writer.
Native WebSocket forwarding checks message size, served model, late events,
usage/replay and safe errors. Committed output disables inapplicable retry parsing,
not served-model validation. Converted text, reasoning, refusal and argument
snapshots change only after append-only/identity checks and successful emission.
Closed Messages blocks reject mutations; deferred Gemini and tool-search calls
stay behind their output barriers. Numeric normalization preserves exact values
and does not release unfinished arguments. Service tiers, effort and cancellation
do not change as a side effect of parsing optimizations.
Outbound text requests larger than 1 MiB use masked text/continuation frames with
payloads of at most 1 MiB, retaining one WebSocket message. Each frame awaits
backpressure within the existing single write deadline. A partial or timed-out send remains
execution-uncertain and is not replayed. The reqwest client still owns proxies,
TLS and HTTP upgrade; the native frame layer verifies the handshake and rejects
unrequested extensions and subprotocols.
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
Optional `reauth` marks a new native WS attempt whose authorization differs from
the last successfully opened socket for the same candidate in this client session.
Both the credential fingerprint and login incarnation participate; failed
handshakes keep the previous successful baseline. Initial and same-auth opens
record false; reused turns and HTTP leave it absent. Diagnostics expose only
the flag, never credential material or its fingerprint.
These fields share existing routing JSON storage and add no retry or transport policy.
Optional `codexServerTiming` records allowlisted numeric native
`responsesapi.websocket_timing` values separately from Relay timings: backend
overhead, inference, engine TTFT and inter-token intervals. They are not additive
phases, cache-hit evidence or end-user latency. Missing/invalid measurements stay
absent. Only an active response may receive a measurement; reused connections
require an exact response ID, while an ID-less event needs the sole first response.
Concurrent measurements use their exact response owner and any supplied lane
must agree. Timing notifications bypass per-response setup buffers and forward
unchanged without supplying usage, semantic output or generation commitment.
They cannot disable a safe pre-output repair. Unmatched and late events never
attach to the next response. Raw events and IDs
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

- Responses HTTP recognizes the observed Codex Desktop Companion Page envelope:
  an `agent`-namespaced `function_call_output` containing only `type`, `namespace`,
  `name`, and `output`, with a valid open-page payload or numbered snapshot header.
  It preserves the exact text and item order as user input before continuation
  validation. Call identities, extra envelope fields, malformed context and
  duplicate JSON members remain outside this compatibility rule. The same
  bounded body-reading workers handle identity, gzip and zstd requests; other
  client protocols retain their original inputs.
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
  Messages routes with explicit `interleaved_system` support instead retain
  text-only system messages at their original positions, including after tool
  results. Each consecutive system group follows a user turn and precedes an
  assistant turn or ends the input. Leading instructions retain the top-level
  system field. Reference and exact-route exclusions remain binding; model names
  and generic Messages support never enable this feature. Nonleading developer
  roles, invalid placement and unrepresentable content remain explicit errors.
  This capability does not extend Gemini's roles or qualify live provider support.
  Converted Messages input validates its own system contract before lowering
  tool changes: consecutive inline messages form one placement group, following
  a user turn or an assistant turn ending in a supported server-tool result.
  After the first assistant turn, the group ends the input or precedes an
  assistant turn. Existing text-only client startup attachments before that boundary
  retain their separate instruction/user order; this exception does not admit
  misplaced tool changes. Root instructions use the top-level field. Root and
  plain inline sections use Messages text blocks; supported tool changes retain
  their separate lowering. Null/empty inline content cannot disappear during
  conversion. Native requests remain pass-through.
  Nullable aliases cannot erase explicit limits;
  conflicting non-null limits fail before transport.
- Preserve call kind, namespace, declared name, item ID versus `call_id`,
  parallel grouping, arguments, and results. Only argument deltas concatenate;
  repeated identity fields do not. Tool names retain their declared identity;
  new calls require distinct nonblank call IDs, preserved without normalization.
  The shared conversion boundary rejects whitespace-only IDs in history, JSON,
  and streams. Malformed Chat call lists, mixed legacy/current call fields, and
  calls attached to tool-result messages fail instead of being silently discarded.
  Responses history items validate a supplied non-null `status` against the
  documented `in_progress`/`completed`/`incomplete` enum before conversion;
  malformed values report `input.status`. Native forwarding remains unchanged.
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
- Explicit `max_output_tokens` remains client-owned for native full/Lite
  generation over HTTP and WebSocket. An omitted limit stays omitted; an upstream
  refusal is returned through the existing error policy, without resending the
  request after dropping its limit. Legacy native `/responses/compact` omits
  generation limits independently of normal request preparation.
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
- Messages `thinking.display` supports `summarized` and `omitted` on enabled
  or adaptive thinking. The display policy does not change effort or budget.
  Omitted summaries produce no thinking blocks/deltas; visible SSE block indices
  remain contiguous. Hiding applies only to the public projection. Portable Chat
  reasoning and original native output stay in scoped continuation state for the
  next tool-result turn, without fabricated client signatures. Native Messages
  retains its wire policy.
  Explicit summarized display requests Responses `reasoning.summary: "auto"`
  or Gemini `includeThoughts: true`; Chat has no portable summary-generation
  control and rejects that explicit request instead of discarding it.
- Gemini `generationConfig.thinkingConfig.includeThoughts` uses the same display
  policy without changing its exact level/budget or enabling thinking. False
  hides only client output; null/absence keeps provider defaults. True alone can
  request a Responses summary without inventing effort. Messages needs an
  already representable enabled/adaptive thinking control; otherwise explicit
  summary generation fails with the original Gemini parameter. Zero budgets and
  known non-reasoning models keep their disabled/default controls. Native Gemini
  retains the original flag. JSON/SSE tool continuations preserve full portable
  reasoning and original provider-owned state for both visible and hidden output.
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
  The exact `clear_thinking_20251015` edit with `keep: "all"` preserves the
  converter's complete history; actual clearing, compaction and unknown controls
  remain unsupported. This policy does not grant opaque-state ownership.
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
reason under a deadline. After upgrading and sending the first request, the native
WS bridge allows up to 16 active responses, including the default lane, on one
physical upstream socket. It accepts up to 32 distinct named lanes per client connection.
Each lane stays FIFO; independent lanes can overlap when their selected route,
model, headers, search mode and authorization permit reuse of that socket.
The first response's setup frames remain buffered until generation or a terminal
event, while other lanes can start before any upstream application event arrives.
That buffer is bounded to 1,024 frames and 128 MiB. Reported setup usage is retained
immediately, including on disconnect or overflow, without committing the request
for retry purposes. Reused lanes keep their existing setup-event delivery.
The bridge retains each lane's latest response identity, allowing cross-lane forks
without replacing the socket or losing the source lane.
Incomplete responses keep only volatile affinity on that connection. Replacing
or closing the socket clears all lane-local identities and volatile bindings;
it does not reset the client connection's named-lane count. A request-scoped terminal
failure preserves sibling requests and queued work, including on the default lane.
An identifier-free request error belongs to the active default lane, or the sole
active owner for legacy streams. Explicit connection-limit errors, and unscoped
errors without an unambiguous owner, end the connection and settle every owner;
ambiguous response output is rejected rather than assigned to an arbitrary lane.
Unscoped transport notifications forward without altering lane usage. Failed
same-lane continuations retire their cached parent; failed cross-lane forks
preserve the source lane's parent. Route, model and authorization checks still
govern reuse; portable replay remains the existing owner's responsibility.
Each active native response owns its candidate lease together with its request,
usage and replay capture; terminal settlement and cancellation release that
response's reservation. Explicit upstream lane and response identifiers are
checked before retries, accounting, model guards or client output, including
initial buffered events. An event naming an inactive lane or a different response
from that lane's identified active owner is ignored, including a late cached
parent arriving before another lane obtains its response ID. Events without a
lane can use a known active response ID or the active default lane. Identifier-free
legacy events remain accepted with one owner; ambiguous named-lane output is rejected.
Requests requiring a replacement socket wait for active owners to drain. Background
authorization renewal keeps reading active upstream responses and bounded client
input. Its unsent reservation is separate from dispatched generations; cancellation
releases that reservation while a started grant rotation finishes persistence.
Admission rechecks the current route and authorization before dispatch. Replacement
retains the request identity and budget, and rejects an opaque-only predecessor
before another upgrade when its authorization incarnation changed. A safe
pre-execution retry settles its failed attempt and retains its request identity
and budget while waiting for siblings; new work does not postpone that handoff.
Handshake and live pre-output rejection borrow one `ConnectRecovery` owner.
One-shot history and identifier repairs remain consumed across reconnects,
deferred retries and HTTP fallback. The typed HTTP handoff carries the current
repaired request, continuation ownership, reset permission, transport reason and
any existing candidate reservation. HTTP reconsiders transport-specific candidate
exclusions; physical attempts retain the original request identity and shared
budget. A slow optional handshake keeps its selected member and one rotation
step, while retaining source/account attribution on native-only reconnects.
Exhausting a pre-output retry reports its error without closing the client
connection, including on the default lane. A queued fork retains its original
parent's replay history when the source lane advances before dispatch; repair
does not adopt the source's newer branch. Known usage stays on the physical
attempt that reported it.
An unknown disconnect is not replayed. Disconnect settlement releases every
active owner's lease and retains its own observed usage.
One client-socket owner retains pending `response.create` frames in per-lane FIFO order
across native connection preparation, retry and HTTP fallback. The local queue
allows at most 32 requests and 64 MiB of retained wire data; these are Relay
resource limits, not provider concurrency limits. Queued requests are prepared
only after the preceding response in their lane finishes, so continuations resolve against
completed state and acquire their own current policy, budget and lease. Invalid
queued JSON, queue overflow and unsupported controls return a local request error
without cancelling the running response. Invalid initial or follow-up requests,
missing continuation state and incompatible HTTP-fallback lanes leave the connection
usable; queued valid requests retain their order. Initial validation errors do
not extend the first-request deadline. Native-mode HTTP fallback proceeds to the next request
after a pre-output rejection without replaying the failed request; failure after
partial output remains connection-terminal. Disconnect drops pending frames
without dispatching them; control frames remain outside the generation queue.
HTTP-only transport observations are bounded, expiring, and credential/route-scoped.

When upstream WebSockets are disabled at client connection time, the HTTP bridge
multiplexes independent client lanes over its existing HTTP request executor.
It allows 16 active requests, including the default lane, and 32 distinct named
lanes for the connection lifetime. The shared pending queue retains at most
32 requests and 64 MiB; busy lanes are skipped without reordering their own
requests. Each lane prepares its next request after the prior owner settles
usage and releases its lease. Completed HTTP continuation state can fork across
lanes under the existing key, route and authorization ownership rules; this does
not establish same-native-socket opaque continuation support. The connection
stays HTTP-backed if the upstream WebSocket setting changes while it is open.
Validation errors and late stream failures stay lane-scoped. One physical client
reader/writer owns all output and acknowledges writes before each lane advances.
Controls are bounded separately to eight frames per lane and 16 KiB per frame.
Disconnect cancels active owners, releases their leases and discards pending
requests without dispatching them. Native-enabled connections use the shared-socket
bridge described above; their automatic HTTP fallback remains sequential. A handoff
drains native owners first and preserves deferred logical retries and pending client
frames, including their request identities and dispatch budgets.

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
Responses conversion places the initial callable catalog before history in a
developer `additional_tools` item, then introduces deferred schemas once after
the discovering result or before a retained call when discovery history was
compacted. The explicit initial item also preserves unused initial definitions
through SIWC preparation. Forced tools without history load at the end. Requests
with inline replacements/removals retain their final root catalog authority.
Other converted APIs use their root callable catalog.
Relay preserves the discovery marker and does not search, execute tools, emulate
hosted search, or promise a provider cache hit.

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
setting without rebuilding the listener; each admitted request retains its
initial diagnostics setting. Forwarded tool catalogs are observed read-only
after conversion and before transport encoding. Historical tool-policy settings
remain readable but cannot filter tools or reject a new request.
The UI hides route, context, and tool diagnostic panels
outside debug mode, including those in historical rows; history is not erased.
Token/cache usage, pricing, total latency, TTFT/generation speed, outcomes,
attempt history, errors, and all authorization/schema/size guards stay active.
No debug setting changes routing, provider caching, or tool execution policy.

Desktop saves, configuration/route switches, and refresh commands record bounded
`command_save`, `command_switch`, and `command_refresh` samples in the existing
local performance store, independently of request debug mode. Each sample spans
the renderer's IPC dispatch through promise settlement and contains only the
static command name, `ok`/`error`, and elapsed milliseconds. Arguments, results,
account identifiers and error text are excluded. Recording uses the existing
asynchronous SQLite worker without awaiting diagnostic storage or replacing the
command result. A command that only queues work measures its acknowledgement, not that
work's eventual completion. These timings include IPC/queueing but exclude later
snapshot reloads and rendered frames; they are not isolated lock-wait or provider
latency measurements.

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
Desktop usage reports run on a blocking worker. A read-only WAL connection pins one
snapshot/history revision under a short writer lock, then computes without holding
up request/state writes. Its 24 MiB pager and the writer's 8 MiB pager share a
32 MiB budget; in-memory tests retain one connection. Pagination decodes only
selected rows; aggregates and filter choices use narrow indexed projections.

Weekly-capacity card and chart projections use SQL-only snapshots on that same
bounded reader. On file-backed databases they read committed observations without
waiting for the request-writer mutex. SQLite's
[WAL snapshot isolation](https://sqlite.org/isolation.html) preserves one consistent
view; transaction cleanup, query-only mode and pager budgets remain unchanged.
Usage-backed capacity collection still pairs its read with the usage revision
and retains the existing commit fences.

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

Schema 46 stores identical routing/tool diagnostic JSON once in an immutable,
SHA-256-indexed dictionary. Request results and physical sends retain independent
references, so late usage corrections cannot change a completed logical result.
Transactional reference cleanup follows replacement, history expiry, clearing and
account removal. Existing inline records remain readable and convert in batches
of at most 256 per table at startup and during minute maintenance.
Verbose route/cache/transport, context-size and BPS-catalog observations expire
after seven days, independently for each request/send. Core outcomes, request
timing, tool counts/policy, reported usage and pricing retain the 30-day history
window; lifetime aggregates remain available. Compaction invalidates report caches
only after commit. Freed SQLite pages are reused; startup does not run VACUUM.

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
Cards requiring reauthentication retain historical API-equivalent usage and
payback, while hiding quota, reset-credit controls and remaining-value forecasts.
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
Successful catalog-warning writes publish their in-memory value under the same
store guard as persistence, keeping concurrent results in the same order across
memory and restart while preserving unrelated gateway settings. Storage failures
retain the existing best-effort in-memory warning behavior.
Claude Desktop, OpenCode and Codex refreshes retain independent outcomes after
every client is attempted. A peer failure cannot erase Codex's deferred-startup
state; a standalone Codex success clears only its own warning. Other client
failures retain a fixed client identity, error code and recording time until
their own successful refresh or reconnect. Manual reconnects clear only the
matching client's failure. One background result batch persists and publishes
all warnings together. Each batch carries per-client in-memory fences: a newer
batch or matching foreground completion invalidates its older result. Late
failures cannot undo a successful reconnect, and late successes cannot erase a
newer failure or restart requirement. Unchanged peers still publish their own
results; a fully superseded or replayed batch does not write warning state.
Raw error messages, paths and credentials stay outside
these warning records; existing Codex warning fields remain readable on upgrade.

The opt-in [installed Codex tool-cycle check](../../CONTRIBUTING.md#installed-codex-tool-cycle)
uses the actual generated catalog and in-process gateway. It verifies adoption
of the generated instructions, unchanged model/effort, one audited MCP execution,
two physical sends and two terminal usage records. A type/content/order digest
compares the complete multi-part result with the physical upstream, retaining
client-added execution metadata. Codex 0.167.0-alpha.3.2 passed the native Responses
SSE case with the fixture's declared Low effort. Isolated config, synthetic keys
and loopback-only traffic keep this separate from user sessions, Desktop UI,
foreign-protocol and live-provider acceptance.

The separate, explicitly authorized live-profile mode uses one selected generated
catalog row and an exact declared effort through an existing authenticated loopback
Relay provider. The local key remains in the parent observer; the isolated client
uses a synthetic key. Responses are forwarded incrementally without changing their
bytes. First-byte, first-text, tool-argument, terminal, whole-client and between-turn
timings retain their separate meanings. The observer validates the provider's
actual call identity and complete client result before forwarding the next request.
Windows Codex 0.167.0-alpha.3.2 has passed the single-MCP and two-function native
Responses SSE cycles on the BPS `gpt-6.1-sol` High route, including generated
instruction adoption and two completed generations per cycle. In the two-function
case one native BPS envelope expanded into two validated client calls; both distinct
results returned with their original IDs, namespaces and arguments. The check audits
each tool once and rejects unrelated executable output, duplicate executions and
swapped or missing results before further generation. Read-only ledger correlation
confirmed the same account, one recorded physical attempt per generation, terminal token
usage and cached input on the result turn. These cases do not qualify other models,
efforts, JSON clients, larger/custom-tool batches, discovery, media, Desktop UI,
failure recovery or a general latency/cache advantage. Built-in execution remains
outside this check.

ChatGPT, Claude Desktop, and OpenCode have separate profile ownership and recovery.
Claude uses a Relay-owned UUID profile in its `Claude-3p` library, preserves foreign
profiles/selections, and maps stable client-only Claude IDs back to real models.
External client configuration edits are allowed. With a valid recovery record,
explicit reconnect reapplies Relay's connection and model-catalog fields while
preserving unrelated client preferences, extension settings, renamed entries and
the original recovery point. Background refresh skips externally changed profile
bytes instead of overwriting them or reporting an ownership error. Restoration
retains edited profiles and renamed entries, preserves newer foreign selections,
and selects the previous profile only if its entry still exists; otherwise it
clears Relay's selection. Unapplied snapshots consume only their recovery record,
even after external edits. The additive preservation flag contains no secrets and
defaults to false for existing recovery records. Missing-recovery reclamation
still requires the current endpoint/key binding and validated model identities,
not a display name. Malformed configuration, corrupt recovery/path data and an
actual concurrent write remain distinct errors, not bans on external editing.
There is one entry per enabled Messages-routable model, not one per effort.
Initial connection and refresh use the bounded streaming-client snapshot. Explicit
streaming denials and impossible conversions do not produce a Desktop entry;
source-only effort enums honor the same captured raw pool policy and reference
exclusions. A successful authoritative empty refresh clears the owned model list
while retaining authentication, foreign profiles and the original recovery point.
Initial connection still requires at least one usable model before closing or
writing client state. A failed snapshot keeps the saved profile.
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
On verified Desktop 4.29.0+ installations, models resolved from an Anthropic
catalog record with a supported native effort cap use a version-neutral
`claude-relay-` route and the documented `behavesAs` identity. The identity comes
from the catalog's `anthropic/claude-*` source record, not a display name or a
foreign provider's claim. Desktop passes this association to embedded Code's
`modelPicker`; it is not an arbitrary effort enum or a capability declaration
for other vendors. Other models and older clients retain the currently generated
baseline IDs. An unknown publisher version cannot replace an existing identity
profile with a weaker baseline; authoritative-empty refresh still clears owned
entries. Missing-recovery reclaim requires each identity to match the freshly
generated route and catalog identity exactly. Historical `claude-zenith-` and
`claude-fable-5-zenith-` aliases are retired; they no longer resolve pool models.
Windows resolves the registered Desktop package and reads its publisher version
within one bounded metadata helper. The result must match the exact package
family and application ID, or the explicitly selected executable. Version
evidence is read afresh rather than cached across application updates. The
registered-package path keeps the previous combined discovery/version deadline;
an explicitly selected binary keeps its existing metadata-read deadline.
Catalog preparation skips publisher-version helpers when no generated model
needs an identity association and the owned profile has none. A saved identity
still requires version evidence before a nonempty replacement, even when the
new catalog has only other vendors; a failed probe preserves that profile.
The embedded Code engine has its own model-capability lookup. A Desktop picker
selection or saved session effort does not prove the effort sent over HTTP:
unsupported `max`/`xhigh` can become `high` inside that client. Installed Code
2.1.308 and 2.1.309 have passed generated-profile-to-loopback checks: Opus 4.6
preserves Max, and Opus 4.7 preserves Max/XHigh, on both native controls and
opaque Relay aliases. Each case keeps the requested wire model, sends one
Messages request, and consumes a complete synthetic SSE answer. This qualifies
the installed Code hop, not Desktop picker/settings handoff or a real provider.
An opt-in combined fixture also runs the generated alias through Relay's actual
in-process gateway and a synthetic Messages upstream. It checks the canonical
upstream model, unchanged Max/XHigh, one physical send and one successful usage
record per case. Both installed versions passed this combined path for the same
supported efforts. All endpoints and keys belong to the fixture; it does not use
the running application's accounts. Its observer buffers the synthetic SSE
answer and stubs token-count calls locally, so neither stream timing nor the
token-count endpoint is qualified by this check.
An Opus 4.6 XHigh control on 2.1.309 becomes High with or without an alias;
identity mapping does not create missing client capabilities. Relay records
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
Profiles with retained ownership gain current flags on reconnect/catalog refresh.
Missing-recovery reclamation requires the complete current generated settings;
it does not infer defaults for an older profile. Non-UUID profiles are unmanaged:
Relay preserves their files, entries and previous selection rather than adopting
or deleting them. Recovery records for retired profile IDs are rejected unchanged.

OpenCode profiles explicitly disable standard SDK reasoning variants that the
model route does not advertise; omission would let OpenCode restore its built-in
defaults. Models without evidenced effort levels retain the provider's default
mode. Refresh can re-enable a newly supported effort, removes stale generated
variants, and preserves genuinely custom user variants.
An explicit generated `reasoning: false` also disables every standard SDK effort;
absence of a generated variant map must not restore built-in reasoning defaults.

Direct OpenCode profiles constrain reference token limits by the exact selected
source/model/native endpoint's current numeric evidence. Other models and protocols
cannot supply that restriction, and the shared model catalog stays unchanged.
They use the same endpoint projection for tool and reasoning flags and intersected
effort levels. A denial or changed effort list updates only its selected route;
reference exclusions cannot be revived by a source declaration. Refresh removes
stale generated variants without selecting another effort for the user.
Pooled profiles use the eligible alternatives for the selected client API, not
model-wide flags alone. Direct and pooled profiles remove explicitly unsupported
image input while retaining separately declared PDF/audio/video modalities.
Attachment support is disabled only when no non-text input remains; this does
not add support for undeclared media or qualify the client's actual upload flow.
Saved custom windows remain preferences within current generated ceilings; refresh
cannot restore an older larger window over a known bound. Smaller saved windows
remain intact, including when source bounds later relax. Fresh profiles use current
evidence. Generated input/output limits fit their configured context window.
Unknown ceilings do not invent new restrictions. Direct launch and pooled exports
share the supported SDK protocol set: native Gemini Interactions alone is not an
OpenCode route, and an empty compatible catalog cannot be reported as a connected
direct model set. Catalog refresh may still remove obsolete managed entries.

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
Codex's `auth.lock` coordinates credential changes. Snapshot capture also rechecks
`config.toml` and an authoritative `auth.json` after native credential reads.
Concurrent edits or removal return a retryable `conflict` before storing a snapshot,
preserving the current files. Keyring-backed capture leaves an unselected fallback
file undecoded and excludes it from content-conflict checks; `auto` uses that file
only when the native entry is absent. Completed external edits remain capturable.
The profile undo journal includes
native credentials, files and protected binding secrets for conditional error rollback.
History repair uses the saved effective provider, including custom API providers, and
preserves rollout content and newer chats. History rollback follows a failed connection
restore. This in-process rollback is not a durable power-loss transaction.
OpenCode and Claude retain their separate recovery scopes. OpenCode restores its
saved connection without requiring a current Relay provider entry. Current
foreign providers, model selections and unrelated preferences remain current,
including removed preferences; a missing config restores the complete snapshot
and its original model selection. Semantically unchanged config keeps its bytes.
A successfully consumed snapshot reports completion even when Relay's entries
were already removed. Malformed config or recovery data retains the snapshot;
the command clears only OpenCode's catalog warning after successful recovery.
Recovery parses captured config bytes before preparing a replacement. Comments
remain token separators; unterminated comments and empty comma entries are
rejected. A non-object `provider` value stays untouched rather than being erased
by the connection merge. Conflicting snapshots or a malformed missing-file
marker require recovery. Config, snapshot and name bytes are rechecked before
applying the prepared restore and before consuming its record. Observed changes
return a conflict with recovery data retained; write failures permit a retry.
These are optimistic byte checks across independent application writers.
Source edits do not upgrade an already running binary.

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
