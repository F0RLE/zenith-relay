# Zenith Relay Active Roadmap

This file is the active implementation order. Open checkboxes are unfinished.
Checked P0 entries remain only as compact evidence until the whole phase gate
closes; completed phase history otherwise stays in Git and tests.

Behavior and ownership remain canonical in:

- [product-direction.md](product-direction.md)
- [project-structure.md](project-structure.md)
- [local-pool-runtime-contract.md](local-pool-runtime-contract.md)
- [local-account-auth-architecture.md](local-account-auth-architecture.md)
- [local-gateway-architecture.md](local-gateway-architecture.md)
- [app-ux-flow-spec.md](app-ux-flow-spec.md)

## Refactor Guardrails

- Finish and prove the Remote Pool product before structural refactoring.
- During product completion, move or rename code only when the change is
  required for correctness. Cosmetic module churn waits for P2-P5.
- Refactors must preserve behavior and public command/protocol contracts.
- Rust runtime snapshots own account status, eligibility, and routing order.
  React renders them and must not recreate scheduler policy.
- Explicitly exhausted OAuth quota is ineligible. Stale OAuth quota remains
  eligible below fresh quota and follows the existing refresh backoff.
- Secrets, prompt text, response text, proxy credentials, and raw account
  identities must not enter frontend state, telemetry, logs, or fixtures.
- Local Pool, Remote Pool, and Zenith API remain separate configuration paths.
- Remote account ownership changes are compensated state transitions, not CRUD
  shortcuts. Reconnect, disconnect, delete, restore, and profile switching must
  never silently create two active copies or strand the only usable session.
- `relay-server` owns the user-managed Remote Pool. The private
  `zenith-account-pool` remains a separate internal provider for Zenith-owned
  capacity and is not reused for application sessions.
- Do not add a root Cargo workspace or mass-move files merely to match the
  target tree.
- Update [project-structure.md](project-structure.md) before introducing a path
  not already present in its target tree.

## P0: Complete Remote Pool For The Application

Ship the user-managed server as the third working application mode before
reorganizing the implementation.

### Documentation Baseline Before Implementation

Before extending Remote Pool, replace accumulated product documentation with a
small verified public baseline:

- rewrite the root `README.md` from scratch as a concise product entry point:
  one current synthetic screenshot, the three modes, install/run links,
  privacy boundary, Help links, development checks, and license;
- remove screen-by-screen narration, internal module inventories, provider
  routing details, fallback topology, and implementation notes from the public
  README;
- add the intended Help paths to
  [project-structure.md](project-structure.md), then create separate versioned
  RU/EN guides for `Zenith API`, `This computer`, and `My server`;
- make the sidebar Help action open the Help center instead of resetting the
  onboarding wizard;
- implement Help as one normal full-page application view with three clear
  mode tabs; open the active mode by default and keep onboarding as a separate
  action inside the relevant guide;
- give every mode guide the same short structure: when to use it,
  prerequisites, setup steps, one complete working example, expected result,
  common failures, recovery, and where credentials are stored;
- keep examples synthetic and copyable, and never include real keys, account
  identities, proxies, provider routes, or internal server names;
- capture the baseline README screenshot through Playwright from synthetic
  state so its theme, viewport, version, and redaction are reproducible.

Mandatory Help examples:

```text
Zenith API
-> add a Zenith key
-> connect ChatGPT
-> send one request
-> find balance and usage

This computer
-> sign in or import one account
-> add it to the pool
-> start the local endpoint
-> connect ChatGPT
-> verify quota and request usage

My server
-> deploy or connect a server
-> move one account after preview
-> create/use the server client key
-> connect ChatGPT or another client
-> close the desktop app
-> verify the next request and statistics after reopening
```

Each example must state the expected visible result and the safe recovery action
for the most likely failure. Help must not contain feature marketing or repeat
controls that are already self-explanatory in the current screen.

The initial documentation describes only behavior that already works. Planned
behavior stays in this roadmap until its acceptance checks pass.

### Ready API Confidentiality

- Ready API surfaces show only Zenith API, the requested public model, customer
  usage, customer price, Zenith request id, and a user-safe status;
- upstream provider identity, owned-pool selection, external fallback,
  provider-specific errors, route order, upstream request ids, and internal
  cost never enter Relay state, translations, usage rows, support bundles,
  screenshots, or Help;
- the application must render Gateway and Control API public DTOs and must not
  infer provider state from latency, error text, headers, or model aliases;
- personal Local/Remote Pool sources remain visible to their owner because the
  user configured those sources. This is separate from hidden Zenith Ready API
  routing.

### Account Transfer

Implementation status (2026-07-22):

- [x] move selected local accounts without exposing credentials to the React
  frontend;
- [x] preview, confirm, and validate remote accounts in bounded groups of five
  before changing local routing;
- [x] keep local records and credentials, but disable and remove them from the
  local pool only after every remote group succeeds;
- [x] return created remote account ids for compensation and delete newly
  created remote records when a partial transfer or local commit fails;
- [x] preserve an existing remote account's display settings, proxy, model
  rules, priority, weight, and enabled state while updating its session; the
  explicit Move action still adds it to the remote pool;
- [x] expose per-account background progress for long transfers without
  weakening the all-or-nothing local routing commit;
- [x] persist the local-account to remote-account mapping and render the local
  copy as `On server` instead of disabled or unavailable;
- [x] return a moved account to the computer through a compensated operation:
  fetch and store the latest server credential in an inactive local record,
  validate it without routing, remove the remote copy, and only then activate
  local execution. An incomplete step must leave the local copy inactive;
- [x] protect a selected account that currently owns a managed direct ChatGPT
  profile: before transfer commit, use the existing reversible profile flow to
  attach that profile to the remote endpoint or explicitly restore/detach it.
  Failure rolls back the profile and transfer before remote ownership changes;
- [ ] pass the Remote Pool live-acceptance matrix with working accounts.

- let the user select local accounts or compatible JSON session files and send
  them to the selected server through preview and explicit confirm;
