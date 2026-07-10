# Zenith Codex Product Direction

## Goal

Zenith Codex should become an open desktop app for:

1. buying and using Zenith API access;
2. managing a user's own local AI accounts and API keys;
3. combining those local accounts into a personal pool;
4. running that personal pool either on the user's computer or on the user's
   own server;
5. showing quota, subscription, reset, health, and usage state.

The public app can be useful for normal users without exposing Zenith internal
backend operations.

## Dual Runtime Target

The personal pool has two public runtime targets:

```text
Desktop Local Gateway
Remote Pool Server
```

They share the same public objects:

```text
sources
accounts
local API keys
gateway settings
routing policy
model visibility
quota/health/usage
profile attach/restore
```

Target differences:

- Desktop Local Gateway runs inside Zenith Codex on the user's computer. It is
  best for personal use, quick setup, Codex/OpenCode attach, and localhost/LAN
  access. It works only while the app/computer is running.
- Remote Pool Server is a user-managed service reached through the same public
  personal-pool protocol. The user either connects an existing compatible
  server or deploys the Zenith personal-pool server, then manages both through
  the same UI. It stores that user's encrypted secrets on that server and keeps
  serving while the desktop app is closed.
- Zenith API mode remains separate: normal paid Zenith API requests go to
  `https://api.zenithmarket.dev/v1`.
- Zenith private owned-account infrastructure is not this public server. Public
  self-host code must not include Zenith internal routing, billing, inventory,
  provider economy, or admin policy.

Recommended package split:

```text
zenith-codex desktop app
-> manage local runtime
-> manage user self-host runtime over public protocol
-> optionally deploy/update Remote Pool Server

public personal-pool server package
-> public self-host server for user-owned accounts/sources
-> same protocol as desktop local gateway management
-> no Zenith private backend authority
```

The server package/repository decision is part of P4 in
[local-pool-final-planning.md](local-pool-final-planning.md).

## Product Modes

### Zenith API Mode

Existing mode. User saves a Zenith API key, the app configures Codex/OpenAI
compatible clients to use:

```text
https://api.zenithmarket.dev/v1
```

Owned by Zenith backend:

- customer API key auth;
- balance;
- public model catalog;
- customer debit;
- public usage history;
- top-up intents.

The desktop app renders API responses and creates top-up links. It must not
duplicate Zenith backend business logic.

### Local Pool Mode

Public open-app feature. User adds their own accounts/API keys locally and can
use them through a local gateway started by the app.

Allowed:

- add OpenAI/Codex OAuth accounts;
- import local `auth.json`;
- import pasted token JSON;
- import compatible OAuth/account export JSON for personal accounts;
- add provider sources with a name, OpenAI-compatible base URL, API key, and
  protocol mode;
- use Zenith API as one preset provider source, not as a hardcoded pool
  dependency;
- view quota windows, reset times, subscription status, account health, and
  account notes;
- set local priorities/weights;
- disable/drain accounts locally;
- start/stop a local gateway;
- generate local API keys for local clients;
- configure Codex/OpenCode/other compatible clients to use the local gateway.

Rules:

- user-owned accounts stay on the user's device by default;
- the personal pool is only for that user's own traffic;
- local pool usage must not affect Zenith backend billing;
- local account details are never uploaded to Zenith by public app flows;
- public UI copy must not describe Zenith internal backend operations.

### Local Gateway

The local pool should be exposed to clients through a local OpenAI-compatible
server, not only through direct account switching.

Target user flow:

```text
user accounts + provider sources
-> Zenith Codex local pool
-> local gateway at http://127.0.0.1:<port>/v1
-> generated local API key
-> Codex/OpenCode/client config
```

This lets a user combine several personal accounts and external API keys behind
one local endpoint. A provider source can be Zenith API, OpenRouter, direct
OpenAI-compatible vendor, a self-hosted gateway, or any compatible endpoint the
user configures. Codex can then be configured as if it were using one API key:

```toml
model_provider = "zenith_local_pool"

[model_providers.zenith_local_pool]
name = "Zenith Local Pool"
base_url = "http://127.0.0.1:14998/v1"
wire_api = "responses"
requires_openai_auth = true
experimental_bearer_token = "zlp_local_generated_key"
supports_websockets = false
```

The app should also write/repair the matching local `auth.json` API-key shape
when needed:

```json
{
  "auth_mode": "apikey",
  "OPENAI_API_KEY": "zlp_local_generated_key"
}
```

The local gateway needs:

- port setting;
- localhost by default;
- optional LAN scope with explicit warning;
- generated default key plus named keys;
- key rotation;
- per-key allowed models and excluded models;
- optional model prefix per key;
- per-key account/source scope;
- local request logs and usage stats;
- one-click client config attach/restore.

Management should be command-first inside the desktop app. Optional local HTTP
management is advanced, localhost-only by default, protected by a separate
management key, and never part of public customer API.

### Remote Pool Mode

Public open-app feature. User runs the same personal pool on a server they
control. This is one mode with two setup paths:

```text
Connect existing server
Deploy new server
```

