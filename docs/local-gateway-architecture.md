# Zenith Relay Local Gateway Architecture

## Scope

These notes define Zenith Relay local gateway behavior and local pool
requirements. They are product and architecture notes only. Use original Zenith
implementation, UI copy, assets, provider records, and naming.

## Required Capabilities

The local Codex-compatible pool should include:

- local HTTP API with `/v1/models`, `/v1/responses`,
  `/v1/chat/completions`, `/v1/images/generations`, `/v1/images/edits`, and
  later `/v1/messages`;
- generated local API keys with labels, enable/disable, rotate/delete,
  per-key account/source scope, allowed models, excluded models, and optional
  model prefix;
- localhost by default, optional LAN scope, visible base URL, and port cleanup;
- profile attach/restore for Codex/OpenCode config and `auth.json`;
- import preview for local auth files and pasted token JSON;
- request logs, daily/weekly/monthly stats, latency, tokens, cached tokens,
  reasoning tokens, error categories, and local estimated cost;
- account health with last success/failure, consecutive failures, cooldowns,
  quota-limited state, and image capability status;
- automatic routing by API-source role, greatest remaining quota, active load
  between equal-quota candidates, and stable tie-breaks; key scope covers
  single-account use;
- stream-safe retry rule: retry only before response bytes are sent.

## Implementation Risks To Avoid

Avoid:

- one huge backend module for storage, gateway, scheduler, profile edits,
  request translation, logs, and tests;
- too many advanced controls visible in the first screen;
- UI copy that explains internal mechanics instead of user-owned local state;
- provider-specific logic mixed with generic pool logic;
- public docs that imply user accounts are uploaded to a central service.

## Zenith Module Split

Exact repository paths and package names are owned by
[project-structure.md](project-structure.md). The trees in this section explain
module responsibilities only; `project-structure.md` wins if a path differs.

Keep desktop adapters under the real Tauri backend path and never put runtime
logic into the React frontend. Reusable local/server behavior belongs in
`crates/relay-core`; the only standalone runtime package is `relay-server`.

Design the runtime around a shared core plus target adapters:

```text
local_pool_core
-> records, validation, scheduler contracts, usage math, redaction

desktop_local_runtime
-> Tauri storage, OS keychain, OAuth/import, local HTTP gateway, profile attach

self_host_runtime_client
-> typed client for compatible self-host and Personal Pool Server targets

external personal-pool server package
-> deployable user-managed server, implemented outside the Tauri app
-> uses the same public contracts
```

Rules:

1. Frontend renders shared DTOs and does not know whether state came from local
   runtime or self-host server except `runtime_target.kind`.
2. Desktop local runtime can use OS keychain and profile attach/restore.
3. Self-host server runtime cannot assume desktop profile paths or OS keychain.
4. Shared scheduler/core must not read local files, Tauri state, or Zenith
   backend internals.
5. Public server code must not import private Zenith account-pool modules,
   provider economy, customer billing, or gateway fallback policy.
6. If a feature is local-only or server-only, expose it through capabilities and
   disable the matching UI when unavailable.

```text
zenith-relay/
  src-tauri/
    src/
      local_pool/
        mod.rs
        models.rs              shared Rust contracts returned to frontend
        commands/
          mod.rs                Tauri command registration only
          state.rs              state snapshot commands
          sources.rs            source CRUD/test commands
          accounts.rs           account import/CRUD/quota commands
          gateway.rs            start/stop/key/profile gateway commands
          keys.rs               generated local API-key commands
          routing.rs            automatic order, priority, weight, affinity commands
          telemetry.rs          log/stat query commands
          profile.rs            attach/restore/repair commands
          diagnostics.rs        tests and support bundle commands
          self_host.rs          custom self-host connection commands
        store/
          mod.rs
          secret_store.rs       encrypted vault references and OS-held master key
          telemetry_db.rs       SQLite state, request logs, affinity, migrations
          vault.rs              authenticated encrypted secret file
        accounts/
          mod.rs
          account_store.rs      account records, index repair, current state
          oauth.rs              browser/manual OAuth login sessions
          token_authority.rs    refresh locks, token generation, reauth
          imports.rs            preview/confirm for auth.json/token/batch
          quota.rs              quota/subscription refresh
        sources/
          mod.rs
          providers.rs          source adapters, tests, model discovery
          openai_compatible.rs
          zenith_api.rs         preset adapter using user-provided Zenith key
        runtime/
          mod.rs
          runtime_manager.rs    credential snapshots and update queue
          model_registry.rs     local visible model registry
          watcher.rs            profile/config watchers
          translators.rs        responses/chat/messages/image adapters
          executors.rs          selected source/account execution
        scheduler/
          mod.rs
          selection.rs
          capacity.rs
          cooldown.rs
          affinity.rs
        gateway/
          mod.rs
          server.rs             localhost HTTP server lifecycle
          auth.rs               local API-key request auth
          routes.rs             /v1 route dispatch
          management.rs         optional localhost-only management
        profile/
          mod.rs
          codex.rs              Codex config/auth adapter
          opencode.rs           OpenCode adapter
          instances.rs          named profiles and process binding
          repair.rs             history visibility repair
        self_host/
          mod.rs
          client.rs             public self-host protocol client
          capabilities.rs
        personal_server/
          mod.rs                server connection and deploy-helper commands
          deployment.rs         config/instruction generation only
        diagnostics/
          mod.rs
          support_bundle.rs
          redaction.rs
      main.rs
      key_storage.rs            existing Zenith API key storage until migrated
      codex_config.rs           existing Zenith API config writer until split
      launcher.rs               existing launcher until profile module owns it
      files.rs
      platform.rs
      tray.rs
```

The deployable Personal Pool Server runtime is not a Tauri module. Its package
or repository is selected in P6 of
[local-pool-final-planning.md](local-pool-final-planning.md). The desktop
`personal_server` module only connects to it and prepares user-approved
deployment material.