- normalize local and uploaded sessions to the same account model;
- process large imports as bounded background work with visible per-account
  progress, success, duplicate, and failure states;
- deduplicate by stable provider identity and update an existing remote session
  without losing its proxy, pool membership, model rules, or display settings;
- encrypt credentials before a remote account can become selectable, then
  discard the uploaded JSON payload;
- make active session ownership explicit: the normal action is `Move to
  server`, and the local copy leaves the local pool only after the server has
  stored and validated the session;
- keep exactly one active owner for a rotating OAuth session. The inactive
  local copy is a recoverable record, not a second runnable account;
- keep the stable server id and remote account id on the inactive local record
  so reconnect, display, return, and recovery never depend on labels or list
  order;
- never refresh or route the same rotating OAuth session concurrently from the
  local and remote runtimes.

### Remote Ownership Reconciliation And Recovery

Implementation status (2026-07-22):

- [x] persist a redacted ownership-operation journal before the first cross-host
  side effect. It records operation id, local account ids, pinned server id,
  confirmed remote ids, and phase, but no credential. Startup resumes or
  compensates interrupted move/return work instead of relying on in-memory
  rollback;
- [x] reconcile every saved `(server_id, remote_account_id)` after connect,
  restart, and manual refresh. A matching remote record stays `On server`; a
  missing record becomes `remote_missing`/recovery-required and is never
  automatically re-enabled locally;
- [x] keep identity changes fail-closed. Reconnecting to the same stable server
  restores links, while a new server identity never adopts mappings merely
  because labels or account positions match;
- [x] make destructive actions unambiguous: deleting the inactive local record
  removes only that recovery copy, deleting the remote record never silently
  activates an older local credential, and `Return to computer` remains the
  only normal operation that transfers active ownership back;
- [x] before forgetting a server connection, show the count of linked local
  recovery records and preserve their server ids. Disconnect removes the local
  management token but does not claim that remote accounts were deleted or
  returned;
- [x] provide an explicit lost-server recovery path for a preserved local
  credential. It stays inactive by default and may be force-activated only
  after warning that an unreachable remote copy could still be running.

Acceptance:

- moving the account used by a managed direct profile cannot leave that profile
  refreshing the same OAuth session outside the remote runtime;
- remote deletion from this or another management device is reflected after
  reconnect without a false healthy/disabled state;
- disconnect/reconnect to the same server preserves links, and an identity
  mismatch cannot mutate or reveal linked accounts;
- every interrupted move, return, delete, and forced recovery has one explicit
  recoverable owner state after restart;
- fault injection after every remote/local commit boundary proves a crash
  cannot leave an untracked runnable copy on both hosts.

### Autonomous Server Runtime

Implementation status (2026-07-22):

- [x] create and persist one encrypted server-managed ChatGPT client key without
  exposing it in ordinary key controls or runtime snapshots;
- [x] negotiate `profile_attach`, pin the returned API URL to the connected
  server origin, and consume the credential only inside the desktop Rust
  backend;
- [x] attach ChatGPT to the remote `/v1` endpoint through the existing
  reversible profile flow from the API screen;
- [ ] prove requests continue after closing the desktop app and that reopening
  shows matching server usage, speed, quota, and API-equivalent statistics;
- [x] prove graceful termination stops new work, settles or cancels bounded
  in-flight work, flushes queued terminal usage, and restarts without duplicate
  request rows. Forced termination must recover without inventing success;
- [x] prove one backup contains a consistent SQLite database plus encrypted
  vault, requires the separately stored original vault key, preserves server
  identity and client access, and restores without replacing a valid live store
  until integrity checks pass;
- [x] add bounded retention for raw usage/error/wake/import history so an
  always-on server cannot grow forever. Required per-key totals and active
  spending-window state must survive pruning.

- keep the gateway, quota refresh, token refresh, scheduler, usage capture,
  proxies, and wake tasks running after the desktop app closes;
- expose one generated remote client key and one OpenAI-compatible `/v1`
  endpoint without exposing the management token to model clients;
- configure the local ChatGPT profile for the remote endpoint through the same
  reversible attach/restore flow used by the local pool;
- keep management on REST/JSON and model streaming on SSE; WebSocket remains a
  compatibility adapter only;
- preserve server state across restart and provide tested backup and restore;
- support one active user-managed server in the application for the first
  release.

### Application Control And Monitoring

Implementation status (2026-07-21):

- [x] negotiate remote model-pricing support through an explicit protocol
  capability;
- [x] persist server-owned input, cached-input, and output price overrides and
  return them in the remote runtime snapshot;
- [x] recalculate stored remote usage from token counts when a price changes,
  including models absent from the built-in catalog;
- [x] edit and restore remote prices through the same Pool model table used by
  the local runtime;
- [ ] preview and atomically apply a versioned server configuration preset for
  pool order, model rules, existing proxy assignments, routing, and price
  overrides. The preview shows an exact diff against a server configuration
  revision; apply rejects a stale revision and rolls back every field on error;
- [ ] keep credentials, management/client key secrets, host/TLS/vault settings,
  usage, desktop paths, profile bindings, and other local-only state outside a
  preset. Unknown schema versions, unsupported fields, and missing referenced
  objects fail in preview rather than being silently ignored;
- [ ] let the user save a portable preset from visible Local/Remote Pool
  settings and explicitly apply it to the server. This is never background
  synchronization: reconnect reads server state and does not auto-apply the
  last local preset;
- [ ] add Remote client access under Gateway/Client Setup, not Pool internals:
  create a named key, reveal its secret once, list it masked, edit account/
  source/model/wire scopes, rotate it, and revoke it. The generated ChatGPT
  profile key remains hidden and system-owned;
- [ ] rotate the hidden profile key only through an explicit backend attach
  operation: verify the updated managed profile before revoking the old key,
  and never expose either value to React or ordinary key controls;
- [ ] prove the remote user-owned API-source lifecycle end to end: create,
  test/discover models, edit, disable, route one request, rotate the stored
  source key, and delete. The source secret is accepted only by the desktop
  backend/server and is never returned in a runtime snapshot;
