# Zenith Relay Local Account/Auth Architecture

## Scope

This document defines account, auth, import, quota, and profile-projection
contracts for Zenith Relay local pool. It is internal planning material. Public
UI and public docs must use only Zenith terms: `account`, `source`, `local
gateway`, `local API key`, `quota`, `health`, and `profile`.

Do not expose private operational details in public surfaces.

## Backend Split

Exact repository paths and package names are owned by
[project-structure.md](project-structure.md). This section owns account/auth
responsibilities, not the canonical tree.

Keep desktop account/auth adapters under `src-tauri/src/local_pool/`. Reusable
credential, token, quota, and candidate behavior belongs in
`crates/relay-core`; keep both separate from gateway route orchestration:

```text
local_pool/
  accounts/
    account_store.rs      account records, current profile state, index repair
    oauth.rs              OAuth login sessions, callback listener, token exchange
    token_authority.rs    refresh locks, token rotation, reauth state
    imports.rs            auth file/API key/token import preview + confirm
    quota.rs              quota, subscription, reset-credit refresh
  profile/
    codex.rs              Codex profile inspect/attach/restore
    opencode.rs           OpenCode profile inspect/attach/restore
    instances.rs          named profiles and process binding
    repair.rs             history visibility repair
  sources/
    providers.rs          user API-key source records and model discovery
  scheduler/
    selection.rs          runtime candidate selection only
  store/
    secret_store.rs       local secret refs and encrypted fallback
    profile_backups.rs    profile attach/restore backups
```

Scheduler consumes normalized account/source health. It must not parse auth
files, refresh tokens, mutate profiles, or repair account stores.

## Account Records

Use account detail records plus small index/cache. Index is for fast lists and
current selection; detail records are source of truth.

Required fields:

```text
LocalAccount
  id
  label
  identity_email_or_name
  auth_mode: oauth | api_key | imported_token
  source_id
  secret_refs
  account_id
  organization_id
  plan_type
  subscription_active_until
  quota
  quota_error
  health
  token_generation
  token_updated_at
  requires_reauth
  reauth_reason
  tags
  notes
  enabled
  draining
  created_at
  last_used_at
```

An account may also carry an optional HTTP(S) proxy override inside its native
secret-store/encrypted-vault credential payload. The common account proxy is a
separate protected secret. Neither URL is part of account records, snapshots,
diagnostics, exports, logs, or portable imports. Re-import and token rotation
preserve an existing account override. SOCKS and silent direct fallback are not
supported. When `accountProxyRequired` is enabled, an account without either a
valid override or a valid common proxy stays persisted but cannot perform OAuth
exchange, refresh, model discovery, quota refresh, wake, or provider requests.

Useful index shape:

```text
AccountIndex
  schema_version
  summaries[]
  current_profile_accounts: profile_id -> account_id
```

Index repair contract:

- if index missing, empty, or corrupt, rebuild from account details;
- if detail files are missing, drop only orphan index rows;
- if all detail files are unreadable, keep data quarantined and return clear
  recovery error;
- preserve previous current account only if detail still exists;
- write index atomically and keep timestamped backup before repair.

Index rebuild details:

- collect account ids from detail JSON file names only;
- ignore non-JSON files and directories;
- load each detail through the normal account loader so schema migrations still
  apply;
- sort summaries by `last_used_at`, then `created_at`, then id;
- keep a timestamped `.bak` copy of the old index before replacing it;
- if repair succeeds with fewer accounts, surface a warning count.

## Storage Repair And Caches

Local stores must tolerate app crashes, manual edits, and interrupted writes.

Corrupt JSON handling:

- quarantine corrupt JSON with timestamp/reason metadata;
- continue with empty store only when empty state is safe;
- show recovery warning in diagnostics;
- never delete quarantined data silently;
- all saves should be atomic write + rename.

OAuth pending state:

```text
oauth_pending/<login_id>.json
```

Rules:

