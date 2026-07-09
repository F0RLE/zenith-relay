# Zenith Codex Product Direction

## Goal

Zenith Codex should become an open desktop app for:

1. buying and using Zenith API access;
2. managing a user's own local AI accounts and API keys;
3. combining those local accounts into a personal pool;
4. showing quota, subscription, reset, health, and usage state;
5. privately uploading Zenith-owned operator accounts into the server
   account-pool when operator mode is enabled.

The public app can be useful for normal users without exposing Zenith's internal
provider routing or owned server account pool.

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
duplicate pricing, routing, provider, or margin logic.

### Personal Local Pool Mode

Public open-app feature. User adds their own accounts/API keys locally and can
use them through a local gateway started by the app.

Allowed:

- add OpenAI/Codex OAuth accounts;
- import local `auth.json`;
- import pasted token JSON;
- import Sub2API-style JSON for personal accounts;
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
- local account details are never uploaded to Zenith unless operator mode is
  explicitly enabled and the account is Zenith-owned;
- public UI copy must not describe Zenith's internal provider routing.

### Personal Local Gateway

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
local configuration. It is not internal Zenith provider routing.

### Operator Server Upload Mode

Private admin mode for Zenith operations. This mode is hidden from normal users
and can be enabled only by an operator/admin build flag or signed admin login.

Purpose:

```text
operator computer
-> capture/login/import Zenith-owned account
-> preview identity/quota/subscription
-> upload selected credential bundle
-> zenith-account-pool server
-> encrypted secret ref
-> server-side quota refresh and execution
```

Rules:

- only Zenith-owned accounts are allowed;
- customer-owned accounts must not be uploaded into Zenith server pool;
- upload uses short-lived import sessions;
- server validates, encrypts, and deduplicates before account becomes routable;
- `access_token` only imports are admin-test only until refresh path is proven;
- unknown quota is never public-routable;
- server execution must not depend on the operator computer staying online.

## First Public UX

Use a compact operational layout, not a marketing page:

1. **Connect**: choose Zenith API, Local Pool, or Operator Upload if enabled.
2. **Accounts**: list accounts with email/label, provider, auth mode, health,
   quota windows, reset time, subscription, last used, and tags.
3. **Pool**: local routing strategy, priorities, disabled/draining state,
   model support, cooldowns, and local gateway status.
4. **Usage**: recent requests, latency, token usage when available, account
   chosen locally, and errors.
5. **Settings**: local gateway port, client config targets, import/export,
   language, update channel.

Recommended first routes:

- `Zenith API` opens the existing key/balance/top-up flow.
- `Local Pool` opens account import and local gateway controls.
- `Operator Upload` appears only for admins.

## Local Pool UI Direction

Cockpit shows useful behavior patterns, but Zenith should use a quieter
operator-style interface with less empty space, clearer grouping, and Russian /
English localization from day one.

Main navigation:

1. **Home**: current mode, active endpoint, local gateway state, total accounts,
   healthy accounts, warnings, and recent usage.
2. **Sources**: Zenith API preset plus user-added OpenAI-compatible providers.
   Each row shows name, base URL host, protocol, enabled state, model count,
   last test, and quick actions.
3. **Accounts**: personal OAuth/API accounts with email/label, provider,
   subscription, quota windows, reset time, health, local tags, and actions.
4. **Pool**: routing strategy, account/source priority, weight, disabled/drain
   state, model support, cooldowns, and generated local API keys.
5. **Gateway**: port, localhost/LAN scope, current base URL, generated key,
   attach/restore buttons for Codex/OpenCode, test request, and logs.
6. **Usage**: request history, model, source/account, API key label, latency,
   tokens when available, status, error category, and local estimated cost.
7. **Settings**: storage paths, language, theme, update channel, imports,
   exports, backups, and advanced timeout/retry options.

Visual rules for Zenith:

- default to table/list for dense operational data; use cards only for account
  summaries and repeated source/account items;
- show quota as compact bars with exact reset text beside them;
- keep destructive actions behind inline confirmation;
- put test/start/stop/refresh actions near the object they affect;
- keep internal Zenith provider routing invisible in public UI;
- show generic terms such as `source`, `account`, `local gateway`, and
  `local API key`;
- do not copy Cockpit images, Chinese text, gradients, or component code.

First desktop layout:

```text
sidebar: Home / Sources / Accounts / Pool / Gateway / Usage / Settings

top strip:
mode selector | active endpoint | gateway on/off | health summary

main area:
selected view list/table | right details drawer for edit/test/logs
```

For one selected object, use a full-width details view instead of a thin right
column. The right drawer is only for quick edit/test panels when a list remains
visible.

## Import Formats

Support these formats in local/personal mode:

1. OAuth browser login with manual callback fallback.
2. Local Codex `auth.json`.
3. Pasted JSON with `id_token`, `access_token`, optional `refresh_token`.
4. Nested `tokens` JSON.
5. `refresh_token` only, exchanged before use.
6. `access_token` only, marked degraded/no refresh.
7. Sub2API-style OpenAI OAuth export.
8. API key plus optional custom base URL.