- [ ] expose per-client-key request cost and optional budgets from a
  server-owned ledger using integer micro-USD. Exactly one terminal request id
  contributes cost despite retries/fallback, and key rotation or deletion does
  not erase historical attribution;
- [ ] call a budget a hard limit only when concurrent requests use atomic
  reservation/settlement without double charge. Otherwise expose it explicitly
  as a soft alert. Payments, purchases, refunds, and reseller balances remain
  outside Relay Server;
- [ ] test supported version overlap, an older server missing an optional
  capability, incompatible protocol versions, server identity change, token
  rotation, and disconnect while a management request is in flight;
- [ ] add stable capability names and versioned protocol DTOs for presets,
  client access, and budgets before enabling their controls. Older compatible
  servers omit the capability and keep the rest of Remote Pool usable;
- [ ] pass the Remote Pool live-acceptance matrix with working accounts and
  compare launcher statistics against server usage.

- render server-owned accounts, canonical status, quotas, pool order, models,
  proxies, request usage, speed, and errors from the remote runtime snapshot;
- send user-edited model input, cached-input, and output prices to the server;
- make the server the source of truth for remote price overrides and calculate
  API-equivalent usage from stored token counts without storing prompt or
  response text;
- keep the server as the source of truth for remote pool configuration and
  usage. Reconnecting the application reads server state; it never silently
  pushes a stale local copy;
- broad configuration writes include the server revision they were previewed
  from. A conflict reloads the server snapshot instead of overwriting edits
  made by another management device;
- keep the management token inside the desktop backend and use separate,
  revocable client keys for `/v1` traffic;
- do not overwrite remote settings from stale local settings when the app
  reconnects;
- show connection loss without stopping or mutating the server runtime;
- support connect-existing and Docker deployment through the same negotiated
  server protocol and pinned server identity.

Acceptance:

```text
local accounts or JSON sessions
-> preview and confirm move
-> encrypted remote storage
-> remote quota/model validation
-> remote /v1 request and streaming response
-> close desktop
-> remote request still succeeds
-> reopen desktop
-> matching quota, status, usage, speed, and API-equivalent statistics
```

- failed or partial transfer never removes the working local session;
- the server never persists raw imported JSON or returns stored credentials in
  normal management responses;
- parallel chats, account rotation, proxy routing, cancellation, and retry
  boundaries pass real-server tests;
- remote source create/test/use/rotate/delete passes with no secret in the
  snapshot, usage row, logs, or support data;
- named client-key create/use/rotate/revoke, scope denial, terminal cost, and
  concurrent budget behavior pass server tests without exposing key values;
- configuration preview/apply, stale-revision conflict, restart, backup, and
  restore preserve one server-owned result;
- Remote Pool can be used end to end without a source-tree refactor.

### Remote Pool Live Acceptance

Run this gate immediately after P0 implementation and before structural
refactoring. It uses working accounts already shown in the application's
Connections page; credentials remain inside the normal app/vault flow and are
never copied into commands, fixtures, screenshots, or logs.

Preparation:

1. Select one healthy account for the basic run and a second healthy account
   for rotation/concurrency when available.
2. Record the current local quota/status and create the normal recoverable
   transfer state.
3. Move the selected accounts through the application's `Move to server`
   preview/confirm flow. Do not run the same rotating OAuth session in local
   and remote runtimes simultaneously.
4. Require successful remote credential, quota, model, proxy, and pool
   validation before removing the local account from active routing.
5. If a selected account owns the managed direct ChatGPT profile, exercise the
   explicit attach-or-restore choice and verify its rollback path once.

Run through the server endpoint:

- one non-streaming request;
- one normal streaming request and visual chunk-cadence check;
- two parallel chats to prove free-account preference and only-candidate
  sharing;
- one intentional safe request failure to verify the Errors view;
- one request after changing the model input, cached-input, and output price
  override in the launcher.

For every successful request, the Remote Usage page must show exactly one row
with the same contract as `This computer`:

```text
time and Zenith request id
success/error status
requested and resolved model
safe account/source label
wire API
latency, TTFT, and generation duration
visible output speed in tokens/second
input, cached input, cache-write input, reasoning, output, and total tokens
API equivalent in integer micro-USD rendered as US dollars
```

Parity checks:

- local and remote pages use the same columns, details dialog, filters,
  pagination, Models/Connections/Errors aggregates, and daily/weekly/monthly
  buckets;
- request totals equal the sum of stored rows without counting attempts twice;
- cached input, cache-write input, and reasoning remain breakdowns and are not
  added to total tokens a second time;
- speed is derived from visible output tokens and generation duration; missing
  timing displays unknown instead of zero or an invented value;
- API equivalent uses the server-owned versioned catalog or remote price
  override and the same integer calculation as local mode; unknown price/input
  split remains visibly unpriced;
- account/source labels may come from the current snapshot, but raw email,
  provider account id, tokens, proxy credentials, prompt, response, and raw
  headers never appear in usage payloads.

Always-on proof:

1. Close the desktop launcher without stopping the server.
2. Send a streaming request through the remote client endpoint.
3. Restart the server and send one more request.
4. Reopen the launcher and reconnect.
5. Verify both requests, token breakdown, timing, speed, API equivalent,
   quota impact, and aggregates survived without duplicate rows.

The gate fails if the UI substitutes cached local statistics for server data,
if reconnect overwrites server prices/settings, if a request is missing or
duplicated, or if Remote mode exposes fewer diagnostic fields than local mode.
After the test, leave the account on the server or use the tested recovery flow
to disable the remote copy before returning its preserved local record to
active routing.

Live run status (2026-07-21):

- [x] moved one healthy refreshable account, validated its encrypted remote
  session, fresh quota, six models, and pool membership before disabling local
  routing;
- [x] rejected and rolled back an access-only session, then added local UI and
  backend preflight guards so the same invalid transfer cannot start again;
