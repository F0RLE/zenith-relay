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

Runtime ordering is intentionally small:

1. mandatory `previous_response_id` binding to the account that created it;
2. valid optional session affinity when the candidate still passes hard filters;
3. explicit API-source role: primary first, stabilizer with OAuth accounts,
   reserve last;
4. lowest active-request load normalized by traffic share and available quota;
5. OAuth preference within an otherwise equal stabilizer comparison;
6. committed dispatch balance normalized by traffic share and available quota,
   then greatest known quota reserve and least recently used;
7. manual priority, weight, and stable id as final tie-breakers.

Source role is explicit and editable. Subscription plan names and expiry dates
must not create a hidden routing rule.

Each persisted usage attempt includes a redacted routing decision: selection
reason, eligible count, quota reserve, effective weight, and pre-dispatch load.
It never includes request/response bodies, credentials, proxy addresses, or raw
account identities.

Retry rules:

- never try the same candidate twice for one request;
- retry only classified retryable failures;
- never switch candidate after response bytes reached the client;
- local pool key is never forwarded as an upstream credential.

## Work Order

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
