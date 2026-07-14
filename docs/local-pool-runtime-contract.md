# Zenith Relay Local Pool Runtime Contract

## Purpose

This document turns local pool research into buildable runtime contracts for
Zenith Relay. It covers account login, API-key source login, local gateway
usage, account selection, quota use, and profile switching.

Public UI must stay neutral. Use `source`, `account`, `local gateway`, `local
API key`, `quota`, `health`, and `profile`.

## User Modes

### Zenith API

User enters a Zenith API key in the existing Zenith mode and can attach
Codex/OpenCode to the Zenith endpoint. Zenith backend still owns public catalog,
balance, billing, and usage history.

The user may separately add that key as a Zenith source preset inside Personal
Local Pool. The two records and their runtime state must not be silently merged.

### Local Pool

User adds personal accounts and API-key sources. App starts a local gateway and
generates local API keys. Clients talk to local gateway; runtime chooses healthy
source/account by local policy.

### Remote Pool

User connects Zenith Relay to a server they own. The server exposes the same
personal-pool management protocol as the desktop local runtime, but stores
secrets and executes requests on that server.

This is one runtime mode with two setup paths:

```text
Connect existing compatible server
Deploy new Zenith personal-pool server
```

Use cases:

```text
user wants pool on a VPS
user wants the pool to keep working when desktop is closed
user wants several local clients to use one personal endpoint
user wants to manage server accounts from Zenith Relay UI
```

Rules:

- the server is user-managed and outside Zenith paid API backend;
- the app must not send Zenith customer API keys or backend internals to that
  server;
- every server import still uses preview/confirm;
- server responses are treated as user-managed data;
- server feature availability comes from `/capabilities`, never from hardcoded
  server name;
- disconnect removes the saved server token from the local secret store.

## Runtime Target Matrix

The public personal pool must support two target adapters over one domain
model:

```text
LocalRuntimeAdapter
SelfHostRuntimeAdapter
```

Shared domain:

```text
ProviderSource
LocalAccount
LocalGatewayKey
GatewaySettings
SchedulerPolicy
UsageLog
QuotaWindow
HealthState
ModelSupport
```

Local target:

- Tauri backend owns storage, secret refs, OAuth/import, scheduler, HTTP
  gateway, request logs, and profile attach/restore;
- default endpoint is `http://127.0.0.1:<port>/v1`;
- app shutdown stops the local gateway unless user has a supported background
  service mode;
- secrets live in OS keychain or encrypted local fallback.

Self-host target:

- deployable personal server owns storage, secret refs, scheduler, HTTP
  gateway, request logs, and request execution;
- desktop app is a management client for that server;
- endpoint is the server's `/v1` URL;
- app shutdown does not stop the server;
- secrets live on the server and are never returned raw to the app after
  confirm.

Deploy/connect split:

- `connect existing` saves URL/token, tests health, reads capabilities, then
  uses `SelfHostRuntimeAdapter`;
- `deploy new` prepares server config and install/update instructions, then
  connects through the same URL/token flow;
- supported MVP deploy methods are Docker Compose, single binary, and manual
  setup instructions;
- deploy helpers are optional. The protocol contract must work even when user
  installs the server without the desktop app.

Adapter rules:

- frontend renders one pool UI from the same state snapshot shape;
- command layer routes actions to local Tauri implementation or self-host HTTP
  client based on active target;
- target-specific fields are nested under `runtime_target`;
- unsupported actions are disabled from capabilities;
- local and self-host telemetry labels must include target kind, but not raw
  host, raw account id, raw email, or secrets.

## Public Self-Host Protocol Contract

Remote Pool talks to a user-managed server through a public personal-pool
protocol. This protocol is separate from Zenith private backend APIs.

Transport:

```text
HTTPS preferred
HTTP allowed only for localhost/private test
Authorization: Bearer <user-self-host-token>
Content-Type: application/json
```

Trust rules:

- remote `http://` self-host URLs are blocked by default unless user enables an
  advanced insecure option;
- `https://` certificate errors are hard failures by default;
- redirects from configured self-host URL to another host require explicit user
  confirmation;