- [x] verified the server-managed client key starts `/v1` immediately, remains
  hidden from ordinary key controls, and survives server restart;
- [x] completed one non-stream request and one SSE request; the stream returned
  its first text at 1.069 seconds and completed without a cadence stall;
- [x] completed two simultaneous chats through the only healthy account and
  stored two distinct successful usage rows without duplicate attempts;
- [x] produced one safe upstream `400`, stored one
  `upstream_invalid_request` row, and kept the account healthy and in rotation;
- [x] applied a live input/cached-input/output price override, matched the exact
  integer micro-USD calculation, preserved it and its usage across restart,
  then restored the official catalog price;
- [x] inspected the Remote Usage Requests, Models, Pool Members, and Errors
  views and confirmed server-owned rows, aggregates, token breakdown, TTFT,
  generation speed, end-to-end speed, and API equivalent are visible;
- [x] restarted the server twice and preserved the account, quota, pool state,
  models, generated profile key, settings, and usage without duplicates;
- [x] added local/server schema migrations and regression tests proving a
  failed candidate followed by a successful fallback produces one successful
  usage row, while every intermediate attempt still updates account health;
- [x] added a runtime regression test proving terminal OAuth authorization is
  attempted once, excluded immediately without a timer, and skipped by the
  next request;
- [ ] close the desktop process, send another streaming request, reopen and
  reconnect, and compare the resulting quota and usage. This must run outside
  the active Codex session because that process currently provides its relay;
- [ ] run live two-account rotation and proxy routing when a second healthy,
  refreshable account and a test proxy are available.

## P1: Stabilize Product Logic

Fix behavior against one tested Rust owner before changing names or paths:

- use one canonical account status and routing-order calculation for local and
  remote snapshots;
- consolidate quota discovery, freshness, refresh, exhaustion, and reauth
  classification;
- verify token-refresh ownership and locking across transfer, restart, and
  concurrent requests;
- remove arbitrary inactivity timers and cooldowns that do not come from a
  classified provider failure;
- prefer available quota and a free account, but allow one healthy account to
  serve multiple chats when it is the only candidate;
- retry only before response bytes reach the client and never replay visible
  streamed output;
- make import, deletion, proxy assignment, price updates, client-key ledger
  updates, preset apply, and runtime rebuilds transactional and restart-safe;
- cover every accepted rule with focused scheduler, quota, import, server
  restart, and streaming tests.

Acceptance:

- the same synthetic state produces the same account status and routing order
  locally and on the server;
- manual quota refresh checks every selected account and cannot leave stale UI
  state presented as current;
- two simultaneous chats do not cause spurious reauth, quota exhaustion, or
  account removal;
- failed refresh and provider errors produce one normalized actionable state.

## Refactor Sequence After Product Acceptance

After P0 and P1 pass, perform structural work in this order:

1. characterize accepted behavior with focused tests;
2. remove dead and generated files;
3. extract only functions that already have an independent responsibility or
   proven reuse;
4. split oversized modules along runtime ownership boundaries;
5. remove superseded paths and rename only genuinely false domain symbols,
   updating every caller and test in the same change;
6. reconcile the accepted implementation with
   [project-structure.md](project-structure.md) and record the final source tree.

Do not create wrappers, modules, or folders only to make the tree look tidy.

### Detailed Refactor Contract

The current hotspots are not refactored merely because they are large. They
are refactored because they contain more than one independent owner:

| Current module | Mixed responsibilities to separate |
| --- | --- |
| `src-tauri/src/main.rs` | Tauri bootstrap, Ready API HTTP client, public DTO parsing, top-up validation, profile switching, formatting, and integration tests |
| `src-tauri/src/local_pool/commands/accounts.rs` | Tauri inputs, file import, source import, account import, quota I/O, account mutation, rollback, deduplication, and record validation |
| `crates/relay-core/src/gateway/mod.rs` | HTTP routes, request translation, candidate execution, error classification, retry/cooldown, streaming, response translation, and usage extraction |
| `relay-server/src/http/management_api.rs` | resource handlers, duplicate import parser, validation, secret mutation, gateway diagnostics, usage queries, and error serialization |
| `relay-server/src/app.rs` | runtime rebuild, record mapping, proxy resolution, token refresh, and usage batch persistence |
| `relay-server/src/store/sqlite.rs` | all aggregates, usage analytics, affinity, backup, schema migration, and migration recovery |
| `ConnectionsPage.tsx` | five views, every dialog, import state, proxy assignment, account status sorting, and terminal-error classification |
| `styles.css` | tokens, reset, shell, every page, every table, dialogs, responsive states, and animation |

The accepted owner boundaries are:

- `relay-core` owns normalized records, import normalization, canonical account
  state, quota interpretation, scheduler order, request/response execution,
  error classification, retry/cooldown policy, streaming semantics, usage
  events, and redaction;
- `src-tauri` owns Tauri commands, desktop paths, file dialogs, OS secret
  storage, process/browser/tray operations, local listener lifecycle, profile
  snapshots, and the remote management client;
- `relay-server` owns HTTP authentication, server SQLite/vault transactions,
  backup/restore, worker scheduling, and always-on process lifecycle;
- React owns user input state, display-only filters, local table layout, and
  rendering typed snapshots. It never decides whether an account can route;
- Local and Remote Pool hosts implement the same `relay-core` behavior through
  different I/O adapters. They do not fork scheduler, quota, or error policy.

### Canonical Account State

Do not store or display one overloaded mutable `status` value. Preserve typed
inputs and derive one operational projection in Rust:

```text
account record
+ enabled_by_user
+ in_pool
+ secret_available
+ proxy_state
+ auth_state and reauth_reason
+ quota windows, freshness, and reset
+ account health
+ account+model availability
+ draining and in-flight count
-> operational_status
-> routing_eligibility plus exact block reason
-> runtime order
-> one AccountSummary for every screen
```

Rules:

1. Connections always shows `operational_status`, even when `in_pool=false`.
   Pool participation is a separate icon/field and must never turn status into
   `excluded`.