Frontend split:

```text
zenith-relay/
  src/
    src/
      features/
        local-pool/
          api/
            client.ts           typed Tauri command wrappers only
            types.ts            frontend DTOs mirrored from backend contracts
          state/
            useLocalPoolState.ts
          shell/
            LocalPoolShell.tsx
            ModeSelector.tsx
            StatusStrip.tsx
          routes/
            Home.tsx
            Connections.tsx
            Servers.tsx
            Sources.tsx
            Accounts.tsx
            Pool.tsx
            Gateway.tsx
            Keys.tsx
            Usage.tsx
            Instances.tsx
            Settings.tsx
            SelfHost.tsx
          components/
            tables/
            forms/
            quota/
            health/
            profile/
            dialogs/
          hooks/
          styles.css            feature-local CSS if global styles grow too big
      i18n/
        locales/
          ru.ts
          en.ts
```

Keep `Zenith API Mode` separate from `Personal Local Pool Mode`. Zenith API is
one preset source in local mode, not the local pool's hard dependency.

Runtime data lives below the branded platform-local `Zenith Relay` directory,
not in the repository. The exact durable `data`, rollback `recovery`, temporary
`cache`, optional `logs`, and external ChatGPT profile paths are defined only in
[project-structure.md](project-structure.md#desktop-runtime-data-tree).

Rules:

1. Frontend files never read or write this data directly.
2. Secrets use OS keychain first. Encrypted file fallback is allowed only with a
   local master key outside repo/config exports.
3. Settings and records store only non-secret values and `secret_ref` ids.
4. Cache-scoped locks coordinate token refresh. In-process guards coordinate
   profile writes and gateway lifecycle; stale filesystem locks expire before
   takeover.
5. Corrupt durable state is never deleted or replaced automatically; startup
   fails with a recovery error so the original files remain available.
6. Private admin code, Zenith server secrets, and backend execution policy do
   not belong in this public repository.

## Backend Command Surface

The Tauri command API should be grouped by object, not exposed as one large
local-access controller.

Required command groups:

- `state`: return one snapshot with gateway status, settings, sources,
  accounts, keys, health, stats, warnings, and profile attach status;
- `sources`: create, update, test, enable/disable, delete, refresh models, and
  rotate stored key value;
- `accounts`: import preview, save, update labels/tags, enable/disable, drain,
  remove, refresh quota, and test selected model;
- `gateway`: start, stop, prepare restart, kill occupied port, update port,
  update bind scope, update client host, and update advanced timeouts;
- `keys`: create, update label, enable/disable, rotate, delete, and update
  scope/model policy;
- `routing`: update tie-break priority, weight, retry limits,
  and cooldown policy;
- `models`: update aliases, hidden/excluded models, per-source/account model
  blocks, and local pricing used only for estimates;
- `telemetry`: query request logs, clear stats, rebuild aggregates, and reprice
  old logs after local pricing changes;
- `profile`: inspect config, backup, attach, verify, restore, and repair known
  Codex/OpenCode profile shapes;
- `diagnostics`: non-stream test request, stream test request, support bundle
  export with secrets redacted.

Frontend should call commands only. It should not edit Codex config files,
gateway storage, SQLite logs, or secret records directly.

Account/auth/import/quota/profile contracts are tracked in
[`local-account-auth-architecture.md`](./local-account-auth-architecture.md).

### Command Result And Runtime Sync

Every command that changes settings, sources, accounts, local keys, routing, or
model rules should follow one write path:

```text
load current state -> validate patch -> normalize -> diff -> save store
-> update affected runtime shards -> reload gateway if running
-> return state snapshot
```

Rules:

- no-op normalized patches should return current state without disk write;
- source/account/key edits update only affected scheduler and model registry
  shards;
- runtime reload reason should be logged with redacted values;
- if reload is asynchronous, state should expose `reload_pending`;
- gateway port cleanup returns killed process count plus refreshed state;
- timeout presets and detailed timeout fields are advanced settings, not setup
  wizard fields.

Useful state fields for the main UI:

```text
running
api_port_url
base_url
lan_base_url
visible_model_ids
last_error
candidate_count
stats
account_health
source_health
profile_attach_status
```

## Storage And Migration

Use two stores instead of multiple JSON documents:

- encrypted secret store for tokens and API keys, with its master key in the OS store;
- one SQLite database for non-secret state, request logs, affinity, and rollups;
- profile backup directory with one folder per attach/restore event.

Database migrations are versioned and append-only. Use transactions for state
changes and indexes for the filters shown in the UI.

If the database is corrupt or unreadable, keep it in place and fail startup
with a recovery error. Never silently recreate an empty database.

### Local File And Secret Safety

File access rules:

- canonicalize every user-selected profile, import, backup, and export path;
- reject path traversal, empty names, device paths, and unexpected extensions;
- check symlinks/junctions before writes and after open where the OS allows it;
- allow writes only under selected profile dirs, app data dir, or explicit user
  export paths;
- use SQLite transactions for durable state changes;
- use SQLite backup API for profile/history DB backups;
- hold profile/file locks while writing client auth/config;
- do not follow stale PID or stale lock blindly.

Secret storage rules:

- prefer OS keychain/credential manager for API keys and OAuth tokens;
- encrypted file fallback must store ciphertext only and record key version;
- support bundle, logs, screenshots, diagnostics, and exports redact secrets by
  default;
- export of raw secrets is disabled for MVP and must require explicit reveal
  flow if added later;
- migration never writes plaintext secret values into non-secret config.

Update and rollback rules:

- app updates must come from signed releases;
- run store migrations transactionally and leave source files untouched until
  the new state is committed;
- failed migration keeps the original data available for recovery;
- the current flat JSON schema is imported once; older pre-release layouts are
  unsupported;
- old app versions must fail safely on newer unsupported schema instead of
  corrupting it.

## Local Data Model

Core records:

```text
ProviderSource
  id
  name
  enabled
  base_url
  api_key_secret_ref
  wire_api: responses | chat_completions | messages
  models
  allowed_models
  excluded_models
  model_prefix
  supports_vision
  supports_images
  priority
  weight
  last_test_at
  last_test_status
  last_error

LocalAccount
  id
  label
  provider_kind
  auth_mode: oauth | api_key | imported_token
  identity
  secret_ref
  subscription
  quota_windows
  reset_times
  health
  tags
  enabled
  draining
  priority
  weight

LocalGatewayKey
  id
  label
  key_secret_ref
  enabled
  source_ids
  account_ids
  allowed_models
  excluded_models
  model_prefix
  last_used_at

GatewaySettings
  enabled
  bind_scope: localhost | lan
  port
  client_host: localhost | 127.0.0.1
  routing_strategy
  max_retry_candidates
  timeouts
```

Secrets should be encrypted locally where platform support exists. Raw secrets
must not appear in logs, screenshots, or exported support bundles.

## Runtime Auth Model

Runtime should use a normalized credential record independent from import file
shape. This lets OAuth accounts, API-key sources, and generated runtime-only
entries pass through one scheduler/executor path.

```text
RuntimeCredential
  id
  stable_index
  source_id
  account_id
  protocol
  label
  status: unknown | active | pending | refreshing | error | disabled
  disabled
  unavailable
  auth_kind: oauth | api_key | imported_token | runtime_only
  quota
  model_states
  last_error
  last_refreshed_at
  next_refresh_after
  next_retry_after
  success_count
  failure_count
  recent_request_buckets
```

`stable_index` should be derived from normalized identity such as source kind,
base URL, account id, and secret fingerprint. It must not expose raw tokens or
keys. Use it for UI ordering, config reconciliation, and stable row identity
after app restart.

Recent request buckets belong on the credential state, not only in telemetry.
They make pool tables and source/account health visible without expensive log
queries.

## Request Auth Boundary

Local request authentication is separate from source/account credentials.

Required behavior:

- authenticate through a provider chain, not one hardcoded header parser;
- support `Authorization: Bearer`, `X-Api-Key`, `X-Goog-Api-Key`, `key`, and
  `auth_token` only when enabled for local compatibility;
- distinguish `not_handled`, `missing_credentials`, and
  `invalid_credentials`;
- attach local key id, source/account scope, and request-auth source to request
  context;
- never pass local gateway key to source executors.

This gives the UI precise errors:

```text
local endpoint did not require this auth shape
no local API key provided
local API key is invalid or disabled
```

## Gateway Request Flow

```text
client request
-> local API key auth
-> model/source/account scope filter
-> request shape detection
-> scheduler candidate filter
-> account/provider executor
-> stream/non-stream adapter
-> usage capture
-> health/cooldown update
-> local log/write stats
```

Hard filters before automatic ranking:

```text
disabled
draining
auth_required
quota_exhausted
cooldown
model_not_allowed
protocol_not_supported
capability_missing
```

API-source role and traffic share apply only after hard filters. Manual priority
is the final tie-breaker.

## Scheduler Design

The local scheduler is incremental. Updating one source/account upserts or
removes that candidate; request selection evaluates its model and key scope
without rebuilding the whole pool.

Core scheduler state:

```text
PoolScheduler
  candidates: candidate_id -> RuntimeCandidate
  response_affinity: bounded response id -> creating candidate id (30-day TTL)
  prompt_affinity: bounded cache key -> successful candidate id (1-hour TTL)
  in_flight: candidate_id -> active request count
  dispatches: candidate_id -> committed request count
  execution_fences: candidate_id -> active token-recovery count
  capability_blocks: candidate/model pairs rejected by upstream discovery
  provider_storm_breakers: server-only provider/model 429 windows

RuntimeCandidate
  kind: OAuth account or API source
  source/account identity and protocol
  enabled, draining, secret availability, health, quota
  models and allowed/excluded rules
  API-source role marker, manual priority, and traffic share
  per-model cooldowns and last-used time
```

Candidate states:

```text
ready
cooldown
blocked
disabled
```

Selection contract:

1. Normalize source/account scope from local key policy.
2. Apply hard gates.
3. Promote expired cooldown entries.
4. If a previous-response binding exists, require its creating candidate; a
   cooldown or failed preparation does not authorize cross-account replay.
5. Apply API-source role tier: primary before OAuth/stabilizer, reserve last.
6. In automatic mode, select the greatest known minimum remaining quota from the
   latest backend refresh. A protected OAuth candidate contributes only quota
   above its reserve.
7. Between equal-quota candidates, prefer the lowest active-request count, then
   rotate by dispatch count and a stable identifier. Last-used time does not
   affect routing. Subscription, token totals, measured speed, manual priority, and
   manual weight do not affect OAuth selection. API-source roles remain strict,
   with traffic share used only between API sources in the same role.
   Highest-quota mode stops after the quota comparison and stable-id tie-break;
   it does not use active load or dispatch history.
8. Prefer a successful `prompt_cache_key` binding when its candidate has the
   same role and active-request count as the baseline and remains within 500
   basis points of the baseline quota.
9. Exclude candidates already tried for this request.

The desktop runtime and the current SQLite server runtime are single-instance.
Prompt affinity therefore remains process-local and leases remain in-process.
Before enabling multiple server replicas, move both leases and prompt affinity
to the shared PostgreSQL/Redis control plane with ownership tokens and expiry;
do not emulate distributed coordination through SQLite files.
10. If all candidates are cooling down, return a local cooldown diagnostic with
   earliest retry time.

Every emitted usage attempt may carry a bounded redacted routing diagnostic:
the decisive comparison, eligible count, selected quota reserve, and load
counters before reservation. Local and self-hosted stores keep
this as structured JSON without request text, response text, headers, secrets,
proxy addresses, or raw account identities.

Single-source and mixed-source paths should share the same candidate state
logic. Mixed-source selection must not skip model cooldown state just because
another source exists.

## Runtime Model Registry

The local gateway should maintain a runtime model registry separate from Zenith
API public catalog.

Registry data:

```text
model_id
source/account client count
source-specific model metadata
quota/cooldown markers per credential
temporary suspension reason per credential/model
last updated
cached visible models by wire API
```

Rules:

- registering an account/source replaces its previous model snapshot;
- empty model snapshot unregisters that account/source from visible catalog;
- source-specific metadata wins only for that source;
- quota/cooldown markers should expire and invalidate cached model lists;
- temporary suspension can hide one credential/model without hiding the model
  globally when other healthy credentials support it;
- `/v1/models` uses local key scope, source/account filters, model rules, and
  health-aware registry state.

Static model metadata can be bundled as fallback and refreshed in background.
Remote/static refresh should detect changed source families and refresh only
affected credentials or registry shards.

Model registry availability:

- keep source-specific metadata beside global model info;
- cache `/v1/models` output per wire API and invalidate on registration,
  unregistration, quota, suspension, or model metadata changes;
- quota-exceeded marker should expire after a short window;
- suspension reason should distinguish quota from hard unsupported/unavailable;
- model remains visible when at least one scoped healthy credential can serve
  it;
- if all credentials are cooling down only because of quota windows, visibility
  can stay with a cooldown reason in detail view;
- if all credentials are hard-suspended or unsupported, hide model from default
  catalog and expose reason in diagnostics.

## Runtime Pipeline Notes

Some local gateway designs use two runtime paths:

- legacy Rust proxy inside the Tauri backend;
- Go sidecar proxy generated from the same local collection.

Do not replicate that dual runtime. For Zenith, keep one local gateway runtime
with a clean module boundary. The useful behavior is the pipeline, not the
structure.

### Persisted Collection Shape

A monolithic `collection` shape combines:

- service state: enabled, port, bind scope, client host, gateway mode, proxy;
- key records: default key plus named keys;
- account membership: collection account ids and per-key account ids;
- routing: automatic tie-break policy and retry limits;
- model rules: aliases, hidden/excluded models, per-account excluded models;
- timeouts, debug flag, stats, and health snapshots.

Zenith should split this into `GatewaySettings`, `LocalGatewayKey`,
`LocalAccount`, `ProviderSource`, `ModelRule`, and `UsageLog` records. This avoids a large
all-in-one local-access module and makes source/account/provider-specific
behavior testable.

### Local API Key Resolution

Required behavior:

1. Extract bearer/local key from request.
2. Match enabled key from collection keys.
3. Fall back to legacy collection key only for old configs.
4. Touch `last_used_at`.
5. Resolve key policy:
   - provider gateway override;
   - account id scope;
   - model prefix;
   - allowed models;
   - excluded models.

Zenith MVP should implement named local keys without legacy fallback. Each key
should have:

```text
enabled
label
secret ref
source/account scope
allowed/excluded models
optional model prefix
last_used_at
created_at/updated_at
```

### Local Key Scope And Deletion Cleanup

Effective candidate scope:

```text
key.source_ids/key.account_ids set -> use key scope
key.source_ids/key.account_ids empty -> use default pool membership
```

Rules:

- enabled local keys must have a secret and at least one usable source/account
  after inherited scope resolution;
- deleting a source/account removes its id from default membership, local key
  scopes, per-account model rules, profile
  bindings, scheduler state, and model registry;
- if a local key loses all usable scope, keep the key record but mark it
  unavailable until user edits scope or pool membership;
- if deletion affects a running gateway, reload or restart the affected runtime
  shard immediately;
- deleting current profile binding should return a restore/reattach action.

### Model Visibility And Rewrite

The gateway should build `/v1/models` from collection models, health-aware
availability, source model lists, aliases, hidden models, and per-key filters.

Request handling then:

1. strips key prefix;
2. maps alias to canonical model;
3. validates model is visible for that key;
4. rewrites request body to canonical model before dispatch.

Zenith should keep this order. Important rule: model aliases and prefixes are
local compatibility features only. They must not affect Zenith API public model
catalog/pricing.

### Model Policy Semantics

Local model policy should support:

- per-key `model_prefix`, stripped before validation and dispatch;
- per-key `allowed_models`;
- per-key `excluded_models`;
- global hidden/excluded model rules;
- per-source/account excluded model rules;
- aliases from visible client model to canonical source model;
- alias `fork` behavior: show both original model and alias when enabled;
- wildcard rules such as `gpt-5.4-*`.

Normalization rules:

```text
trim whitespace
trim model prefix slashes
case-insensitive dedupe
ignore empty rules
reject alias when source_model == alias
resolve dated snapshot aliases to canonical model when supported
```

Dispatch rule:

```text
client model -> strip local prefix -> alias/canonical resolution
-> validate against key/global/account policy -> rewrite request body model
```

If a client model is not visible for the local key, return a normal
OpenAI-compatible `model_not_found` or `permission` style error. Do not dispatch
and wait for the selected source to reject it.

### Runtime Config Updates

Config/runtime changes should update only affected objects:

```text
typed command -> validate -> diff old/new -> redacted change log
-> update store -> update runtime credential/source shard
-> refresh scheduler/model registry -> emit frontend snapshot
```

Rules:

- ignore no-op writes by comparing normalized state hash;
- ignore empty/incomplete writes from interrupted saves;
- debounce repeated source/account/model-rule updates;
- batch credential updates and dedupe by stable credential id;
- keep runtime-only credentials merged with persisted credentials;
- remove scheduler/model registry entries when source/account is deleted;
- force credential refresh when model alias, excluded model, retry, or auth
  metadata changes.
- when reacting to a file/profile watcher event, skip persistence write-back so
  the app does not create an infinite save/reload loop.

Do not rebuild the whole pool after every UI edit unless storage migration or
schema repair requires it.

### File Watcher And Hot Reload

Profile/config watchers should feed the same runtime update path as UI commands.

Watcher rules:

- debounce config reload events;
- ignore empty config writes from interrupted saves;
- hash file content and skip reload when hash did not change;
- keep old config snapshot to produce a redacted change list;
- lock auth directory to configured app store when mirroring is enabled;
- batch add/modify/delete credential updates and dedupe by credential id;
- merge runtime-only credentials with file-backed credentials before diffing;
- normalize volatile fields before comparing auth records so timestamps do not
  cause pointless reloads;
- stop dispatcher and clear pending updates on shutdown.

Auth update event:

```text
action: add | modify | delete
credential_id
credential_snapshot
source: ui | file_watcher | runtime
```

Config reload should classify what changed:

```text
auth_dir_changed
retry_policy_changed
model_alias_changed
model_exclusion_changed
transport_changed
source_definition_changed
```

Only affected credentials and registry shards should refresh. Full rebuild is
reserved for schema migration, corrupted store repair, or auth directory change.

### Request Translation

The gateway should normalize:

- `/v1/responses` and `/v1/responses/compact`;
- `/v1/alpha/search` for Responses Lite web search through OAuth accounts;
- `/v1/chat/completions` into `/v1/responses`;
- `/v1/images/generations` into `/v1/responses` with image tool payload;
- `/v1/images/edits` multipart/JSON into `/v1/responses`;
- SSE Responses streaming by default and a WebSocket compatibility path;
- later runtime can expose `/v1/messages` and `/v1beta` Gemini routes.

Zenith first runtime should ship:

```text
GET  /v1/models
POST /v1/responses
GET  /v1/responses (WebSocket upgrade)
POST /v1/responses/compact
POST /v1/chat/completions
POST /v1/alpha/search
```

The managed client target is `POST /v1/responses` with `stream=true` over SSE.
Reuse pooled HTTP connections and HTTP/2 where available. Keep the WebSocket
upgrade route for explicit compatibility, but do not advertise it. The
automated SSE and WebSocket correctness matrix passes at 1, 20, and 200
concurrent requests; release probes retain retry, cancellation, continuity,
and performance coverage.
Public clients and the REST/JSON management plane do not use gRPC or JSON-RPC;
Tauri invoke already owns desktop RPC. Add another protocol only for a measured
internal bottleneck or a real future plugin contract.

Compact and alpha-search are account-only paths: API-key sources are skipped,
while local-key scope, quota, health, cooldown, active load, proxy, and retry
policy still apply. Responses Lite strips hosted/server-executed tool
declarations and keeps only client-executed function, custom, and tool-search
entries before an OAuth request is sent. Then add images. Add `/v1/messages`
after OpenAI/Codex local path is stable.

### Account Candidate Selection

Candidate scope should work this way:

```text
key.account_ids empty -> collection.account_ids
key.account_ids set   -> key.account_ids
```

Then it applies:

1. model, health, cooldown, quota, and Free-policy hard filters;
2. mandatory previous-response binding;
3. API-source role tier;
4. OAuth preference and greatest remaining quota;
5. active load and dispatch balance only between equal-quota candidates;
6. prepared account/token refresh.

Zenith should use the same logical order, but with source/account terminology:

```text
scope -> hard gates -> response owner -> routing order -> executor
```

Hard gates always run before response ownership or ranking. A continuation may
use only the healthy, model-capable candidate that created its response; it is
never replayed through another account.

### Execution Attempt Loop

Each request should run through an attempt loop:

1. Resolve selected local key and scoped sources/accounts.
2. Pick candidate via scheduler.
3. Mark candidate as tried for this request.
4. Prepare auth/account if the source requires just-in-time refresh.
5. Execute canonical model or model pool entry.
6. Mark result with success/failure, status, retry-after, and model id.
7. If request shape is invalid, stop immediately.
8. If failure is retryable and no application event was sent, try next
   candidate. Once a stream event is forwarded, replay is forbidden.
9. Stop when `max_retry_candidates` is reached.

`tried` and `attempted` should be separate:

- `tried`: candidates selected by scheduler this request;
- `attempted`: candidates that actually reached auth preparation or execution.

This prevents infinite loops when a candidate has no usable model mapping, while
still allowing another source/account to be selected.

Source-specific request preparation must be protected by a per-account lock.
Concurrent requests should not run several refresh/prepare flows for the same
account at once.

Selected credential metadata should be attached to request context before
execution. Logs and UI can then show which local key, account/source, model
mapping, and retry attempt were used without parsing source responses.

### Execution Manager Contract

Gateway runtime should use one manager for credential lifecycle, selection,
execution, refresh, and persistence.

Manager owns:

```text
credential store
executor registry
selection strategy
scheduler
model registry hooks
request prepare locks
runtime config snapshot
per-credential transport
usage hooks
auto-refresh loop
```

Execution loop rules:

- keep `tried` and `attempted` maps per request;
- stop when `max_retry_candidates` attempted credentials is reached;
- if selection fails after a previous candidate failed, return last execution
  error when it is more useful than generic `no candidate`;
- prepare request auth after candidate selection and before executor call;
- mark result for prepare failures and executor failures;
- retry next candidate for transient/auth/quota errors;
- stop immediately for request-invalid errors;
- publish selected credential id into request metadata before execution;
- apply per-credential HTTP transport through execution context, not global
  mutable client state.

Request auth preparation:

- executor declares whether a credential needs just-in-time preparation;
- per-credential lock protects preparation;
- after lock, reload current credential snapshot and re-check if preparation is
  still needed;
- persist updated credential and return persisted snapshot;
- failed preparation updates health/cooldown and can try next candidate.

Model pool execution:

- route model can expand to several source model choices;
- try source models inside selected credential before picking a new credential;
- log requested model alias separately from resolved/source model;
- request-invalid error from one model stops the request instead of trying all
  fallbacks blindly.

### Routing Strategies

The runtime offers automatic and highest-quota strategies, plus explicit
subscription-expiry and subscription-plan orders. A local key may narrow its
scope to a single account/source or a selected set, but visible list sorting
never changes runtime order. Automatic uses:

```text
hard filters
API-source role
OAuth preference inside the stabilizer tier
greatest minimum remaining quota after the protected-account reserve
active load between equal-quota candidates
committed dispatch balance
stable id tie-break
```

Highest-quota keeps the same hard filters, source role, OAuth preference, quota
comparison, and stable tie-break, but ignores active load and dispatch balance.

In automatic and highest-quota modes, subscription plan names and expiry dates
do not determine runtime priority. Manual priority remains an advanced final
tie-breaker rather than a routing group that starves otherwise eligible
accounts.

### Response Continuity And Transport Ownership

Zenith does not infer or persist hard chat-to-account bindings from headers,
metadata, `conversation_id`, or request content. A client-supplied
`prompt_cache_key` is a best-effort hint: hash local key identity, resolved
model, and cache key; bind only after success; keep at most 4096 entries in
memory for one hour; and ignore it when the candidate is busier than the
baseline or trails its quota by more than 500 basis points.

Two protocol constraints remain mandatory:

- `previous_response_id` maps to the account that created the response. If that
  account is unavailable, the continuation fails instead of replaying through a
  different account;
- when the compatibility path is used, an active WebSocket owns its current
  upstream account for continuation messages on that connection. A new
  independent SSE or WebSocket request is scheduled normally.

Response ownership uses a bounded 30-day cache and is invalidated when its
candidate is removed. This cache is protocol correctness, not an optional
routing preference.

### Cooldown And Health

The runtime should track both account health and per-account per-model
cooldowns.

Local proxy path:

- success clears model cooldown and consecutive failure state;
- 429 `usage_limit_reached` parses `resets_at` or `resets_in_seconds`;
- cooldown can make all candidates return 429 with a retry message;
- network errors and auth errors mark account failure;
- image capability errors mark image status unavailable.

Scheduler state:

- `ModelState` stores status, unavailable flag, next retry time, last error,
  quota state;
- success resets model state;
- 401/402/403 usually cooldown 30 minutes;
- 404 or unsupported model cooldown 12 hours;
- 429 uses provider `Retry-After` or backoff;
- 5xx/timeout cooldown around 1 minute;
- aggregate auth availability follows model states.

Zenith should model this explicitly:

```text
AccountHealth
  auth_state
  consecutive_failures
  last_success_at
  last_failure

ModelHealth
  account_id/source_id
  model_id
  unavailable
  next_retry_at
  reason
  last_error
```

Do not hide "all candidates cooling down" as generic 503. Return clear local
diagnostic to the app UI; for client API response, use normal OpenAI-compatible
error shape.

Concrete cooldown policy:

```text
success              -> clear model state and quota marker
401                  -> 30m model cooldown, auth warning
402/403              -> 30m model cooldown, payment/access warning
404 unsupported      -> 12h model cooldown
429 with Retry-After -> use Retry-After
429 without header   -> exponential backoff 1s..30m
408/500/502/503/504  -> 1m model cooldown
context too large    -> request invalid, no retry
bad request shape    -> request invalid, no retry
```

Request-scoped 404 should not poison the account globally when it is clearly
about a missing `previous_response_id` or stale conversation state. It should
return to the client or retry only through a safe transcript-rebuild path.

### Probe And Error Classification

Gateway tests should classify failure stage, not show raw source errors only.

Suggested categories:

```text
local_gateway_unreachable
local_key_invalid
account_auth_failed
quota_or_rate_limited
model_capacity
context_too_large
image_generation_not_enabled
account_pool_empty
source_unavailable
request_invalid
stream_incomplete
client_canceled
```

Retryable failures:

```text
401 auth failure after candidate selected
408 timeout
429 quota/rate limit/model capacity
500/502/503/504 transient source error
image_generation_not_enabled when another image-capable account exists
```

Non-retryable failures:

```text
invalid local API key
model blocked by local key policy
bad JSON/body shape
context too large
unsupported endpoint
client canceled request
stream already emitted payload bytes
```

UI should display stage + action: local gateway, local key auth, account auth,
quota, source status, model policy, or client request shape.

### Retry And Streaming

Critical behavior:

- Retry another account/source only before response bytes reach the client.
- For streaming, read a bootstrap chunk first. If the selected source errors before first
  payload, try another model/account. After first payload, log failure but do
  not transparently retry.
- Single-account mode can retry the same account for selected transient statuses.
- Global retry waits are capped by max retry interval.

Zenith gateway must keep this invariant. It prevents duplicated or corrupted
stream output.

Streaming bootstrap detail:

- create source stream and capture source headers before response goroutine;
- keep headers mutable until first payload so a bootstrap retry can replace
  them;
- retry bootstrap errors only while no payload bytes were sent;
- retryable bootstrap statuses include auth failure, payment/access failure,
  request timeout, rate limit, and 5xx;
- validate SSE `data:` payloads for Responses streams before forwarding;
- after first payload, terminal stream error is logged and returned as stream
  error only when protocol allows it, never as transparent retry;
- client cancellation stops retry loop and avoids marking unrelated candidates
  unhealthy.

### Refresh Loop

Quota/auth refresh should run from a scheduler, not a fixed scan loop.

Use:

- min-heap by `next_refresh_at`;
- dirty set for accounts changed by UI or request results;
- bounded worker pool;
- provider/account-specific refresh lead time;
- failure backoff and ineffective-refresh backoff;
- stop scheduling after unrecoverable unauthorized refresh failure;
- skip API-key-only sources unless they expose a real quota refresh endpoint.

Refresh writes should update account quota, subscription, auth state, model
health, and scheduler shards in one transaction-like step.

If a refresh succeeds but the credential is still immediately due for refresh,
set a short ineffective-refresh backoff. This prevents tight refresh loops when
a source returns stale expiry data.

Do not schedule API-key-only sources for auth refresh unless their protocol
adapter declares a real refresh or quota endpoint. Show `quota unsupported`
rather than treating this as unhealthy.

Refresh loop implementation contract:

```text
min_heap(next_refresh_at)
dirty_set(credential_id)
bounded_worker_pool
wake_channel
job_queue
```

Rules:

- only enabled accounts that participate in the pool are scheduled
  automatically; other accounts refresh on explicit request or when added;
- UI edits and request results enqueue dirty credentials;
- due credentials are popped from heap and sent to workers;
- if credential no longer exists or no longer needs refresh, remove it from
  heap;
- pending refresh state prevents duplicate worker jobs;
- refresh failure uses backoff;
- routine quota refresh does not rediscover an already-known model catalog;
- success that still leaves credential immediately due uses short ineffective
  backoff;
- unauthorized/revoked refresh marks reauth and stops auto-refresh until user
  fixes login.

### Quota Window Wake Jobs

Wake jobs reuse quota refresh events but run in a separate bounded queue from
client request routing. They must not add a fixed polling loop.

```text
quota transition or due reset refresh
-> wake eligibility filter
-> cycle dedupe ledger
-> per-account lock
-> optional jitter
-> minimal request
-> delayed quota verification
-> redacted history event
```

Rules:

- adapter declares wake-capable windows and their user-facing labels;
- full-window detection uses normalized state, not provider-specific hardcode;
- normal client use after the full transition cancels the pending wake;
- one account/window cycle produces at most one confirmed wake;
- wake request uses the lightest supported model, no tools, and capped output;
- generated content is discarded and never enters request logs;
- verification failure becomes `unconfirmed`, not an infinite retry loop;
- local runtime pauses jobs when the app closes; remote runtime persists them.

### Translator Pipeline

Use a registry-based translator layer:

```text
source_format + target_format + model + stream flag -> request transform
source_format + target_format + model + stream flag -> response transform
```

When no transform exists, pass the body through but still normalize the `model`
field to the resolved canonical model. This prevents local key prefixes and
aliases from leaking to the selected source.

Request and response middleware should be available for redaction, tracing,
token capture, and compatibility fixes. Keep translators independent from
scheduler and pricing.

Translation context should carry:

```text
source_format
target_format
resolved_model
client_requested_model
stream
headers
query
original_request_body
translated_request_body
selected_credential_id
execution_session_id
reasoning_effort
service_tier
request_path
```

Response translators need state for streaming. Store per-stream mutable state in
a request-scoped object, not global state. This is required for accumulating
tool calls, reasoning blocks, usage chunks, and final stop reasons.

Protocol rules:

- request fallback may pass body through, but must still rewrite `model` to the
  resolved canonical model;
- response fallback can pass raw body through only when target format equals
  source format or client explicitly requested raw passthrough;
- token-count responses should use a dedicated transform path;
- translators may read usage metadata but must not perform billing or routing;
- translator tests need both streaming and non-streaming fixtures for tool
  calls, reasoning, images, cache read/write usage, and incomplete responses.

For Claude/OpenAI-compatible paths, keep cache usage fields distinct:

```text
input_tokens
output_tokens
cache_read_input_tokens
cache_creation_input_tokens
cache_creation_input_tokens_1h
```

OpenAI-compatible output can expose cached input through
`prompt_tokens_details.cached_tokens`, but internal usage must preserve
cache-write buckets separately for local stats and future provider adapters.

### Execution Context And Hooks

Execution should use a single request context shared across scheduler,
translator, executor, usage capture, and logging.

Required execution fields:

```text
provider-facing request
execution options
selected credential snapshot
translator pipeline
per-credential HTTP client/transport
stream flag
request headers/query
original request bytes
client/source formats
metadata
```

Useful metadata keys:

```text
requested_model
request_path
pinned_credential_id
selected_credential_id
selected_credential_callback
execution_session_id
disallow_free_accounts
reasoning_effort
service_tier
```

Hooks:

- before execute: attach trace ids, selected credential, transport, timeout;
- after execute: publish usage, update health, record latency;
- on stream chunk: record first payload, last payload, chunk errors, usage
  events when available.

Hooks must not mutate scheduler decisions after execution starts.

### Request Logs And Stats

Request logs should include:

- request id;
- account id/email;
- API key id/label;
- model id;
- gateway mode;
- request kind;
- success/status/error category/message;
- latency;
- input/output/total/cached/reasoning tokens;
- local estimated cost;
- pricing snapshot.

Zenith local logs should keep the same diagnostic shape, but avoid exposing
secrets and avoid implying Zenith backend billing. Local estimated cost is only
local informational math.

Telemetry DB baseline:

```text
request_logs
  id
  event_key unique
  timestamp
  request_id
  account_id
  account_label_or_email
  local_key_id
  local_key_label
  model_id
  gateway_mode
  request_kind
  success
  http_status
  error_category
  error_message
  latency_ms
  input_tokens
  output_tokens
  total_tokens
  cached_input_tokens
  reasoning_tokens
  estimated_cost_usd
  pricing_snapshot_version
  input_usd_per_million
  output_usd_per_million
  cached_input_usd_per_million
```

Indexes needed:

```text
timestamp
model_id + timestamp
account_id + timestamp
local_key_id + timestamp
gateway_mode + timestamp
request_kind + timestamp
success + timestamp
error_category + timestamp
request_id + timestamp
```

Aggregate windows should be rebuilt from `request_logs`, not trusted as the
source of truth. Reprice must update old logs when local estimate prices change.

### Usage Event Queue

Usage capture should be asynchronous:

```text
request execution -> usage record -> queue -> plugins/sinks -> SQLite/log/UI
```

Rules:

- request path must not block on telemetry sinks;
- sink panic/error must not crash gateway;
- shutdown drains queue when possible;
- usage record stores selected credential id and stable index;
- usage record stores alias/requested model separately from resolved model;
- failed attempts emit failure records with status/body summary;
- final successful attempt emits token and latency details;
- local estimated cost is computed by local telemetry sink, not executor.

The personal server adapter uses one bounded queue with capacity for 16,384
events. Its SQLite writer drains up to 256 events per transaction, coalesces
account-state writes within the same batch, and reports
`usage_persistence_failed` if the queue or sink cannot accept an event. A
single writer is intentional for SQLite; adding parallel writers increases
lock contention. PostgreSQL and multi-instance coordination belong to a
separate scaled service boundary and are not dependencies of the public
personal server.

Usage detail fields:

```text
input_tokens
output_tokens
reasoning_tokens
cached_input_tokens
total_tokens
ttft
latency
response_headers
```

Token fields follow the upstream usage object. `input_tokens` already includes
cached input, while `cached_input_tokens` is a breakdown and must not be added
to totals.
`output_tokens` includes reasoning and other generated non-visible formatting
tokens. Visible generation speed is
`max(output_tokens - reasoning_tokens, 0) / generation duration after first
output`. TTFT and end-to-end latency are reported separately. If explicit
generation duration is absent, derive it only when both latency and TTFT exist;
otherwise report unknown speed.

### Diagnostics And Support Bundles

Diagnostics should be useful without leaking secrets.

Rules:

- management/profile endpoints are never included in request body logs;
- query parameters and headers are masked before writing logs;
- request body capture is opt-in or limited to small failed requests;
- streaming diagnostics should record first-byte time, last-byte time, finish
  reason, and whether any payload was sent before failure;
- support bundle export redacts tokens, API keys, cookies, auth headers, local
  keys, and exact account identifiers unless user explicitly reveals identity;
- log file download and deletion must guard against path traversal;
- incremental log loading should support `after` timestamp and `limit`.

### Profile Attach And Restore

The app should write local Codex provider config and `auth.json`, then store
backups to restore prior API key or prior account login.

Zenith should keep this as a separate `profile.rs` module:

```text
inspect -> backup -> attach -> verify -> restore
```

Backup entries need profile path, previous config, previous auth, created time,
and source key id. Restoring previous login/API key must be visible even after
app restart.

Profile safety rules:

- create a backup only when current profile is not already managed by the same
  local key;
- update existing backup for the same profile instead of creating duplicates;
- restore only if current config/auth is still managed by the local key;
- if no backup exists but current config/auth is managed, remove only the
  managed blocks;
- preserve unrelated current config sections such as plugins;
- normalize TOML formatting through the existing config writer;
- inspect after attach and after restore so UI can show exact status;
- never overwrite a fresh user login that happened after attach.

### Per-Profile Gateway Lifecycle

When a profile is bound to a local account, local source, or generated local
API key, the gateway lifecycle is profile-scoped.

Start/update flow:

```text
resolve profile -> close old process if needed -> stop old profile gateway
-> write profile auth/config -> start profile gateway if required
-> sync idle threads -> sanitize config -> launch app or prepare CLI command
```

Stop/unbind flow:

```text
stop process -> stop profile gateway -> clear PID
unbind -> remove managed config/model catalog -> restore previous profile state
```

Important rules:

- a named instance must be initialized before binding;
- local gateway service state must not leak across profile directories;
- port conflicts should be detected and fixed before launch;
- managed model catalog and model override files need backup/restore;
- if a profile-specific gateway is no longer needed, kill it and wait for port
  release before starting the next one.

This keeps local pool behavior predictable when the user runs several Codex
profiles with different accounts or local API keys.

### History Visibility Repair

History/session repair belongs beside profile tools, not inside request
routing.

Gateway/profile module should expose:

```text
list repair instances
list repair target providers
dry-run repair
run repair
restore from backup if write fails
```

The dry-run result should say what will change before any file write. Repair
must back up session files, SQLite databases, and session index files before
mutation. Running instances should be called out because the user may need to
restart them after repair.

### Sidecar Config Lessons

If a sidecar runtime is added later, these contracts are useful:

- `apiKeys` manifest with labels and policy;
- `api-key-account-ids` scope map;
- auth directory with one file per OAuth account;
- API-key source records with base URL/key;
- account manifest with plan rank, remaining quota, subscription expiry;
- routing strategy and retry settings;
- max retry credentials and retry interval;
- global and per-account excluded model rules.

Zenith can implement these directly in Rust first. A sidecar is not required for
MVP unless we later need separate process isolation or reusable gateway binary.

## Local Management Boundary

Primary management surface should be Tauri commands. Local HTTP management is
optional and should stay disabled unless a local integration needs it.

If enabled:

- bind to `127.0.0.1` by default;
- validate `Host` header against localhost or the explicit LAN host list;
- require a generated management key, separate from local API keys;
- accept `Authorization: Bearer <key>` and optional `X-Management-Key` for
  local tooling only;
- reject local API keys on management routes and reject management keys on
  `/v1/*` model routes;
- accept remote/LAN management only behind explicit advanced setting;
- rate-limit failed management auth attempts;
- temporarily block a client after repeated failed management auth attempts;
- compare management keys in constant time;
- store persistent management key as hash, not plaintext;
- disable browser CORS access for management routes unless a trusted local app
  integration explicitly needs it;
- never use wildcard CORS with credentials;
- require confirmation tokens for destructive HTTP management operations so a
  local webpage cannot trigger state changes by guessing a route;
- never expose raw secret values through management responses;
- keep destructive actions as explicit commands with confirmation state;
- do not make management routes part of public customer API.

## Implementation Order

The only active build order and release gates live in
[local-pool-final-planning.md](local-pool-final-planning.md). This document owns
gateway architecture and runtime invariants only.

## Admin Boundary

Public local pool is for the user's own accounts and API keys. It stays local by
default.

Private admin flows are outside the public app contract and must be documented
in private backend docs. Public UI must not say or imply that normal user data
is sent to Zenith backend inventory.