- token is scoped to the configured host and must not be sent to other hosts;
- CORS/browser assumptions do not apply to Tauri commands; backend command code
  owns requests and redaction;
- the app labels self-host data as user-managed and never mixes it with Zenith
  API balance/billing.

Client permission boundary:

- frontend calls only typed Tauri commands, never arbitrary `fetch` to
  self-host/local management endpoints;
- Tauri backend pins self-host token to the configured origin and strips it on
  redirects to any other host;
- local gateway HTTP server validates `Host` against localhost or the explicit
  LAN bind setting;
- local gateway rejects browser-origin management access unless a trusted
  integration is explicitly enabled;
- local API keys cannot authenticate management endpoints;
- management keys cannot authenticate `/v1/*` model requests;
- source/API credentials never cross into frontend state except as masked
  labels.

Versioning:

```text
GET /health
GET /capabilities
```

`/capabilities` must return:

```text
protocol_version
server_name
server_managed_by_user: true
features[]
supported_wire_apis[]
supports_accounts
supports_sources
supports_quota
supports_usage
supports_local_gateway
supports_profile_attach
supports_wake_tasks
```

Compatibility rules:

- app declares `min_supported_protocol_version` and
  `max_supported_protocol_version`;
- server declares `protocol_version` and optional `compatibility_min_client`;
- if versions do not overlap, app disables management actions and shows a
  reconnect/update reason;
- every optional screen is enabled only from `features[]`, not from server name;
- token is pinned to the saved origin and server identity fingerprint when the
  user chooses to trust a self-host server;
- reconnect after certificate/server identity change requires explicit user
  confirmation;
- token rotation/disconnect must clear the old token from secret store and
  cancel in-flight self-host commands;
- protocol additions are additive by default. Breaking changes require a new
  major protocol version and disabled UI until the user upgrades.

Stable feature names:

```text
accounts
account_batch_import
sources
quota
models
usage
local_gateway
keys
profile_attach
diagnostics
wake_tasks
account_proxies
```

Minimum safe endpoints:

```text
GET  /health
GET  /capabilities
GET  /state
GET  /accounts
POST /accounts/{id}/refresh
POST /accounts/import/preview
POST /accounts/import/confirm
POST /accounts/import/batch/preview
POST /accounts/import/batch/confirm
POST /accounts/proxies/assign
POST /accounts/{id}/proxy
DELETE /accounts/{id}
POST /proxies/common
POST /proxies/policy
POST /pool/members
POST /pool/quota/refresh
GET  /sources
POST /sources
PATCH /sources/{id}
DELETE /sources/{id}
POST /sources/{id}/test
GET  /quota
POST /quota/settings
GET  /models
POST /gateway/start
POST /gateway/stop
GET  /usage
GET  /wake-tasks
POST /wake-tasks
PATCH /wake-tasks/{id}
DELETE /wake-tasks/{id}
POST /wake-tasks/{id}/test
GET  /wake-history
```

`account_batch_import` accepts a bounded JSON object, JSON array, JSON Lines,
or portable version-1 object with an `accounts[]` array. Preview responses are
redacted and return independently selectable item IDs. Confirm requires the
batch session ID plus `selectedItemIds`; item IDs cannot be confirmed from a
different batch. Proxy and source records in a portable bundle are reported as
ignored and are never ingested by account import.

`POST /sources/{id}/test` performs bounded model discovery with the stored
upstream credential, updates the source model registry, and returns only the
redacted source summary. `POST /wake-tasks/{id}/test` validates that specific
task selector and model policy and reports the eligible account count; it does
not enqueue or send a wake request.

`account_proxies` provides one encrypted common HTTP(S) proxy plus an optional
encrypted override per account. Effective routing is `account override ->
common proxy -> direct`. A configured proxy that is missing or invalid fails
closed for that account; runtime requests, token refresh, quota refresh, model
discovery, and wake requests must never silently fall back to a direct route.
The optional `accountProxyRequired` policy removes the final direct route for
OAuth accounts. Accounts without a valid account or common proxy remain stored
and visible but are excluded from execution until a proxy becomes available.
Management snapshots expose only proxy mode and availability, never a saved URL
or credentials. Bulk assignment maps submitted proxy lines to submitted account
IDs in order and reports unused lines.

