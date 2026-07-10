# Local Pool Implementation Roadmap

This file contains unfinished implementation work for the public Zenith Relay
personal pool. Completed work is removed after verification.

Canonical specifications:

- [product-direction.md](product-direction.md): product modes and boundaries.
- [local-pool-runtime-contract.md](local-pool-runtime-contract.md): local and
  remote runtime protocol.
- [local-account-auth-architecture.md](local-account-auth-architecture.md):
  account import, OAuth, quota, profiles, backups, and repair.
- [local-gateway-architecture.md](local-gateway-architecture.md): storage,
  scheduler, execution, telemetry, and gateway internals.
- [app-ux-flow-spec.md](app-ux-flow-spec.md): screens and interactions.

## Product Target

The app has three user modes:

```text
Zenith API
Local Pool
Remote Pool
```

`Remote Pool` has two setup actions, not two product modes:

```text
Connect existing server
Deploy new server
```

Local and remote pools expose one endpoint and choose among the user's own
accounts and API-key sources.

```text
Codex / OpenCode / compatible client
-> one local or remote endpoint
-> one generated pool key
-> shared scheduler
-> OAuth account or API-key source
```

The server keeps working when the desktop app is closed.

## Boundary

Public Zenith Relay owns:

- user-owned accounts and API-key sources;
- local and user-managed remote pool configuration;
- local/remote quota, health, usage, and client configuration;
- the public personal-pool protocol.

It does not own or expose:

- Zenith provider economy or customer billing;
- Zenith backend provider routing;
- Zenith-owned selling inventory;
- private `zenith-account-pool` admin or execution policy.

Zenith's private selling pool remains a separate service. It may later reuse
proven neutral behavior, but public app code must not contain its credentials,
inventory, routing policy, or operator authority.

## Core Design

Use one normalized runtime candidate for every usable upstream:

```text
RuntimeCandidate
  id
  kind: oauth_account | api_source
  source_id
  account_id
  protocol: responses | chat_completions | messages
  enabled
  draining
  priority
  weight
  models
  health
  quota
  cooldowns
  last_used_at
  secret_ref
```

OAuth accounts and API-key sources must pass through the same scheduler. A
chat-only source is an executor adapter, not a separate routing mode.

Request flow:

```text
authenticate local pool key
-> normalize request and model
-> build candidates
-> apply hard filters
-> choose candidate
-> translate to candidate protocol
-> execute
-> translate response
-> update health, cooldown, quota hint, and usage
```

Hard filters always win:

- disabled or draining;
- missing/invalid secret;
- reauth, checkpoint, captcha, blocked, or expired;
- exhausted or stale required quota;
- active cooldown;
- unsupported model or protocol;
- outside local-key scope;
- failed health threshold.

MVP ordering is intentionally small:

1. valid session affinity when the candidate still passes hard filters;
2. highest explicit priority;
3. enough known quota;
4. least recently used inside the same priority;
5. weight only as a tie-break/spread control.

Creation defaults may prefer personal account capacity over paid API sources,
but the stored priority is explicit and editable. Source type must not create a
hidden routing rule.

Retry rules:

- never try the same candidate twice for one request;
- retry only classified retryable failures;
- never switch candidate after response bytes reached the client;
- local pool key is never forwarded as an upstream credential.

## Work Order

### P2: Unified Multi-Source Scheduler

Extend the same vertical to multiple candidates.

Implement:

- multiple API-key sources;
- normalized `RuntimeCandidate`;
- priority, weight, enable/disable, and drain;
- model allow/exclude rules;
- local-key candidate/model scopes;
- hard health and cooldown filters;
- least-recently-used spread;
- pre-stream fallback;
- `/v1/models` built from current candidate capabilities;
- request logs by model, source/account, local key, status, latency, and tokens.

Add `chat_completions` through an executor adapter only after the Responses path
passes. Both protocols remain candidates in the same scheduler.

Acceptance:

- one request can fall back from a failed candidate to another;
- disabled, cooled, scoped-out, or unsupported candidates are never called;
- candidate selection is deterministic in tests;
- a model remains usable while at least one eligible candidate supports it;
- stream fallback cannot duplicate output.

### P3: OpenAI/Codex OAuth Accounts And Quota

Add user-owned account capacity after the generic scheduler works.

Implement:

1. OAuth browser login with manual callback fallback.
2. Local Codex `auth.json` import.
3. Token JSON and refresh-token import with preview/confirm.
4. Stable identity and duplicate update rules.
5. One token refresh authority with per-account lock.
6. Quota/subscription refresh with normalized windows and reset times.
7. Reauth, access-token-only, expired, checkpoint, captcha, and blocked states.
8. OAuth account executor for `/v1/responses`.
9. Account health, model cooldown, affinity, and usage capture.
10. Quota wake tasks with account/window selection, cycle dedupe, minimal
    request policy, countdown verification, and execution history.

OAuth accounts and API sources enter the same `RuntimeCandidate` list. No
account-only scheduler is allowed.

Acceptance:

- two OAuth accounts and one API source can share one endpoint;
- default explicit priorities can place OAuth accounts before paid API sources;
- exhausted/reauth accounts are skipped before scoring;
- concurrent requests do not refresh one token twice;
- quota and reset state survive app restart;
- a fully restored quota window can start one verified countdown without a
  duplicate request when normal client traffic already started it;
- deleting an account clears scopes, affinity, cooldown, and profile bindings.

### P4: Remote Pool Runtime

