# Zenith Relay roadmap

This roadmap describes the current source tree and the work that is still
needed. It is not a release checklist. The current contracts live in
[PLANNING.md](PLANNING.md); development and release checks live in
[CONTRIBUTING.md](../../CONTRIBUTING.md). A feature marked as implemented still
needs the acceptance work listed below before it becomes a release promise.

Live-provider checks require user-owned or otherwise permitted accounts. Do not
use production Zenith inventory, customer keys, or provider accounts that we do
not control.

## Status snapshot — 2026-10-06

| Area | Status | What is true in the current tree |
| --- | --- | --- |
| Local pool and rotation | **Partial / acceptance open** | Selection, startup conversion, leases, dispatch fences, quota/usage plumbing, and recovery rules are present. Installed-client and live-provider acceptance is still open. |
| Relay Server runtime | **Implemented** | The server has an HTTP(S) management surface, encrypted vault, SQLite persistence, gateway, scheduler, quota and usage work, and keeps serving after the desktop closes. Remote deployments should use HTTPS. |
| Desktop to server management | **Implemented** | The desktop can pair with a server, deploy a Docker/Compose bundle, and manage sources, accounts, imports, proxies, keys, quota, pricing, routing, models, usage, gateway settings, tool policy, and automations. |
| Configuration presets | **Implemented, with a boundary** | Preview/apply, revision or CAS checks, runtime rebuild, and rollback exist. A preset transfers configuration and metadata; secrets and account credentials use a separate explicit transfer/import path. |
| Account ownership transfer | **Implemented / acceptance open** | Accounts can be moved to the server and returned with state checks and rollback paths. Restart, upgrade, failure, and redaction acceptance remain. |
| Public client protocols | **Implemented in the gateway** | Responses, Chat Completions, Anthropic Messages, Gemini, `/v1/models`, streaming, tools, quota, and usage paths are present. Each provider/client combination still needs live acceptance before it is advertised. |
| Native routes and adapters | **Framework implemented / provider coverage partial** | Relay has protocol bindings, capability discovery, native routes, and typed conversions. This does not mean that every provider already has a subscription connector or that every conversion is semantically complete. |
| Usage and price reference | **Implemented, not billing** | Redacted usage history, provider observations, official/LiteLLM/manual price resolution, source overrides, model overrides, and API-equivalent estimates exist. They do not debit a customer wallet or define a selling price by themselves. |
| Responses WebSocket multiplexing | **Partial** | The current bridge supports one in-flight response and one named lane per connection. Independent concurrent lanes are not implemented. |
| Multi-user hosted mode | **Not implemented** | There is no tenant boundary, customer account model, customer-scoped key lifecycle, or per-tenant isolation. |
| Customer keys and billing | **Not implemented** | The server's profile/system gateway key is an internal server credential, not a customer-scoped key. There is no complete customer key, wallet ledger, reservation/debit/refund, payment, or reconciliation system. |
| Live request operations | **Partial** | Completed redacted usage is available. A live SSE/WebSocket stream for active requests and a hosted audit view are not implemented. |

## Product direction

Relay is a local-first, user-owned pool for permitted AI accounts and API
sources. A user connects a source once, then uses it from Codex, OpenCode,
Claude Code, or another compatible client. The pool can select a connection by
capability, load, quota, affinity, cooldown, and safe retry rules.

The inbound client protocol and the provider-facing protocol are separate. Relay
uses a provider-native route when the semantics are available and a typed
adapter only for a supported conversion. Provider-owned or opaque continuation
state stays with its original physical account. An adapter is a compatibility
mechanism; it does not create provider capability that the source does not have.

The current subscription connector is ChatGPT. Other providers can be added as
generic API sources today when their API contract is known. xAI, Anthropic,
Gemini, Z.ai, and similar subscription connections need their own authentication,
refresh, revocation, entitlement, quota, usage, recovery, and permitted
live-provider acceptance. They are not automatically covered by the generic
protocol layer.

The pool is for user-owned or otherwise permitted connections. It must not add
fingerprint spoofing, concealment of sharing, account resale, or unsafe movement
of an active provider-owned conversation between accounts.

## What is already built

The current foundation is sufficient to keep developing the product vertically:

- **Local pool:** sources, accounts, model inventory, capability checks,
  scheduling, leases, cooldown/recovery, protocol routing, adapters, quota, and
  redacted usage are connected across `relay-core` and the desktop.
- **User-managed server:** the server owns the encrypted vault and runtime;
  the desktop is the management console. Management credentials and request
  credentials remain separate.
- **Remote management:** server routes cover the current source/account,
  import, proxy, key, quota, price, routing, model, usage, gateway, tool-policy,
  and automation operations.
- **Configuration publication:** presets support preview, apply, revision/CAS
  protection, runtime rebuild, and rollback. The server reports validation and
  active-state failures.
- **Credential and account transfer:** credential-bearing changes are explicit
  operations stored in the server vault. Account ownership transfer has checked
  move-back and rollback paths.
- **Protocol surface:** the public gateway exposes Responses, Chat Completions,
  Anthropic Messages, Gemini, models, streaming, tools, quota, and usage paths.
- **Reference pricing:** price evidence and API-equivalent estimates can inform
  routing and display. They are not a customer charge or a balance ledger.

## P0 — finish the current runtime

P0 is about proving the existing contracts on real installed clients and a
permitted server. No new hosted business model should be built before these
checks are complete.

### Client and provider acceptance

- Run Codex and OpenCode through attach, refresh, disable, remove, restore,
  model/image/reasoning refresh, failed-write rollback, and JSON/JSONC recovery.
  Keep verified client configuration usable when a refresh fails.
- Exercise Responses, Chat Completions, Messages, and Gemini for every claimed
  provider path with JSON and streaming, tools and result continuation,
  reasoning/cache usage, pre-output fallback, and a fresh turn after restart.
- Verify compressed requests, account compaction, retained-context continuation,
  OAuth refresh, AgentAssertion task replacement, routing-cookie expiry, and
  credential replacement during a live WebSocket conversation.
- Verify the full source/account inventory independently from endpoint-support
  flags. Newly discovered IDs must survive refresh without a hardcoded allowlist.
- Use two healthy permitted personal accounts to verify rotation, proxy, quota
  refresh, cooldown/recovery, removed-member admission, and redacted usage.

### WebSocket and load acceptance

- Implement and accept Responses WebSocket multiplexing: independent concurrent
  lanes, FIFO within a lane, bounded queuing, scoped events/errors, per-lane
  ownership and retry accounting, continuation forks, usage accounting, and
  disconnect handling. Cover both native upstream WebSocket and HTTP/SSE
  fallback, including malformed events and credential changes.
- Measure admission fairness and retained memory under sustained mixed HTTP, SSE,
  WebSocket, and image load. Include recovery-to-capacity handoff; connected
  limits and synthetic unit tests are not this measurement.
- Finish the remaining token, ownership, delayed-result, failed-rollback, and
  concurrent-refresh matrix. The dispatch fences in `PLANNING.md` are the
  contract, not evidence that every host path has been accepted.

### Server acceptance

After the local path passes:

1. Pair over HTTPS with distinct management and request credentials and a vault
   key.
2. Add or transfer only permitted user-owned connections and inspect redacted
   state.
3. Stream requests with the desktop open and closed; compare quota, usage, and
   timing after reconnect.
4. Prove restart, upgrade or interrupted migration, backup to a clean location,
   restore, and a successful request from the restored runtime.
5. Inspect management responses, diagnostics, usage, and ordinary exports for
   secret, prompt, response-body, and authorization-header leakage.

## P1 — make server publication one clear workflow

The single-operator server foundation exists, but “everything in the local pool
is on the server” is not one atomic operation yet. Configuration presets,
credential transfer, and account ownership transfer are separate paths.

Complete a publication workflow that:

- builds a desired-state revision and shows a diff before apply;
- validates source, model, protocol, capability, policy, and target references
  before changing the active runtime;
- reports the active revision, validation failures, health, and rollback handle;
- distinguishes portable configuration and metadata from local-only state,
  secrets, account credentials, runtime leases, and server-owned changes;
- requires explicit confirmation for credential/account transfer and stores
  transferred secrets only in the user's encrypted server vault;
- never performs implicit bidirectional synchronization that can overwrite a
  newer local or server change.