New connections default to `inPool=false`. `POST /pool/members` changes
membership atomically without deleting the underlying connection.
`POST /pool/quota/refresh` refreshes only enabled, non-draining OAuth members
with bounded concurrency. `POST /quota/settings` accepts a background interval
from 120 through 3600 seconds and a network timeout from 10 through 20 seconds;
both values persist in local/server runtime state and are returned in the
redacted gateway snapshot.

Optional proxy endpoints:

```text
GET  /v1/models
POST /v1/responses
POST /v1/chat/completions
POST /v1/messages
```

Error shape:

```text
error.code
error.message
error.stage
error.retryable
error.request_id
```

Rules:

- the app must not assume a self-host server supports every feature;
- unsupported capability disables the matching UI with a short reason;
- self-host responses must be treated as user-managed data, not Zenith data;
- self-host protocol must not include Zenith customer billing, private economy,
  backend execution policy, internal inventory, or private admin sessions;
- raw secrets from self-host responses are rejected unless the user explicitly
  imports them through preview/confirm;
- protocol additions must be backwards compatible or gated by
  `protocol_version` and `features[]`.

## App State Snapshot Contract

The frontend should read one state snapshot for the local pool surface instead
of assembling state from many unrelated calls.

Snapshot fields:

```text
settings
running
default_profile_attach_status
api_port_url
base_url
lan_base_url
visible_model_ids
last_error
candidate_count
stats
account_health[]
source_health[]
key_summaries[]
wake_task_summaries[]
warnings[]
```

Profile attach status:

```text
profile_dir
attached
config_attached
auth_attached
model_provider
base_url
expected_base_url
error
```

Rules:

- state responses must redact secrets and exact account identifiers by default;
- every mutating command should return the new state snapshot when practical;
- if a gateway reload is needed, command result should say whether reload was
  completed, scheduled, or blocked;
- partial failures return updated state plus a typed warning, not only a raw
  string error;
- frontend must not infer runtime state by reading profile/config files.

## Command And Failure Contract

Command groups should follow the object boundaries from
[`local-gateway-architecture.md`](./local-gateway-architecture.md):

```text
state
sources
accounts
gateway
keys
routing
models
telemetry
profile
diagnostics
instances
automation
```

Test and diagnostic commands should return one failure shape:

```text
title
stage
cause
suggestion
http_status
model_id
detail
gateway_output
```

Stages should be stable enum-like strings:

```text
local_gateway
local_key_auth
profile_attach
source_test
account_auth
quota
model_policy
request_shape
stream
wake_trigger
wake_request
wake_verify
```

The UI can translate these into Russian/English. Raw source bodies stay in
debug logs with redaction.

## Account Login Contract

Supported login/import shapes:

- browser OAuth login with local callback;
- manual callback paste;
- local profile `auth.json`;
- pasted token JSON;
- refresh-token-only import that exchanges token before save;
- access-token-only import marked degraded;
- API key plus base URL/protocol metadata;
- batch JSON/JSONL import with preview.

Flow:

```text
choose type -> collect input -> parse -> optional token exchange
-> optional quota probe -> preview -> confirm selected -> save
-> update runtime credential -> update model registry/scheduler
```

Required invariants:

- no secret enters active pool before preview/confirm;
- OAuth pending state survives app restart;
- each account refresh uses per-account async lock and file lock;
- invalid/reused refresh token marks `requires_reauth`;
- access-token-only account never attempts refresh;
- API-key source without quota adapter shows `quota unsupported`;
- deleting account/source removes all local key scopes and scheduler entries.

## Quota Wake Automation Contract

Quota wake automation exists to start a provider's rolling quota countdown
after a selected window becomes fully available. It is an account lifecycle
job, not a synthetic client request and not a routing strategy.

Supported targets:

```text
OAuth account only
normalized primary quota window
normalized secondary quota window
adapter-declared wake-capable model
```

Task record:

```text
id
name
enabled
account_selector: all_eligible | account_ids[] | tags[]
window_kinds[]: primary | secondary
model_policy: lightest_supported | explicit_model
explicit_model_id
trigger: quota_full | daily | weekly | interval
fallback_schedule
execution_policy: automatic | require_confirmation
jitter_seconds
max_attempts_per_cycle
created_at
updated_at
```