- pending file name must be non-empty and cannot contain `/` or `\`;
- corrupt pending files are quarantined and ignored;
- pending state can be loaded after app restart;
- save and clear are independent per login id;
- no raw token is written before OAuth completes.

Instance store:

- corrupt instance JSON is quarantined and replaced by empty in-memory store;
- instance name and user-data directory must be unique;
- copied profile directory may be copied only into empty or missing target dir;
- updates must preserve unrelated instance fields;
- deleting an instance stops its process/gateway first, then removes binding.

Quota cache:

```text
version
source
custom_source
email_hash
project_id
updated_at
payload
```

Rules:

- key cache by hashed normalized email, not raw email;
- ignore records with wrong cache version or source;
- apply cached quota only when cache is newer than current account quota;
- preserve existing model quota rows when cached payload has empty model list;
- preserve subscription tier and forbidden flag unless fresh payload updates
  them;
- API-key sources without quota adapter should not write fake quota records.

## Stable Credential Identity

Every account/source credential needs a stable non-secret identity for UI rows,
runtime reconciliation, and scheduler updates.

Identity inputs:

```text
source kind
base URL host/path
account id or email hash
secret fingerprint hash
profile/source namespace
```

Rules:

- never use raw API key, access token, refresh token, cookie, or full email in
  stable ids;
- keep `stable_index` visible only as technical row id, not as user copy;
- store success/failure counters and recent request buckets beside credential
  state;
- reset recent buckets by time window, not on app restart;
- update operations should preserve success/failure counters, recent request
  buckets, stable index, and clean model states when the credential remains the
  same;
- deleting an account/source removes its credential from local key scopes,
  current profile state, scheduler, model registry, and runtime cache.

## OAuth Login

OAuth flow should be resumable and independent from one frontend render.

Contract:

1. Start login creates `login_id`, PKCE verifier/challenge, state, callback
   port, redirect URI, auth URL, and expiry.
2. Persist pending state to disk.
3. Reuse active unexpired login session instead of starting duplicates.
4. Run local callback listener on `127.0.0.1:<port>/auth/callback`.
5. Validate `state`, store `code`, emit frontend event.
6. Support manual callback URL paste when local callback is blocked.
7. Complete login exchanges code for tokens and clears pending state.
8. Cancel clears pending state and stops listener.

Token exchange must require `access_token`. `id_token` should be required for
full identity extraction; if refresh response omits it, reuse previous
`id_token` only for same account.

State safety:

- state length capped;
- allowed state characters only: ASCII letters, digits, `-`, `_`, `.`;
- reject path separators and `..`;
- expire pending sessions;
- completing one provider login clears other pending sessions for that provider
  when they are stale;
- manual callback writes only into known pending session.

## Token Authority

All token refresh must go through one authority layer.

Rules:

- per-account async lock;
- per-account file lock so two app processes do not rotate same refresh token;
- token generation counter after each successful refresh;
- before refresh, check managed profile dirs for newer matching auth snapshot;
- accept external snapshot only when same identity and either snapshot is newer
  or local access token is expired while snapshot token is usable;
- retain old refresh token if refresh response omits new one;
- write refreshed token back to account store and managed profiles;
- classify refresh errors that require reauth: reused refresh token, expired
  refresh token, invalidated token, `invalid_grant`;
- access-token-only imports are degraded: usable until rejected, no automatic
  refresh.

This prevents duplicate refresh loops and reduces broken-login cases after user
signs in through another client.

Runtime request preparation must also use a per-account lock. The lock covers
last-moment token refresh, profile snapshot acceptance, and auth-state update
before execution. Concurrent requests can wait or pick another healthy
candidate, but must not rotate the same account independently.

## API-Key Sources

User API-key sources are personal local records, not Zenith production routing.

Validation:

- key must not be URL;
- base URL must be `http` or `https`;
- key and base URL must not be identical;
- source id derived from name/base URL must be stable and sanitized;
- protocol mode explicit: `responses`, `chat_completions`, later `messages`;
- model catalog may be discovered or manually entered;
- API-key account has no OAuth tokens and no quota unless source exposes a
  supported quota endpoint.

Some clients require both API-key config and account login state. Support this
as profile compatibility option:

```text
api_key_source_account -> optional bound_oauth_account_id
```

Only allow binding to OAuth account with refresh token. Refresh bound OAuth
account before writing profile config. Label this feature as client
compatibility, not routing.

## Profile Projection

Profile projection writes client config files from selected local account or
local gateway key.

For each supported client profile:

```text
inspect -> backup -> attach -> verify -> restore
```

Projection writes:

- `auth.json` with either API-key shape or OAuth token shape;
- provider config in client config TOML/JSON;
- optional model catalog file when client needs local model visibility;
- managed marker with profile id, account id/key id, created time, app version.

Safety rules:

- create backup only if current profile is not already managed by same local
  key/account;
- update existing backup for same profile instead of creating duplicates;
- restore only when current profile still contains managed marker;
- if marker missing, never overwrite fresh user login;
- if no backup exists, remove only managed blocks and leave unrelated config;
- preserve plugins and unrelated client settings;
- after attach/restore, inspect again and show exact state.

## Profile Backup Store

Profile attach must be reversible across app restarts. Store one backup record
per managed profile path, not one new file per attach click.

Backup record:

```text
ProfileBackup
  profile_dir
  previous_auth_json
  previous_config_toml
  managed_key_or_account_id
  created_at
  updated_at