Allowed:

- connect any compatible server by URL and access token;
- deploy the Zenith personal-pool server, then connect through the same flow;
- read capabilities and protocol version;
- import or upload the user's own accounts/sources through preview/confirm;
- start/stop or restart gateway when the server supports it;
- view server-side quota, health, usage, request logs, and local API keys;
- rotate server local API keys;
- export a redacted support bundle;
- detach the app without stopping the server.

Rules:

- the server is user-managed unless explicitly branded as Zenith API;
- server secrets stay on that user-managed server;
- the app can help deploy/update the server, but it must show server owner,
  host, version, and health clearly;
- no public self-host endpoint receives Zenith customer billing authority;
- no public self-host endpoint exposes Zenith private routing or provider
  economy logic.

### Provider Sources

Local pool sources are editable user-owned records. Zenith API is only one
default preset.

Provider source fields:

```text
id
name
enabled
base_url
api_key
wire_api: responses | chat_completions
models: discovered or manual
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
created_at
updated_at
```

User actions:

- add provider;
- edit name, base URL, key, protocol mode, models, priority, weight;
- test provider with selected model;
- enable/disable;
- delete;
- rotate local stored key value;
- scope a generated local API key to selected provider sources/accounts.

Zenith provider preset:

```text
name: Zenith API
base_url: https://api.zenithmarket.dev/v1
wire_api: responses
```

The user still provides their own Zenith API key. The app stores it as personal
local configuration. It is not Zenith backend execution config.

## Public Experience

The app uses a compact operational layout with Russian and English localization.
Users choose a mode, connect an account/source/server, and then manage only the
objects available in that mode.

Public UI uses neutral terms such as `source`, `account`, `local gateway`,
`local API key`, `quota`, `health`, and `usage`. It never describes Zenith
internal capacity, provider economy, or backend execution policy.

Canonical details:

- screens, navigation, design, states, and buttons:
  [app-ux-flow-spec.md](app-ux-flow-spec.md);
- account login, import formats, quota, profiles, and repair:
  [local-account-auth-architecture.md](local-account-auth-architecture.md);
- storage, scheduler, execution, telemetry, and Tauri module split:
  [local-gateway-architecture.md](local-gateway-architecture.md);
- local/server protocol and failure contracts:
  [local-pool-runtime-contract.md](local-pool-runtime-contract.md);
- unfinished implementation order:
  [local-pool-final-planning.md](local-pool-final-planning.md).

## Boundary With Zenith Backend

Zenith backend capacity, routing, cost, and inventory stay outside this public
desktop app. Personal local pool is not a backend provider and must not mirror
server execution policy.

The public app may use generic local concepts:

- account model;
- quota windows;
- health states;
- model cooldowns;
- assignment logs;
- usage events;
- import preview;
- scheduler gates.

Storage:

- personal local pool stores user-owned accounts locally;
- Zenith backend inventory is not represented in public app storage.

Billing:

- Zenith API mode bills through gateway;
- personal local pool has no Zenith customer debit.

## Open Source Rule

The public app can be open-source under the existing project license. Internal
admin endpoints, production server secrets, and Zenith backend configuration
must stay out of public UI defaults and docs.

Compatible import shapes are allowed only as user-owned local import formats.
Zenith must use its own implementation, UI text, assets, prices, and provider
records.

## Implementation

The unfinished work order is
[local-pool-final-planning.md](local-pool-final-planning.md). Exact backend
modules remain in
[local-gateway-architecture.md](local-gateway-architecture.md), account/auth
behavior in
[local-account-auth-architecture.md](local-account-auth-architecture.md), UI in
[app-ux-flow-spec.md](app-ux-flow-spec.md), and runtime/self-host contracts in
[local-pool-runtime-contract.md](local-pool-runtime-contract.md).

Do not add a second implementation checklist to this product document.

## Product Decisions And Deferred Research

Decisions for first implementation:

1. Local gateway protocol order:
   - ship `/v1/responses` first;
   - add `/v1/chat/completions` adapter after Responses path works;
   - add Anthropic `/v1/messages` only after OpenAI/Codex local path, streaming,
     usage capture, and translator tests are stable.
2. User-owned cloud sync:
   - no cloud sync in MVP;
   - user-owned accounts, provider keys, OAuth tokens, and local API keys stay
     local by default;
   - any future sync needs a separate encryption, consent, recovery, and threat
     model pass.
3. Compatible account bundle export:
   - import first;
   - redacted config export allowed;
   - raw secret export is not part of MVP and must require explicit reveal,
     encryption, and warning if added later.
4. Platform order:
   - keep Windows/macOS/Linux as target product shape because release workflow
     already builds all three;
   - first live gateway validation may happen on Windows, then macOS/Linux must
     pass before public release claims cross-platform support.
Deferred research:

- exact Claude/Gemini local account support after OpenAI/Codex is stable;
- optional sidecar runtime only if Rust/Tauri gateway isolation is not enough;
- optional compatible bundle export after import, backup, redaction, and local
  store migration are proven.