2. Explicit fresh quota exhaustion blocks selection until its provider reset.
   Stale quota is `quota_stale`, not invented exhaustion or reauthentication.
3. Authentication failure changes auth state only when the normalized provider
   classification proves reauthentication is required.
4. Request/model failures update account+model state. They do not erase a
   valid account-wide quota snapshot.
5. A successful request clears only the failure evidence that success proves
   recovered.
6. Manual quota refresh bypasses the automatic due-time filter, refreshes every
   selected account with bounded concurrency, persists every result, rebuilds
   runtime once, and returns only after the snapshot reflects those results.
7. The ChatGPT-interface reserve protects only the account selected for the
   direct ChatGPT profile; it does not reduce all pool accounts.
8. `runtime_order` is produced by `PoolScheduler`. React may search or group
   rows, but cannot recompute priority, quota tier, or terminal usability.

### Canonical Mutation Transaction

Every source/account/proxy/key/import mutation follows the same host sequence:

```text
validate command input
-> acquire the existing setup/account lock
-> read current records and referenced secrets
-> build the complete proposed state with relay-core helpers
-> write new/changed secrets
-> atomically persist records
-> update token authority and running runtime
-> emit one fresh snapshot
-> remove replaced secrets only after commit
```

On failure, restore records, runtime membership, token authority, profiles, and
newly written secrets from the captured transaction state. A command must not
call several public mutations and attempt an ad hoc reverse sequence.

Cross-host move/return cannot keep a database transaction open across network
calls. It uses the P0 ownership journal instead:

```text
persist intent
-> perform one idempotent remote phase
-> persist returned remote ids/phase
-> perform one local transaction
-> mark complete
-> remove journal only after the resulting ownership state is verified
```

Restart resumes or compensates from the last durable phase. Retry uses the same
operation id and cannot create a second remote record or activate both copies.

Import is a special instance of that transaction:

```text
bounded file read/upload
-> pure relay-core parse and normalize
-> redacted preview with stable item ids
-> user confirms item ids
-> bounded credential/quota/model preparation
-> deduplicate by stable provider identity
-> one commit per independent item plus one final runtime rebuild
-> per-item success/duplicate/failure result
```

Raw JSON and credentials never enter React state, progress events, normal
logs, or persisted preview metadata.

### Exact Refactor Batches

These are batch contracts, not a second competing phase order. Execute them as:

```text
R0 -> P2 -> P3 (R1, R3, R7) -> P4 (R8)
   -> P5 (R2, R4, R5, R6) -> R9 -> P6
```

Each batch ends by deleting the superseded implementation; leaving both paths
active is not accepted. A later batch may start only when its required earlier
owner exists and its behavior gate passes.

#### R0: Behavior Characterization

Before moving files, add focused tests for:

- the same local/server synthetic account producing identical operational
  status, routing eligibility, runtime order, and block reason;
- manual refresh of healthy, stale, exhausted, retryable-error, and reauth
  accounts;
- shared quota/auth fixtures for fresh exhaustion/reset, stale or malformed
  usage, `token_invalidated`, subscription tier/duration/absolute expiry, and
  missing expiry without inventing a term from the import date;
- one account serving two concurrent chats when it is the only candidate;
- no retry after the first customer-visible stream bytes;
- import preview/confirm/restart/cancel and rollback after secret, quota,
  persistence, runtime, or profile failure;
- multiple accounts sharing one proxy and deletion of a referenced proxy;
- accepted proxy input formats and the same fail-closed effective proxy for
  request, token refresh, quota refresh, model discovery, and wake execution;
- local-to-remote move, return, direct-profile handoff, reconciliation, safe
  disconnect, and crash injection at every ownership-journal phase;
- per-client-key terminal-cost idempotency across fallback/retry plus
  rotate/revoke and concurrent budget behavior;
- public Ready API state containing no internal Zenith provider details.

Use the existing synthetic fixtures. Do not create real-token golden files.

#### R1: Shared Import Normalization

Move pure parser and normalization ownership from
`src-tauri/src/local_pool/accounts/imports.rs` and the duplicate block in
`relay-server/src/http/management_api.rs` into
`crates/relay-core/src/accounts/import.rs`:

- `ImportFormat`, `ImportAuthMode`, preview status/warning/issue types;
- `parse_import`, `combine_import_documents`, JSON/JSONL/container and Zenith
  bundle parsing;
- bounded depth/item checks, token-shape normalization, safe metadata/base URL
  validation, stable identity seeds, masking, and redaction;
- neutral normalized account/source import items without desktop paths,
  Tauri types, secret backends, or server stores.

Keep in desktop/server hosts:

- file picker, dropped paths, upload limits, progress events;
- encrypted import-session persistence;
- secret writes, quota/model network preparation, DB transaction, rollback;
- management/Tauri request and response DTO conversion.

Replace `prepare_account_import`, `parse_batch_import_input`,
`normalize_batch_account`, JWT import parsing, and related redaction helpers in
`management_api.rs` with the core parser. Replace desktop-only parser imports
with the same API. Then delete the duplicate server parser and duplicate test
fixtures. One fixture corpus must pass both host adapters.

#### R2: Thin Desktop Account Commands

Reduce `local_pool/commands/accounts.rs` to Tauri DTOs and these orchestration
entry points:

- reveal/export;
- start/preview/resume/prepare/cancel/confirm import;
- update/enable/drain/delete;
- set proxy;
- refresh one/all quotas.

Move existing responsibilities as follows:

| Existing logic | Final owner |
| --- | --- |
| pure record merge, validation, model normalization, identity/dedup lookup | `relay-core/accounts/record.rs` and `accounts/import.rs` |
| import session lifecycle and encrypted preview references | existing `local_pool/accounts/import_session.rs` |
| import transaction, per-item commit, rollback, and final runtime sync | `local_pool/accounts/import_orchestrator.rs`, created only when R1 callers are ready |
| quota HTTP/proxy call | existing `local_pool/accounts/quota.rs` |
| quota classification/application | move `quota_service.rs` behavior into `relay-core/quota/refresh.rs` or `accounts/quota_state.rs` |
| bounded manual refresh orchestration | `local_pool/accounts/quota_refresh.rs` |
| credential/token authority | existing `credentials.rs`, `authority.rs`, and core `token_authority.rs` |
| runtime rebuild after committed mutation | `local_pool/host/runtime_sync.rs` |
| account deletion transaction and recovery | `local_pool/accounts/delete.rs` only if it remains independently large after import extraction |

Do not create a generic account service or repository trait. The command calls
one concrete orchestration function and converts its typed error.

#### R3: One Runtime Projection

Create `crates/relay-core/src/accounts/status.rs`. Its pure projection consumes
`AccountRecord`, candidate runtime state,
proxy/secret availability, quota freshness, and pool membership. It returns:

- `operational_status` and safe detail code;
- `routing_eligible` and exact block reason;
- canonical quota summary and reset time;
- current in-flight/draining state;
- subscription plan/expiry projection;
- selection position/reason only when the account participates in the pool.

Use it from `DesktopState::snapshot_with`, `AppState::snapshot`, server
`account_summary`, and the management API. Delete React functions
`automaticAccountTier`, `accountIsTerminallyUnusable`,
`currentAccountErrorCode`, and any sibling status/sort classifier after typed
fields replace them. Connections and Pool render the same summary; Pool adds
only pool-specific controls.

#### R4: Shared Gateway Execution

Split `crates/relay-core/src/gateway/mod.rs` along its already-existing flow:

```text
gateway/
  mod.rs          router construction and endpoint delegation
  auth.rs         local host/key validation
  request.rs      bounded body read and request normalization
  translation.rs Responses/Chat request and non-stream response conversion
  execution.rs    candidate attempt loop and token/proxy adapter calls
  errors.rs       provider error classification and retry/cooldown decision
  streaming.rs    bootstrap, chunk forwarding, terminal-event and usage parse
  response.rs     OpenAI-compatible local responses and redacted errors
```

Move the current groups without changing semantics:

- `read_json_object`, model resolution, account endpoint choice, and request
  normalization to `request.rs`;
- request/response/tool translation functions to `translation.rs`;
- `execute_account_endpoint`, `execute_client_request`, and `execute_request`
  to `execution.rs`;
- `AttemptFailure`, upstream error classification, retryability,
  rate-limit/reset parsing, and cooldown application to `errors.rs`;
- `bootstrap_stream`, `UsageStream`, SSE terminal parsing, TTFT, response id,
  and usage extraction to `streaming.rs`;
- local host/auth/error/response builders to `auth.rs` and `response.rs`.

Keep `images.rs` and `websocket.rs` as transport adapters. Do not merge them
into the request executor and do not introduce an executor trait until a
second real implementation requires it.

#### R5: Self-Host Management Resources

Turn `relay-server/src/http/management_api.rs` into a module directory. The
root owns route exports and the shared `ManagementError` serializer only:

```text
http/management/
  mod.rs
  sources.rs
  accounts.rs
  imports.rs
  proxies.rs
  keys.rs
  quota.rs
  routing.rs
  models.rs
  usage.rs
  gateway.rs
  automations.rs
```

Resource handlers perform authorization/input conversion and call `AppState`,
`Store`, vault, or core services. Shared validators remain in `mod.rs` only
when two resources actually use them. `diagnose_gateway` and
`internal_gateway_request` are removed from the normal UI contract if
diagnostics remain command-line only; otherwise they live only in
`gateway.rs` behind management auth.

Keep all existing management paths and response shapes during the protocol
compatibility window. Update the versioned capabilities contract before any
path or required-field removal.

#### R6: Server Runtime And Store

Reduce `relay-server/src/app.rs` to state construction, runtime rebuild, and
snapshot delegation:

- move record-to-runtime mappings (`runtime_source`, `runtime_account`,
  `runtime_key`, candidate quota/health and summaries) to
  `runtime_mapping.rs`;
- move queued usage batching, `persist_usage_batch`, and natural-use updates
  to `usage_persistence.rs`;
- move server token persistence/refresh adapters to
  `accounts/token_refresh.rs`;
- keep proxy selection as a small host adapter in `accounts/proxy.rs` or in
  `app.rs` while it remains trivial.

Split `store/sqlite.rs` only at real transaction boundaries:

```text
store/
  mod.rs             Store facade and shared connection lock
  records.rs         sources, accounts, keys, pool membership, settings
  imports.rs         pending import lifecycle
  automations.rs     tasks and wake state
  usage.rs           insert, page/filter, aggregates, API equivalents, clear
  affinity.rs        ResponseAffinityStore implementation
  migrations.rs      schema ledger, backup, recovery, validation
  backups.rs         explicit user backup/restore
```

All modules use the same concrete `Store`; do not create repository interfaces.
Multi-record pool changes, import confirmation, account deletion, and migration
remain single SQLite transactions. Migration numbers/checksums stay ordered and
are never rewritten.

#### R7: Ready API And Bootstrap

Move from `src-tauri/src/main.rs`:

- Ready API DTOs and response parsing to `ready_api/models.rs`;
- `api_get`, saved-key stats/models/usage and safe public error parsing to
  `ready_api/client.rs`;
- top-up parsing, validation, intent creation, and allowed URL handling to
  `ready_api/top_up.rs`;
- save/reset/activate/deactivate commands and reversible profile change to
  `ready_api/commands.rs`;
- platform/locale/state helpers to existing platform/relay command modules.

`main.rs` then contains module declarations, state construction, plugin/setup
hooks, command registration, tray startup, watchers, and shutdown only. Move
its embedded integration tests next to the new owners or into crate-private
test modules; do not expose internals publicly for tests.

#### R8: Frontend Views And Styles

Split React by user workflow, not by every small component:

```text
connections/
  ConnectionsPage.tsx       active tab, query, dialog coordination
  SourcesView.tsx
  AccountsView.tsx
  ProxyStorageView.tsx
  AutomationsView.tsx
  RemoteServerView.tsx
  ImportDialog.tsx

pool/
  PoolPage.tsx              members/models tab and data wiring
  MembersView.tsx
  MemberEditor.tsx
  RoutingSettingsDialog.tsx
  ModelRulesView.tsx

usage/
  UsagePage.tsx             tabs, query and selected request
  RequestsView.tsx
  AggregatesView.tsx
  ErrorsView.tsx
  RequestDetails.tsx
  useTableLayout.ts
```

Keep related small dialogs in their view file until they are independently
reused. Keep one stored table-layout hook for column order/width; it owns UI
layout only, not request filtering or backend aggregation.

Split `styles.css` into one import root and responsibility files:

```text
styles/
  tokens.css
  base.css
  shell.css
  controls.css
  tables.css
  dialogs.css
  pages/connections.css
  pages/pool.css
  pages/usage.css
  pages/settings.css
```

Move rules without restyling first. After screenshot parity, remove duplicate
selectors and only then make visual changes. Locale files remain one file per
language until translation ownership, not line count, requires namespaces.

#### R9: Legacy Removal And Final Names

After all callers use the new owners:

- remove desktop/server duplicate import and status classifiers;
- remove stale cooldown/inactivity compatibility fields that no persisted
  supported schema reads;
- remove old Help-to-onboarding paths, hidden pool-key UI, diagnostics UI, and
  profile-repair UI that the accepted product no longer exposes;
- remove unused Tauri wrappers and old local storage keys after one explicit
  migration read;
- rename only genuinely false names. Keep stable Tauri commands and protocol
  fields until their documented compatibility release expires;
- update `project-structure.md` to the tree that actually exists and delete
  planned files that never gained an owner.

### Refactor Acceptance Gates

Each Relay batch must pass the smallest focused tests plus the following when
its area is touched:

- local/server snapshot parity for status, quota, models, and order;
- import fixture parity, restart resume, cancellation, and secret-redaction
  scan;
- transactional account/proxy/key deletion and rollback fault injection;
- scheduler concurrency, affinity, reserve, stale quota, and only-candidate
  tests;
- streaming chunk cadence, cancellation, terminal-event, and no-retry-after-
  output tests;
- local/server restart, backup/restore, and management contract-version tests;
- remote ownership reconciliation, safe disconnect, linked-record deletion,
  return-to-computer, and lost-server recovery tests;
- per-client-key terminal-cost idempotency, scopes, rotation/revocation, and
  concurrent hard-limit reservation tests when hard limits are enabled;
- Playwright desktop widths/themes plus keyboard/accessibility checks;
- a source scan proving React contains no routing/auth/quota terminal-state
  decision and public Ready API text contains no private provider identity;
- file-size reduction is reported only as a result, never used as the pass
  criterion.

## P2: Source Tree Cleanup

Remove files and generated output that have no runtime owner:

- remove unused legacy local-pool and Ready API wrappers from
  `src/src/tauri.ts`;
- remove the obsolete Help-to-onboarding shortcut after the Help center owns
  user guidance;
- delete ignored `src/output/` Playwright output;
- remove the currently empty local directories `scripts/`,
  `src-tauri/examples/`, and `src-tauri/windows/` when no packaging task claims
  them;
- fold any still-valid rules from `ui-schematic.html`,
  `full-implementation-agent-prompt.md`, and `p0-baseline.md` into the
  canonical runtime/UX documents, then delete those superseded planning
  artifacts.

Acceptance:

- `rg` finds no imports of deleted modules;
- frontend check, unit tests, and build pass;
- Git contains no generated output.

## P3: Restore Ownership Boundaries

### Ready API

Move Ready API implementation out of `src-tauri/src/main.rs`:

- `ready_api/commands.rs`: Tauri command handlers;
- `ready_api/client.rs`: Zenith API requests and safe error parsing;
- `ready_api/models.rs`: key stats and usage DTOs;
- `ready_api/top_up.rs`: amount validation and top-up intent handling;
- keep `main.rs` limited to startup, state construction, command registration,
  tray setup, and shutdown.

Split `src/src/tauri.ts` by the same boundary:

- platform/window/updater helpers stay product-neutral;
- Ready API wrappers live under the Relay API layer;
- Local/Remote Pool calls continue through `relayCommands`.

### Canonical Runtime State

Remove presentation-side routing decisions from:

- `features/relay/components/Ui.tsx`;
- `pages/pool/PoolPage.tsx`;
- `pages/connections/ConnectionsPage.tsx`.

The backend snapshot must provide the canonical runtime order and operational
status. UI sorting may group or filter only when it cannot change routing
meaning.

### Shared Account Import

Create one pure account import parser in `zenith-relay-core` and reuse it from
desktop and server. Keep these host concerns outside the shared parser:

- file dialogs and dropped paths;
- secret storage;
- progress events;
- persistence and rollback;
- remote management transport.

Acceptance:

- local and remote imports produce the same normalized records and errors for
  the same synthetic fixtures;
- `main.rs` contains no Ready API business logic;
- UI tests prove backend order is rendered unchanged.

## P4: Frontend Structure

Split only files that currently combine independent screens or behavior:

- `styles.css` -> tokens/base, shell, tables, dialogs, and page styles;
- `ConnectionsPage.tsx` -> sources, accounts, proxy storage, automations,
  remote server, and import dialog views;
- `PoolPage.tsx` -> members, models, and routing controls;
- `UsagePage.tsx` -> request table, request details, aggregates, and the stored
  column-layout hook;
- `OverviewPage.tsx` -> pure analytics/bucket calculations plus their unit test;
- `Ui.tsx` -> product-neutral primitives and Relay domain presentation helpers.

Keep `RelayStateProvider.tsx`, locale files, and small pages together until a
real independent responsibility appears. Do not create one file per trivial
component.

Acceptance:

- each page entry owns navigation and data wiring, not every child view;
- no React file calculates backend eligibility or routing priority;
- visual and accessibility Playwright suites pass.