Move the proven pool runtime to a user-managed server.

Do not reuse private `zenith-account-pool`.

Server MVP:

- one standalone server package;
- single binary first;
- Docker Compose after the binary works;
- SQLite;
- encrypted secret store;
- background quota/health refresh;
- public `/v1/models` and `/v1/responses`;
- management health, capabilities, state, sources, accounts, keys, and usage;
- one management token and one or more pool request keys;
- backup and restore instructions.

Desktop flow:

```text
Connect existing server
-> enter URL and token
-> test origin and capabilities
-> manage through RemoteRuntimeAdapter

Deploy new server
-> generate config/install instructions
-> user deploys
-> continue through the same connect flow
```

The desktop must not remain online after import. Confirmed secrets live on the
server and are not downloaded back to the app.

Acceptance:

- remote request works while Zenith Relay is closed;
- local and remote targets return the same public state shape;
- unsupported server actions are disabled from capabilities;
- token is pinned to the configured origin and never follows a cross-host
  redirect;
- disconnect removes the local connection token but does not stop the server.

### P5: Complete Operational UI

Build only screens required by the working runtime:

1. Overview with mode selector and runtime status.
2. Connections with Sources, Accounts, Automations, and Remote Server views.
3. Pool with Candidates, Keys, and Model Rules views.
4. Gateway with Endpoint, Client Setup, and Diagnostics views.
5. Usage with request table and detail drawer.
6. Profiles with attach, restore, backup, and repair.
7. Settings and Recovery.

Remote Pool setup offers:

- `Connect existing server`;
- `Deploy new server`.

UI rules:

- seven top-level sidebar items only: Overview, Connections, Pool, Gateway,
  Usage, Profiles, Settings;
- default window `1160x760`, minimum `840x560`;
- system theme by default with neutral light and charcoal dark palettes;
- one selected record uses a full-width detail view;
- tables/lists for operational data;
- quota and reset time are visible together;
- wake automation shows selected accounts/windows, next eligibility, last
  result, and history without storing generated text;
- every mutation has loading, success, failure, and retry state;
- destructive actions require confirmation;
- raw secrets are hidden by default;
- public UI never explains Zenith private routing or owned inventory.

Acceptance:

- every backend command has a reachable UI action;
- empty states have one primary action;
- `1160x760` and minimum `840x560` windows have no overlap or clipped controls;
- Russian and English strings exist for all shipped controls;
- Playwright screenshots cover the main local and remote flows.

### P6: Release Verification

Required unit/contract coverage:

- state validation and migrations;
- secret redaction and no plaintext export;
- import preview and duplicate handling;
- token refresh locking;
- quota transitions;
- scheduler hard filters, priority, LRU, weight, cooldown, and affinity;
- local-key scopes;
- protocol translation;
- stream retry boundary;
- profile attach, backup, restore, and repair;
- remote version, origin, redirect, and token safety.

Required end-to-end flows:

```text
API source -> local endpoint -> Codex request -> usage

OAuth account + API source
-> local scheduler
-> fallback
-> quota/health update

local state
-> upload/import to Remote Pool
-> close desktop
-> server request still works
-> reopen desktop
-> quota/health/usage refresh

attach Codex
-> use pool
-> rotate local key
-> restore previous profile
```

Release gates:

- Windows x64/ARM64, macOS Intel/Apple Silicon, and Linux x64/ARM64 builds pass;
- local endpoint, OAuth callback, secret storage, profile discovery, wake tasks,
  and restore flows pass on all three operating systems;
- AppImage, DEB, RPM, DMG, portable EXE, setup EXE, and MSI artifacts are
  produced by the release workflow;
- local and remote stores survive restart and interrupted writes;
- no raw secret appears in logs, frontend state, diagnostics, or support
  bundles;
- failed remote server cannot affect Zenith API mode;
- the final test matrix is run with real user-provided test credentials before
  release.

## Private Zenith Selling Pool

The commercial owned-capacity path is separate:

```text
Zenith operator capture client
-> protected import session
-> private zenith-account-pool
-> encrypted Zenith-owned account
-> zenith-gateway prefers owned pool when healthy
-> external provider fallback when unavailable
```

Rules:

- customers still call only Zenith Gateway;
- Gateway owns customer auth, price, debit, public catalog, and fallback;
- private account-pool owns only Zenith-owned account execution capacity;
- the public app never ships private credentials or operator authority;
- public local/remote pool completion does not automatically make private
  selling capacity production-ready.

This private track follows the roadmap in
`zenith-account-pool/docs/roadmap.md`.

## Deferred

Do not build before P1-P4 prove the need:

- Claude/Gemini OAuth account pooling;
- Anthropic `/v1/messages`;
- image generation;
- LAN gateway;
- cloud sync of secrets;
- raw secret export;
- hosted multi-user control plane;
- many routing strategies;
- background desktop service;
- automatic VPS provisioning;
- plugin/script execution.

## Completion Criteria

The public personal pool is complete when:

1. OAuth accounts and API-key sources share one scheduler and endpoint;
2. local and remote targets share the public contract;
3. secrets stay on the user's device or selected server;
4. quota, health, fallback, streaming, usage, and profile restore pass end to
   end;
5. Remote Pool works while the desktop is closed;
6. UI exposes every shipped action without internal Zenith details;
7. all release-platform and real-credential tests pass;
8. quota wake automation is deduplicated, bounded, and verified against real
   primary/secondary window transitions.