Provider adapters must declare whether a quota window needs activity to start a
new countdown. Relay must not infer this from a provider name or from a fixed
five-hour/weekly assumption. UI may show `5-hour` and `weekly` only when the
adapter has mapped those normalized windows confidently.

Eligibility flow:

```text
quota refresh or known reset time becomes due
-> normalize current window snapshot
-> detect transition to fully available
-> verify account enabled, healthy, authenticated, and selected
-> skip if a client request already used the account after the transition
-> acquire per-account execution lock
-> check cycle dedupe ledger
-> enqueue one wake attempt with optional jitter
```

`fully available` uses adapter-normalized state or a configurable threshold,
never exact floating-point equality. Default threshold is `>= 99.5%` only when
the adapter cannot provide an explicit full flag.

Wake request policy:

```text
smallest supported text model
lowest reasoning effort or reasoning disabled
fixed internal prompt
no tools, files, images, or external actions
non-stream request
strict output-token cap
response body discarded
```

The wake executor must not use a model that the account cannot access. An
explicit model is validated when the task is saved and again before execution;
otherwise Relay selects the lightest currently supported model.

Verification:

1. Refresh the affected quota after a short adapter-defined delay.
2. Confirm a future reset timestamp, changed cycle fingerprint, or another
   adapter-declared proof that the countdown started.
3. Store `confirmed`, `unconfirmed`, `skipped`, or `failed` per account/window.
4. Do not loop on `unconfirmed`; allow at most one delayed retry when the task
   explicitly permits it.

Dedupe key:

```text
account_id + window_kind + full_transition_fingerprint
```

Natural client usage wins. If the account served a normal request after the
full transition, the wake task records `skipped_already_started` and spends no
extra quota.

Scheduling rules:

- quota refresh scheduler emits window-transition events; no global two-minute
  polling loop is added;
- known reset timestamps schedule one targeted refresh with adapter lead time;
- stale or unknown quota uses existing refresh backoff, not wake-specific busy
  polling;
- local tasks run only while the desktop runtime is active;
- remote tasks run on the user-managed server and survive desktop shutdown;
- bounded concurrency and per-account locks prevent bursts and duplicate use.

History stores task id, trigger, account/window ids, timestamps, model id,
technical outcome, latency, token counts, and redacted error code. It never
stores prompt text, generated response content, OAuth tokens, or raw provider
bodies.

## Local API Key Contract

Local API keys authenticate client requests to the local gateway only. They are
not source credentials.

Local key fields:

```text
id
label
enabled
secret_ref
source_scope[]
account_scope[]
allowed_models[]
excluded_models[]
model_prefix
allowed_wire_apis[]
created_at
last_used_at
```

Request auth order:

```text
extract local key -> validate enabled -> resolve local key policy
-> validate model visibility -> attach scope metadata
```

Never forward local API key to source/account executor.

## Gateway Request Contract

Runtime path:

```text
HTTP request
-> local key auth
-> request shape detection
-> model prefix strip / alias resolve
-> visible model validation
-> scheduler scope
-> candidate selection
-> account/source prepare lock
-> translator request
-> executor
-> translator response
-> health/cooldown update
-> usage event queue
```

Hard gates before priority/weight:

```text
disabled
draining
requires_reauth
quota_exhausted
cooldown
model_not_allowed
model_not_supported
protocol_not_supported
capability_missing
```

Selection:

1. Use local key source/account scope.
2. Apply hard gates.
3. If session binding still points to healthy capable candidate, use it.
4. Apply API-source role tier: primary before OAuth/stabilizer, reserve last.
5. Prefer the lowest active-request load normalized by traffic share.
6. Within the tier, prefer OAuth when otherwise equal, then least recently
   used, known quota, priority, weight, and stable id.
7. Exclude already tried candidates for this request.
8. If all candidates are cooling down, return cooldown diagnostic.

`tried` and `attempted` stay separate. A candidate can be tried but not
attempted when it fails mapping/preparation before executor call.

## Stream Retry Contract

Streaming retry is allowed only before payload reaches client.

