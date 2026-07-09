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
- add API keys and custom OpenAI-compatible base URLs;
- view quota windows, reset times, subscription status, account health, and
  account notes;
- set local priorities/weights;
- disable/drain accounts locally;
- start/stop a local gateway;
- configure Codex/OpenCode/other compatible clients to use the local gateway.

Rules:

- user-owned accounts stay on the user's device by default;
- the personal pool is only for that user's own traffic;
- local pool usage must not affect Zenith backend billing;
- local account details are never uploaded to Zenith unless operator mode is
  explicitly enabled and the account is Zenith-owned;
- public UI copy must not describe Zenith's internal provider routing.

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