Keep the rollout idempotent and recoverable across desktop restart, server
restart, interrupted migration, failed runtime rebuild, and lost management
connection. If a one-click “publish all” action is not worth the complexity,
keep the separate operations visible and document their order instead of
claiming full synchronization.

## P1 — measured performance and routing refinements

Only optimize after representative measurements of warm startup, page open,
policy save, local/remote pool switch, SQLite/history/rollout size, latency,
token usage, cache reuse, and tool-selection quality.

- Measure tool-catalog changes across native and converted routes, JSON, SSE,
  WebSocket, continuation, and policy changes. Smaller catalog JSON alone is not
  proof of a useful improvement.
- Measure provider-native deferred tool search on permitted native Responses
  providers. A local semantic-search round trip remains experimental until it
  wins on quality, tokens, latency, and cache reuse.
- Prove policy-only hot updates preserve the listener, active leases, affinity,
  and runtime state. Add focused checks for demonstrated bottlenecks instead of
  speculative caches.
- Persist only bounded cooldown/health state with expiry and verify restart
  recovery without restoring stale authority or unknown remote work. Keep the
  old-format policy reader for upgrades/imports; obsolete V1 scalar settings do
  not return to new snapshots or presets.

## P2 — optional hosted multi-user mode

This is a separate product expansion above the user-managed server. It starts
only after P0 and P1 are stable and after provider terms permit the use case.

- Add tenants and strict isolation for models, limits, usage, keys, revocation,
  and operational views.
- Issue, rotate, scope, disable, and revoke customer request keys. Do not reuse
  the server's profile/system key as a customer credential.
- Enforce tenant quotas, rate limits, admission policy, and safe request
  attribution at the gateway.
- Record provider cost, account quota, operator price, and customer charge in
  separate records. Support USD price schedules by provider, model, operation,
  or service tier without letting a displayed price create route capability.
- Add an append-only wallet ledger with top-ups, reservations, request debits,
  refunds, payment connectors, and reconciliation. The existing desktop
  Zenith/Telegram top-up flow is not Relay billing.
- Add a redacted live request stream and audit view with tenant isolation and
  bounded retention. Prompt and response bodies remain hidden by default.

## Maintenance and recovery

Keep these items below the product work and only reopen them when the relevant
platform or recovery path changes:

- Move the Tauri/GTK dependency chain when a compatible stable upgrade removes
  the known RustSec warnings; verify the complete platform upgrade and Linux
  behavior rather than applying a partial lockfile update.
- Validate application-first recovery on upgraded installations, both history
  repair directions, Windows extended paths, partial failure, and cleanup
  failure without losing the rollback handle.
- Exercise transactional bulk account changes and concurrent reasoning-policy
  edits through actual client flows. Preserve canonical mutation ownership,
  error origin, cooldown, continuation, and redaction contracts.

## Demand-gated future work

- **Subscription connectors:** add one provider at a time, only with permitted
  live accounts. Prove authentication, refresh, revocation, vault storage,
  entitlements, reset/usage units, models, execution, and recovery.
- **Server scale:** require measured need before distributed state, candidate
  leases, shared affinity, or cross-node storm coordination.
- **Named profiles, aliases, and groups:** extend preset preview/CAS/rebuild/
  rollback with explicit source/binding-scoped identities, collision checks,
  independent ordering, enablement, and price fields. Presentation grouping
  must not imply protocol or scheduler policy.
- **Zenith convergence:** defer until Relay's own P0/P1/P2 and platform
  correctness are proven and separately requested. Control keeps customer and
  money authority; Relay remains a separate product.

## What was removed or reworded

- Completed internal refactors are no longer listed as open roadmap work.
- API-equivalent price estimates are explicitly separated from customer billing.
- The profile/system gateway key is not described as multi-user access.
- The current one-lane Responses WebSocket bridge is not described as full
  multiplexing.
- Presets are not described as silently copying secrets or as a complete
  bidirectional sync of every local record.
- “Live operations” means a future active request stream; completed redacted
  usage is the capability that exists today.

For a release, follow the localized Help, screenshot, changelog, packaging, and
live-acceptance requirements in `CONTRIBUTING.md`. Do not keep test counts,
review diaries, or completed checklists in this roadmap.
