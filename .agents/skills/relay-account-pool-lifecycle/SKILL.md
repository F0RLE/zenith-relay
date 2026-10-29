---
name: relay-account-pool-lifecycle
description: Review or change Relay pool membership, renewal, quota, dispatch ownership, or account removal. Excludes API-source conversion, login-only work, and production Zenith pools.
---

# Relay account pool lifecycle

Make an account connector usable by the existing pool without creating a
parallel scheduler, token cache, or credential authority.

Read the affected contracts in
[PLANNING.md](../../../docs/project/PLANNING.md#pool-and-execution).
For a new login/import method use the
[account connector skill](../relay-account-connectors/SKILL.md).
For additional implementations read the relevant
[refresh and ownership references](../relay-account-connectors/references/implementations.md#refresh-and-pool-ownership).

## Trace the account through its owners

- Admission: `src-tauri/src/local_pool/commands/pool/membership.rs`,
  `accounts/records.rs`, and `crates/relay-core/src/runtime/build/accounts.rs`.
- Renewal: `crates/relay-core/src/accounts/token_authority/`
  and desktop `accounts/authority/`.
- Availability and observations: `accounts/quota_refresh/`, shared
  `quota/`, `scheduler/`, and `runtime/selection/`. Keep the current ownership
  and resource-refresh revisions rather than adding another health registry.
- Removal/replacement: desktop `accounts/mutations/delete/` and
  `accounts/import_orchestrator/`.
  Follow callers to the durable commit and runtime fences.

## Preserve the lifecycle distinctions

- Saved identity, pool membership, valid authorization, model access, quota,
  cooldown, and operator disable/draining are separate states. Sign-in does
  not opt an account into the pool. A new connector needs its own supported
  auth, renewal, entitlement, quota, usage, recovery, and live acceptance;
  generic API compatibility does not supply them.
- Derive eligibility from the owning connector's current policy. Native
  ChatGPT and Excel/BPS are distinct connections and execution paths; inspect
  `providers/chatgpt/oauth.rs` rather than imposing one client's restrictions
  on the other. Unknown plan/quota is neither denied access nor positive
  capacity evidence. A public model catalog cannot grant account access.
- Coordinate renewal through the existing authority. Request preparation,
  monitoring, and any background renewal share that path. Bind refresh to the
  issuing client; reread credentials under the existing process/store locks
  and after a provider reply. A changed login, removed/re-added registration,
  or newer generation fences both success and failure from older work.
- Persist rotated credentials and generation before returning durable success.
  A persistence retry must not refresh the already rotated grant again.
  Transient or malformed replies preserve saved credentials; terminal grant
  errors require reauthentication only for the still-current login. Token-text
  equality alone is insufficient. Preserve operator disable, draining, and
  membership during recovery.
- Keep auth failures, throttling, unavailable models, monitoring failures,
  conversion errors, and unknown/completed execution distinct. Honor
  provider cooldown/reset evidence at its actual account/model scope. Do not
  infer a ban or response quality from a 429, transport error, or model name.
- Bind continuation and request results to the originating account, route,
  and relevant revision. Portable history does not prove replay is safe.
  Retry only under the existing execution-certainty contract; token recovery
  must not resend a possibly executed generation or adopt another account's
  opaque response state.
- Aggregate credits by provider ledger identity, not connection count or
  email. Keep recovery and runtime ownership explicit. Trash/delete, membership
  changes, and reauthentication fence pending refresh, quota, and dispatch
  work; late results must not recreate a registration or credential.

## Establish acceptance

Use [acceptance.md](references/acceptance.md) for the affected transitions.
Inspect existing coverage; run or extend checks only within task and harness
permissions. Use synthetic accounts and provider responses.

Use [relay-protocol-adapters](../relay-protocol-adapters/SKILL.md) only when the
account route changes a conversation conversion. Update existing contracts and
Help for implemented changes, and report reference revisions, deliberate
differences, performed checks, and any remaining live acceptance.