Operator upload should accept only normalized Zenith import bundles or raw JSON
after preview. Raw secrets must never be logged.

## Local Scheduler

Personal local pool should use the same concepts as the server account-pool, but
only for local user traffic:

1. filter disabled, draining, login-required, captcha/checkpoint, expired,
   cooldown, quota-exhausted, unsupported-model accounts;
2. prefer healthy known quota;
3. use priority/weight only after hard filters;
4. spread traffic with last-used balancing;
5. use session affinity only after health and quota gates pass;
6. never retry a stream after output bytes were sent.

## Cockpit Ideas To Translate

Cockpit local access has a useful product shape for a personal local server.
Translate these ideas into Zenith naming and implementation:

1. `collection`: enabled state, port, access scope, client base URL host, gateway
   mode, account ids, local API keys, routing strategy, timeouts, debug logs,
   session affinity, max retry credentials, and model rules.
2. Local API keys: default key plus named keys, enabled flag, label, scoped
   accounts, model prefix, allowed models, excluded models, last-used timestamp,
   rotate/delete actions.
3. Routing strategies: `auto`, `single_account`, `quota_high_first`,
   `quota_low_first`, `plan_high_first`, `plan_low_first`, `expiry_soon_first`,
   and `custom`.
4. Custom routing: account priority and weight. Priority decides tier; weight
   spreads picks inside the same tier.
5. Account model rules: exclude models per account so one weak account does not
   remove a model for the whole pool.
6. Model aliases and model filters: useful for local compatibility, but public
   Zenith API prices/models still come from Zenith backend.
7. Profile attach/restore: write local provider config and auth JSON, then keep
   backup/restore so users can return to previous Codex login/API setup.
8. Usage stats: totals, accounts, models, API keys, daily/weekly/monthly windows,
   request logs, latency, tokens, errors, and estimated local cost.
9. Health state: account available flag, consecutive failures, last success,
   last failure, model cooldowns, image capability status.
10. Timeouts and retries: separate open/idle/total stream timeouts, websocket
    timeouts, upstream send retries, and local test request results.

Do not copy Cockpit identifiers into final UI unless they are generic protocol
terms. Use `Zenith Local Pool`, `Local Gateway`, `Local API key`, `Accounts`,
`Quota`, `Health`, `Usage`, and `Sources`.

Backend reference notes are tracked in
[`local-gateway-architecture.md`](./local-gateway-architecture.md). Build from
that split instead of recreating Cockpit's large all-in-one local-access module.
Live UI observations are tracked in
[`cockpit-live-ui-audit.md`](./cockpit-live-ui-audit.md).

First Zenith local server contract:

```text
GET  /v1/models
POST /v1/responses
POST /v1/chat/completions
```

Later:

```text
POST /v1/images/generations
POST /v1/images/edits
POST /v1/messages
```

The first implementation should prefer `/v1/responses` for Codex. Chat
completions can proxy custom API-key sources that only support chat completions.
Anthropic messages adapter comes later.

## Boundary With `zenith-account-pool`

`zenith-account-pool` remains an internal server backend for Zenith-owned
capacity. It is not the public personal pool backend.

Shared ideas:

- account model;
- quota windows;
- health states;
- model cooldowns;
- assignment logs;
- usage events;
- import preview;
- scheduler gates.

Different storage:

- personal local pool stores user-owned accounts locally;
- server account-pool stores only Zenith-owned accounts with encrypted secret
  references.

Different billing:

- Zenith API mode bills through gateway;
- personal local pool has no Zenith customer debit;
- operator server pool is internal cost only, never public billing.

## Open Source Rule

The public app can be open-source under the existing project license. Internal
operator upload endpoints, production server secrets, and Zenith routing
configuration must stay out of public UI defaults and docs.

Cockpit/Sub2API are references for user expectations and import shapes only.
Do not copy their code, UI text, assets, prices, or provider catalog into
Zenith.

## Implementation Order

1. Document product modes and boundaries.
2. Add local account model in the Tauri app without server upload.
3. Add import preview for local `auth.json`, token JSON, and Sub2API-style JSON.
4. Add quota/subscription refresh for local OpenAI/Codex accounts.
5. Add local gateway skeleton, disabled by default.
6. Add local scheduler with priority/weight and quota gates.
7. Add usage/health diagnostics.
8. Add private operator upload mode after `zenith-account-pool` import sessions
   exist.
9. Add Claude/Gemini only after OpenAI/Codex is stable and each family has
   proven auth, quota, executor, usage capture, and failure handling.

## Open Questions

1. Local gateway protocol order: `/v1/responses` first, then chat completions,
   then Anthropic messages adapter?
2. Should the public app include cloud sync of user-owned accounts? Default
   answer: no, local only, because secrets and policy risk are high.
3. Should users be able to export to Sub2API format? Default answer: import
   first, export later.
4. Which platforms get local gateway first: Windows only, or Windows/macOS/Linux
   together?
5. Should operator upload live in the same binary behind admin login, or in a
   separate internal build? Default answer: same codebase, hidden by signed
   admin capability.