Flow:

```text
select candidate -> open source stream -> read bootstrap chunk
-> if error before payload: mark result and try next candidate
-> if payload emitted: no transparent retry
-> log terminal stream error if later failure happens
```

This prevents duplicated/corrupted streamed output.

## Profile Switch Contract

Profile switching is explicit. Runtime requests do not rewrite client profiles.

Attach/switch flow:

```text
inspect profile -> backup current auth/config/model catalog
-> prepare selected account/local key -> write managed config
-> verify -> update profile binding -> show restore state
```

Restore flow:

```text
inspect profile -> confirm managed marker still matches
-> restore backup or remove only managed blocks
-> preserve unrelated config/plugins -> verify -> clear consumed backup
```

Rules:

- one backup record per normalized profile path;
- repeated attach updates backup metadata, not duplicate files;
- restore refuses when user manually logged in after attach;
- named instance must be initialized before binding;
- switching credential kind can recommend history repair.

## Model Registry Contract

The local model registry is runtime state, not Zenith public catalog.

Inputs:

- discovered source models;
- account plan/model capability;
- local aliases;
- hidden/excluded model rules;
- local key scope;
- model health/cooldown/quota state.

Output:

- `/v1/models` for current local key;
- per-source/account model detail for UI;
- visibility explanation when model hidden.

Model visibility reason values:

```text
visible
hidden_by_local_key
hidden_by_global_rule
excluded_by_source
excluded_by_account
cooling_down
quota_limited
no_healthy_candidate
unsupported_protocol
missing_capability
```

## Usage Contract

Usage records are local diagnostics, not Zenith billing.

Record:

```text
request_id
local_key_id
source_id
account_id
requested_model
resolved_model
wire_api
success
http_status
error_category
latency_ms
ttft_ms
input_tokens
output_tokens
total_tokens
```

Usage publishing is async. Sink failure must not fail user request.
Local UI groups recorded requests by the current account/source label and shows
request count plus input, output, and total tokens. The remote management API
keeps the stored candidate id hashed; it may attach the label already exposed
by the current runtime snapshot, but never a raw provider account id or email.
Relay does not estimate monetary API cost without an authoritative,
provider-specific pricing contract.

For model ids covered by Relay's versioned official OpenAI price catalog, the
local and remote snapshots may also expose an `api_equivalent` aggregate:

```text
micro_usd
priced_tokens
unpriced_tokens
```

The calculation uses recorded input/output tokens and integer micro-USD math.
It is an informational OpenAI API comparison, not the cost of a subscription,
the upstream provider's invoice, Zenith billing, or scheduler input. Unknown
models and totals without a usable input/output split increase
`unpriced_tokens`; Relay never assigns them an invented price. The catalog
stores its source URL, verification date, and version so price updates remain
reviewable and centralized.

## Request Log Query Contract

Remote request logs use server-side pagination and filters. Desktop-local logs
use a bounded recent page; neither path loads an unbounded history into the
frontend.

Query:

```text
page
page_size
range: daily | weekly | monthly | custom
model_query
source_or_account_query
local_key_query
wire_api
request_kind
success
error_category
```

Page response:

```text
events[]
total
page
page_size
total_pages
```

Rules:

- query filters must work from SQLite indexes;
- account/source identity fields are masked unless user toggles reveal;
- request id has copy action and opens one detail view;
- log detail may include redacted headers/body only when debug capture was
  explicitly enabled before the request.

## Activation And Switch Contract

Activating local gateway mode should be one explicit flow:

```text
snapshot previous credential kind
attach local gateway profile
write speed/profile settings
clear stale current account state
bind default profile to local gateway key
prepare session visibility notice if credential kind changed
optionally launch or restart client
return state snapshot
```

Credential kind values:

```text
account
api
local_gateway
```

Rules:

- switching to local gateway must not silently delete previous login/API-key
  backup;
- current account selection should be cleared only for the affected profile;
- switching credential kind should show a repair prompt, not run destructive
  repair without preview;
- optional launch/restart belongs after profile attach succeeds;
- if launch fails, keep attach result and expose next action.

## Cross-Platform Runtime Contract

