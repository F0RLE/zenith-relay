# Zenith Codex Local Gateway Architecture

## Source Reference

Cockpit reference inspected from `cockpit-tools` commit `2ad714ea`:

- account grid and instance list screenshots;
- `src/types/codexLocalAccess.ts`;
- `src/services/codexLocalAccessService.ts`;
- `src-tauri/src/modules/codex_local_access.rs`;
- `src-tauri/src/commands/codex.rs`;
- `sidecars/cockpit-cliproxy/cdk/CLIProxyAPI/internal/api/server.go`;
- sidecar auth, scheduler, translator, request logging, and management
  handlers.

Use these as product and behavior reference only. Do not copy Cockpit code,
UI copy, images, or assets.

## What To Borrow

Cockpit proves these pieces are useful for a local Codex-compatible pool:

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
- routing strategies: auto, single account, quota high/low first, plan high/low
  first, expiry soon first, and custom priority/weight;
- stream-safe retry rule: retry only before response bytes are sent.

## What To Avoid

Cockpit also shows what not to repeat:

- one huge backend module for storage, gateway, scheduler, profile edits,
  request translation, logs, and tests;
- too many advanced controls visible in the first screen;
- UI copy that explains internal mechanics instead of user-owned state;
- provider-specific logic mixed with generic pool logic;
- public docs that imply user accounts are uploaded to a central service.

## Zenith Module Split

Implement local pool as small Rust/Tauri modules:

```text
local_pool/
  models.rs          data contracts shared with frontend
  store.rs           encrypted local config, account/source records, backups
  imports.rs         auth.json/token/provider import preview + normalize
  providers.rs       provider source test, model discovery, protocol metadata
  scheduler.rs       filtering, priority/weight, affinity, cooldowns
  gateway.rs         localhost HTTP server lifecycle and route dispatch
  translators.rs     responses/chat/messages/image adapters
  profile.rs         Codex/OpenCode attach, repair, restore
  telemetry.rs       SQLite request logs and usage rollups
  commands.rs        Tauri command surface
```

Frontend split:

```text
src/features/local-pool/
  api.ts
  types.ts
  routes/
  components/
  hooks/
```

Keep `Zenith API Mode` separate from `Personal Local Pool Mode`. Zenith API is
one preset source in local mode, not the local pool's hard dependency.

## Local Data Model

Core records:

```text
LocalProviderSource
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

LocalGatewaySettings
  enabled
  bind_scope: localhost | lan
  port
  client_host: localhost | 127.0.0.1
  routing_strategy
  session_affinity
  session_affinity_ttl_ms
  max_retry_candidates
  timeouts
```

Secrets should be encrypted locally where platform support exists. Raw secrets
must not appear in logs, screenshots, or exported support bundles.

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

Hard filters before priority:

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

Priority and weight apply only after hard filters.

## MVP Build Order

1. Local source/account schema plus encrypted store.
2. Provider source CRUD: add, edit, test, enable, delete.
3. Local account import preview for `auth.json`, pasted JSON, and API key.
4. Gateway settings and generated local API key.
5. `/v1/models` and `/v1/responses` through a single selected healthy source.
6. Scheduler with disabled/draining, priority, weight, quota, and cooldown gates.
7. Profile attach/restore for Codex/OpenCode.
8. SQLite logs and Usage screen.
9. `/v1/chat/completions` adapter.
10. Images and `/v1/messages` after OpenAI/Codex path is stable.

## Admin Boundary

Public local pool is for the user's own accounts and API keys. It stays local by
default.

Private operator upload is a separate hidden capability. It can upload only
Zenith-owned accounts into `zenith-account-pool` after server import sessions,
encryption, validation, dedupe, quota refresh, and admin audit logs exist.

No public UI should say or imply that normal user accounts are sent to Zenith's
server pool.