## P5: Rust Module Structure

### Shared Core

R1 shared import normalization and R3 one runtime projection must already be
accepted in P3. Apply R4 here: split the gateway into request, translation,
execution, errors, streaming, and response owners without changing behavior.

Keep `images.rs` and `websocket.rs` as their existing transport adapters.

Keep the scheduler algorithm together. Move its large test module only when
doing so does not require making private internals public.

### Desktop Host

R7 Ready API extraction must already be accepted in P3. Apply R2 here: make
`local_pool/commands/accounts.rs` a thin Tauri layer and move independent
import/quota/delete orchestration to the existing account domain.

Split `local_pool/profiles/codex.rs` only along existing behavior boundaries:

- local gateway profile;
- direct account profile;
- safe config/auth snapshot helpers.

Keep the desktop database migration sequence ordered in
`store/telemetry_db.rs`; file length alone is not a reason to fragment it.

### Self-host Server

Apply R5 and R6: split management handlers by resources, `Store` by actual
aggregate/transaction boundaries, and `app.rs` into runtime mapping, usage
persistence, and account token-refresh adapters.

Acceptance:

- no new cross-layer dependency;
- local and server behavior still share `zenith-relay-core`;
- Rust format, Clippy, unit, integration, restart, and backup tests pass.

After these gates pass, apply R9 legacy removal/final names and rerun the
cross-layer acceptance gates before P6.

## P6: Tests, Documentation, And Release

- split `operations.spec.ts`, `p3_accounts.rs`, and server `public_api.rs`
  tests by behavior so unrelated changes do not conflict in one file;
- keep synthetic fixtures shared and secret-free;
- update `project-structure.md` after each accepted move, not before speculative
  scaffolding;
- run the six desktop OS/architecture CI targets and server release workflow;
- publish the self-host server as versioned x64/ARM64 binaries and an immutable
  multi-architecture GHCR image. Generated Compose must pin a stable SemVer tag
  or digest and must never deploy `latest`;
- test a clean Docker Compose and single-binary install, persistent-volume
  restart, supported upgrade, interrupted migration, and rollback by restoring
  the pre-upgrade database plus encrypted vault with the separately held vault
  key. Do not claim that an older binary can open a newer schema;
- test graceful SIGTERM and forced termination under streaming load, usage
  flush/idempotency, retention pruning, bounded disk growth, and recovery from
  a full or read-only data volume;
- verify the previous supported desktop against the current server and the
  current desktop against the previous supported server. Missing capabilities
  degrade only their controls; incompatible versions remain read-only/offline
  and cannot mutate state;
- run real-provider probes for streaming, cancellation, retry boundaries,
  quota rotation, proxy isolation, and profile restore;
- run `Remote Pool Live Acceptance` with explicitly selected working accounts
  from Connections and retain only redacted request ids, counts, timing, token,
  quota, and API-equivalent evidence;
- rewrite the root README against the accepted product one final time rather
  than preserving stale implementation prose;
- recapture the one README screenshot and the Help screenshots from the final
  release build with synthetic redacted data;
- verify all three RU/EN Help guides from a clean install by following their
  steps literally, including their working examples and recovery paths;
- document self-host install/update/backup/restore for Docker Compose and the
  standalone binary, with Caddy and Nginx HTTPS examples, firewall guidance,
  management-token versus client-key separation, secret-file permissions, and
  the rule that the vault key is stored separately from backups. This remains
  final P6 documentation, not a prerequisite for earlier implementation;
- publish a versioned compatibility guide for users implementing another
  server behind `My server`: required health/capability/state endpoints,
  management versus `/v1` authentication, identity pinning, feature flags,
  error/redaction rules, import and usage DTO examples, and SSE behavior;
- publish one machine-readable REST contract and a synthetic conformance check
  that runs against `relay-server` and can be run against another compatible
  server. Do not build language SDKs before real consumers require them;
- hide update/deploy actions when the negotiated server does not support them;
  the first release must not imply SSH access or automatic VPS provisioning;
- publish release checksums, dependency/license notices, the exact server/image
  version, migration compatibility, and a tested rollback procedure;
- add a contract test that scans packaged UI text, Help, screenshots metadata,
  diagnostics, and Ready API fixtures for forbidden internal provider and
  fallback identifiers;
- remove or archive superseded planning/specification documents after their
  still-valid rules are folded into the minimal README, Help, runtime contract,
  and final project tree;
- configure Windows Authenticode signing for EXE, MSI, and NSIS after a trusted
  certificate is available;
- keep Tauri updater signing separate from Authenticode and publish
  `LICENSE.txt` plus corresponding source with releases;
- after one stable compatibility release, use measured traffic and failures to
  decide whether the WebSocket route can be removed.

## Completion

The roadmap is complete when:

1. Remote Pool accepts application-managed account transfers and remains
   operational after the desktop closes;
2. local and remote status, quota, routing, streaming, proxy, and usage logic
   pass the same behavioral contracts;
3. remote model prices and API-equivalent statistics are server-owned and
   editable from the application;
4. account ownership can move, reconcile, return, disconnect, delete, and
   recover without silently running two copies or losing the only copy;
5. remote configuration uses previewed versioned writes, and named client keys
   have tested scopes, rotation/revocation, exact terminal cost, and limits;
6. startup files contain wiring rather than product business logic;
7. runtime status and order have one Rust owner;
8. desktop and server share import normalization;
9. high-churn UI and backend modules have focused ownership;
10. generated and dead files are absent from the source tree;
11. the accepted implementation matches the documented final source tree;
12. README and Help describe all three modes with verified screenshots and
    working examples without exposing Zenith internal routing;
13. a third-party user-managed server can implement and verify the versioned
    public compatibility contract without Zenith private backend knowledge;
14. versioned self-host artifacts, upgrade/backup/restore, bounded retention,
    compatibility, local checks, cross-platform builds, real-provider probes,
    signing, updater verification, and Remote Pool live statistics parity pass.