Zenith Relay must provide the same local-pool, automation, profile, and remote
management features on Windows, macOS, and Linux.

Release targets:

| System | Architectures | Packages |
| --- | --- | --- |
| Windows | x64, ARM64 | portable executable, setup executable, MSI |
| macOS | Intel, Apple Silicon | app bundle, DMG |
| Linux | x64, ARM64 | AppImage, DEB, RPM |

Platform-specific behavior belongs behind one Rust platform adapter:

```text
profile/config path discovery
secret-store access
loopback OAuth callback and browser launch
process launch and detection
file locking and stale-lock recovery
autostart/background capability
open file/folder action
```

Rules:

- routing, quota, wake-task, usage, and profile business logic stays shared;
- frontend never hardcodes Windows drive letters or Unix home paths;
- Windows uses the native credential store, macOS uses Keychain, and Linux uses
  Secret Service when available;
- when a native secret store is unavailable, Relay requires an encrypted local
  vault or disables secret persistence. Plaintext fallback is forbidden;
- profile locations come from platform APIs and XDG conventions, with manual
  selection as a fallback;
- loopback OAuth and manual callback paste must work on all three systems;
- local endpoint binds to loopback by default on every system;
- wake tasks follow the same cycle-dedupe rules everywhere. Desktop tasks pause
  when Relay exits; user-managed server tasks continue independently;
- a platform may not ship publicly while a core feature is silently missing.

## Implementation Order

The only active build order and release gates live in
[local-pool-final-planning.md](local-pool-final-planning.md). This document owns
runtime, self-host, gateway, and failure contracts only.

## First Test Matrix

- OAuth pending session resumes after app restart.
- Two parallel refreshes rotate token once.
- Profile/file locks prevent two app instances from writing the same profile at
  the same time.
- Stale profile/gateway lock takeover checks PID/process state first.
- API-key source with no quota endpoint remains usable.
- Deleted account disappears from local key scopes and profile binding.
- `/v1/models` explains hidden model reason.
- Local key never reaches source executor.
- Stream retries only before first payload.
- Failed candidate updates per-model cooldown.
- Successful candidate clears cooldown and resumes model.
- Usage sink panic does not fail request.
- Profile restore refuses after fresh manual login.
- Profile attach/restore rejects path traversal, symlink/junction escape, and
  unexpected file extension writes.
- Store migration creates backup, rolls back on failure, and quarantines corrupt
  files.
- Unsupported newer store schema fails safely instead of rewriting it.
- Support bundle redacts secrets and account identity by default.
- Support bundle export requires explicit user action and redaction preview.
- Telemetry/debug body capture stays disabled by default.
- Signed update verification rejects unsigned or wrong-channel package.
- Self-host protocol version mismatch disables unsupported UI.
- Quota-full transition emits one wake job per account/window cycle.
- Natural client use after transition suppresses the wake request.
- Unconfirmed countdown start does not create an infinite retry loop.
- Wake history contains no generated response body or secret.
- Path, secret-store, OAuth, profile attach, local endpoint, and wake-task tests
  pass on Windows, macOS, and Linux.
- Self-host token is not sent after redirect to another host.
- Self-host server identity change requires explicit confirmation.
- File watcher ignores empty config writes.
- File watcher skips reload when content hash is unchanged.
- Runtime update queue dedupes multiple updates for the same credential id.
- Runtime-only credential survives file-backed auth refresh.
- Deleting a source/account removes local key scopes, model rules, and runtime
  registry entries.
- Candidate selector applies API-source roles, spreads concurrent requests by
  traffic share, and rotates otherwise equal OAuth accounts per request.
- Mixed-source selector does not select cooling-down credentials when another
  healthy candidate exists.
- `tried` candidate without attempted execution does not count against
  `max_retry_candidates`.
- Just-in-time request auth preparation runs once under concurrent requests.
- Preparation failure updates candidate health and tries another candidate.
- Model registry keeps model visible when one scoped healthy credential remains.
- Model registry hides model when all scoped credentials are hard-suspended.
- Streaming bootstrap retry replaces source headers before first payload.
- Streaming retry stops after first payload was sent.
- Management key auth blocks repeated failed attempts and never accepts local
  API keys as management keys.
