# Zenith Relay roadmap

This file lists unfinished product work. It is not a release checklist and does
not claim that a feature exists. Implemented behavior is documented in
[PLANNING.md](PLANNING.md); contributor and release checks are in
[CONTRIBUTING.md](../../CONTRIBUTING.md).

Relay is a local-first, user-owned pool. The shipped subscription connector is
ChatGPT. Other providers can be used as generic API sources when their API
contract is known; a subscription connector needs its own authentication,
refresh, entitlement, quota, usage, and recovery work.

## Product direction

The target product is a reliable personal pool with one clear flow:

- connect permitted accounts and API sources once;
- keep the source's native protocol when it is compatible;
- use a typed adapter only for a conversion whose semantics are supported;
- select a healthy member by model, capacity, quota, and the chosen rotation
  mode;
- expose the same pool locally or through a user-operated Relay Server;
- keep credentials, provider state, usage, and recovery data in their owning
  stores.

Work follows this order: finish and accept the current local/server runtime
(P0), make publication and measured routing improvements predictable (P1),
then consider a separately designed hosted multi-user product (P2). A generic
protocol adapter or a model listed by a provider is not a promise of a
subscription connector.

## Boundaries

The following are outside the current product contract:

- hosted multi-user tenants and customer-scoped keys;
- wallet, payment, customer billing, and reconciliation;
- distributed multi-server scheduling;
- fingerprint spoofing, sharing concealment, or account resale;
- moving an opaque provider conversation to another account without saved
  portable history.

## P0 — finish and accept the current runtime

P0 proves the existing local and user-managed server paths with permitted
accounts and real installed clients.

### Client and provider acceptance

- Exercise ChatGPT and generic API sources through add, refresh, disable,
  remove, restore, model refresh, failed-write rollback, and configuration
  recovery.
- Verify Responses, Chat Completions, Anthropic Messages, and Gemini requests
  for every advertised source, including JSON, streaming, tools, reasoning,
  cache usage, safe pre-output retry, and a fresh turn after restart.
- Verify compressed requests, compaction, retained-context continuation, OAuth
  refresh, credential replacement, routing-cookie expiry, and client recovery.
- Keep the complete source inventory separate from endpoint support. A model
  returned by a source must not disappear only because a route is unavailable.
- Use two permitted accounts to check rotation, proxy, quota refresh,
  cooldown/recovery, member removal, and redacted usage.

### Concurrency and load

- Finish Responses WebSocket multiplexing: independent lanes, FIFO per lane,
  bounded queues, scoped errors, ownership, retry accounting, continuation
  forks, usage, and disconnect handling.
- Measure admission fairness and retained memory under mixed HTTP, SSE,
  WebSocket, and image traffic. Use measurements before adding caches or new
  limits.
- Complete the token-ownership, delayed-result, rollback, and concurrent-refresh
  matrix for desktop and server.

### Server acceptance

1. Pair over HTTPS with separate management and request credentials and a vault
   key.
2. Add or transfer only permitted user-owned connections and inspect redacted
   state.
3. Compare requests, quota, usage, and timing with the desktop open and closed.
4. Verify restart, upgrade, interrupted migration, backup, restore, and a
   request from the restored runtime.
5. Check management responses, diagnostics, usage, and exports for secrets,
   prompts, response bodies, and authorization headers.

## P1 — make server publication explicit

The desktop and server already have separate configuration, credential, and
account-transfer operations. Finish a single documented workflow that:

- builds a desired-state revision and shows a diff before applying it;
- validates sources, models, protocols, capabilities, policies, and references;
- reports the active revision, validation failures, health, and rollback handle;
- distinguishes portable settings from local state, secrets, leases, and
  server-owned changes;
- requires explicit confirmation for credential or account transfer;
- never performs implicit two-way synchronization that can overwrite newer
  changes.

The workflow must remain recoverable across desktop/server restarts, interrupted
migration, a failed runtime rebuild, and a lost management connection.

## P1 — measured routing and performance work

Only optimize after measuring the affected path:

- tool catalogs across native and converted routes, JSON, SSE, WebSocket,
  continuation, and policy changes;
- policy-only updates without losing listeners, leases, affinity, or runtime
  state;
- bounded cooldown and health persistence across restart;
- startup, page open, policy save, remote pool switch, history size, latency,
  token usage, cache reuse, and tool-selection quality.

## P2 — optional hosted mode

This is a separate product and starts only after P0/P1 and provider-permission
review:

- tenant isolation for models, limits, usage, keys, and operational views;
- scoped request keys with rotation and revocation;
- tenant quotas, rate limits, admission, and attribution;
- separate provider cost, account quota, operator price, and customer charge;
- an append-only wallet ledger with reservations, debits, refunds, payments,
  and reconciliation;
- a redacted live request stream and tenant-isolated audit view.

## Maintenance

Revisit these only when the affected platform or recovery path changes:

- upgrade the Tauri/GTK dependency chain as one tested platform change;
- verify application-first recovery, history repair, Windows extended paths,
  partial failure, and cleanup failure;
- test transactional bulk account changes and concurrent reasoning-policy edits
  through real client flows.

## Future connectors

Add subscription providers one at a time. Each connector needs permitted live
accounts and evidence for authentication, refresh, revocation, vault storage,
entitlements, quota units, usage, models, execution, and recovery. Do not mark
a provider as supported because a generic protocol adapter can format its
request.

## Demand-gated future work

- Add distributed server scheduling only after measured demand requires shared
  state, candidate leases, affinity, or cross-node coordination.
- Add named profiles, aliases, and groups with source-scoped identities,
  collision checks, independent ordering, enablement, and price fields.
  Presentation grouping must not change protocol or scheduler policy.
- Consider Zenith convergence only after Relay's own runtime and hosted-mode
  boundaries are proven and the integration is requested separately. Zenith
  remains the authority for customer and money concerns.