```

Backup rules:

- normalize profile path before using it as backup key;
- do not save current local gateway state as the "previous" state;
- if same profile is attached again, update existing backup instead of adding a
  duplicate;
- backup both auth file and config file, including missing-file state;
- if backup file is corrupt, quarantine it and continue with empty backup list;
- write backup atomically.

Restore rules:

- restore only when current profile still matches the managed local key/account;
- if config backup exists, merge it with safe current sections that should not
  be lost, such as plugins;
- if auth/config backup is missing, remove only managed local gateway blocks;
- after disabling local gateway, show restore actions for every managed profile;
  do not restore or consume backups automatically;
- never overwrite a fresh manual login or user-edited API key when managed
  marker no longer matches.

Attach status should be inspectable:

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

This is the feature that lets the user switch back to previous login/API-key
state without hunting through backup files manually.

## Import Pipeline

Do not import secrets directly into active pool without preview.

Direct import supports one object or pasted content. Batch import uses session:

```text
start scan -> parse files -> build draft records -> optional quota probe
-> preview -> confirm selected -> save -> clear session
```

Batch session must support:

- `session_id`;
- persisted snapshot for app restart;
- cancel;
- resume;
- progress events;
- final preview event;
- selected-item confirm;
- failure rows that do not block valid rows.

Accepted personal import shapes:

- local `auth.json` with API key;
- local `auth.json` with token object;
- top-level `id_token` + `access_token` + optional `refresh_token`;
- nested `tokens` object;
- `refresh_token` only, exchanged before preview;
- `access_token` only, marked degraded/no refresh;
- API key plus optional base URL and protocol metadata;
- one JSON object per line;
- array of mixed account/source objects.

Preview row fields:

```text
item_id
source_file
label
account_id
identity
auth_mode
source_name
quota_status: skipped | success | failed
status: ready | existing | quota_failed | invalid
error
default_selected
selectable
existing
```

On confirm, save only selected items. If quota was probed successfully, store
quota snapshot with account.

## Account And Source CRUD Semantics

CRUD actions should preserve runtime consistency:

- upload/import accepts single item or batch;
- batch save/delete may return partial success with per-item errors;
- enable/disable changes status without deleting credential history;
- field patch supports explicit allowlist of editable metadata fields;
- status toggle is separate from arbitrary metadata patch;
- delete removes persisted credential, runtime credential, scheduler entries,
  model registry entries, and local key scope references;
- if deleting current profile account, clear current profile selection and show
  restore/attach action;
- file/path based imports must reject unsafe names and path traversal;
- raw secret download/export is disabled by default and requires explicit
  reveal/export action.

Field patches should update mirrored metadata and normalized attributes
together. Example: changing a source base URL should update source metadata,
runtime credential attributes, model discovery state, and request-auth scope in
one command result.

## Batch Account Delete

Bulk delete must be resumable and must clean runtime references as each account
is removed.

Job snapshot:

```text
job_id
status: running | paused | completed | failed
total
completed
failed
errors[]
account_ids[]
next_index
created_at
updated_at
```

Rules:

- job id accepts only ASCII letters, digits, `-`, and `_`;
- running job snapshot becomes `paused` when loaded after app restart;
- each successful account deletion removes account id from pool membership,
  local key scopes, routing rules, model rules, profile bindings, scheduler,
  and model registry;
- failed rows keep account id plus short error;
- retry failed builds a new job from failed ids only;
- clear job removes memory state and persisted snapshot;
- batch delete UI must show progress and keep failed row recovery visible.

## Quota And Subscription

Quota refresh should update account health but not surprise-switch active
profiles unless user explicitly enabled local auto-switch.

Flow:

1. Prepare account through token authority.
2. If API-key source has no quota adapter, store `quota = none` and show
   unsupported instead of error.
3. Query usage/quota endpoint.
4. Parse primary and secondary quota windows.
5. Parse reset times as timestamps or relative seconds.
6. Parse reset-credit count and details when available.
7. Refresh subscription state separately when usage response lacks plan/expiry.
8. Store `quota_error` with code, message, timestamp on failure.
9. Keep previous subscription when refresh fails but old value still exists.

Quota model:

```text
QuotaSnapshot
  primary_percentage
  primary_reset_at
  primary_window_minutes
  primary_present
  secondary_percentage
  secondary_reset_at
  secondary_window_minutes
  secondary_present
  reset_credits_available
  reset_credits[]
  raw_snapshot_ref
```

Refresh-all should use bounded concurrency. Start with 5 workers, then make it
configurable if needed.

Auto-switch should be recommendation-first in public app:

- show current account over threshold;
- show best candidate;
- switch only if user enabled automatic switching for that profile/pool.

## Current State And Switching

Current selection must be per profile/client, not one global mutable value.

Required behavior:

- get current account for profile;
- clear stale current id if account was deleted;
- switch writes prepared profile bundle;
- switch updates current profile state;
- switch updates account `last_used_at`;
- switch can update compatible client auth only when user enabled that target;
- delete account removes it from local pool membership, local key scopes, and
  current profile state.

Switch flow:

```text
load account -> repair index if needed -> prepare auth -> write profile
-> verify -> update current state -> update last_used -> emit UI refresh
```

For local gateway mode, switching should not mutate user client profiles on
each request. Runtime requests use scheduler state. Profile mutation happens
only through explicit attach/restore/switch actions.

## Multi-Instance Binding

Codex profiles and extra instances need explicit binding state.

Instance record should include:

```text
InstanceProfile
  id
  name
  user_data_dir
  working_dir
  extra_args
  bind_account_id
  follow_local_account
  launch_mode: app | cli
  app_speed
  auto_sync_threads
  last_pid
  initialized
```

Binding rules:

- default profile and named instances share same binding contract;
- binding an account/local key to a profile is allowed only after profile is
  initialized, unless it is the default profile that already has a known home;
- apply binding by writing profile projection, not by editing global current
  account only;
- unbinding removes managed local gateway/provider blocks and stops any local
  per-profile gateway process;
- start flow closes old process, stops stale profile gateway, writes selected
  binding, ensures gateway if needed, syncs idle threads, sanitizes config,
  then launches app or prepares CLI command;
- stop flow clears PID and stops profile gateway;
- close-all stops every known instance gateway and clears stored PIDs.

Track credential kind before and after binding:

```text
account | api
```

If kind changes, UI should recommend history visibility repair before launch or
after launch, depending on whether the client needs it.

## Session Visibility Repair

Switching between account login and API-key/local gateway modes can leave old
threads hidden in some clients. Repair should be a separate manual repair
action, not hidden inside every switch.

Repair inputs:

```text
mode: quick | deep
dry_run
target_provider
instance_ids[]
session_ids[]
```

Discovery:

- list instances from default profile plus named profiles;
- list candidate providers from config, session files, and SQLite metadata;
- show whether instance is running;
- default target provider comes from the profile config.

Safe repair flow:

```text
scan -> dry-run summary -> backup -> write -> rebuild metadata -> prune backups
```

Backups must include changed session files, SQLite DBs via consistent SQLite
backup, `session_index.jsonl`, and `manifest.json`. If write fails, restore
from backup automatically. Keep only a small number of latest repair backups
per profile.

UI should show counts:

```text
instances changed
session files changed
sqlite rows changed
sqlite timestamp rows changed
session_index rows added/updated
backup dirs
running instances affected
metadata rebuild failed
```

Dry-run must not create backups or write files.

## Client Auth Adapters

Some compatible clients keep their own auth store. Treat each client as an
adapter with the same contract:

```text
discover candidate paths -> read existing -> update one provider entry
-> atomic write preferred path -> sync legacy paths if they exist -> verify
```

Adapter rules:

- prefer official/current client path, but read older paths for migration;
- keep unrelated provider entries in the auth file;
- write atomically;
- verify by comparing account id/email/token expiry when the client stores those
  fields;
- detect expired local token before writing;
- if client keeps runtime auth cache, trigger reload/restart when available;
- if reload is unavailable, return a clear "restart client" action;
- do not expose raw tokens in UI, logs, or support bundles.

Adapters are optional targets. Main local gateway must continue working even
when a client adapter is disabled or unsupported on the current OS.

## Error Classification

Expose short local categories to UI:

```text
profile_not_found
profile_not_managed
account_not_found
account_requires_reauth
token_refresh_failed
quota_refresh_failed
quota_unsupported
import_invalid
import_quota_failed
source_test_failed
profile_write_failed
profile_restore_blocked
```

Raw HTTP/source/client bodies should stay in debug logs with secret redaction.
User-visible messages should say stage and action.

## Implementation Order

The only active build order and release gates live in
[local-pool-final-planning.md](local-pool-final-planning.md). This document owns
account, auth, import, quota, and profile contracts only.

## Test Checklist

- corrupt index repairs from detail files;
- orphan index row removed when detail missing;
- deleting current account clears current profile state;
- OAuth pending state survives app restart;
- manual callback validates state and login id;
- two parallel refreshes produce one token rotation;
- reused/invalid refresh token marks account `requires_reauth`;
- access-token-only import does not force refresh immediately;
- batch import can cancel, resume, preview, confirm selected rows;
- profile restore refuses when user logged in manually after attach;
- profile backup dedupes repeated attach for same profile;
- unbind/disable removes only managed local gateway config when no backup
  exists;
- instance binding refuses uninitialized named profile;
- start flow stops stale per-profile gateway before launching;
- session visibility dry-run creates no backup and writes no files;
- failed session repair restores from backup;
- client auth adapter preserves unrelated provider entries;
- API-key source rejects URL-as-key and invalid base URL;
- quota refresh stores error without deleting previous subscription.
