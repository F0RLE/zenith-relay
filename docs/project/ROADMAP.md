# Zenith Relay roadmap

Open work, not shipped features or a progress log. Current contracts are in
[PLANNING.md](PLANNING.md); development/release checks are in
[CONTRIBUTING.md](../../CONTRIBUTING.md).

## Hosted multi-user Relay

The planned hosted service has one Relay identity across website and Telegram,
workspace-scoped keys/balance/access, direct client inference, and Desktop as
the operator client. The current server is single-owner. The separate
[Hosted proposal](HOSTED_RELAY_ARCHITECTURE.md) owns this design and its safety gates.

### Delivery stages

1. **Core acceptance:** qualify the existing native/converted routes, shared
   request budget, terminal outcomes, ownership, and attempt accounting needed
   by Hosted. Do not rebuild the existing four codecs or route-binding model.
2. **Hosted foundation:** decide permissions, retention, currency/tariffs, and
   launch region; implement PostgreSQL identity, workspaces, audit, vault
   references, scoped idempotency, and continuation ownership/checkpoints.
   The first administrator panel uses [passkey-only sign-in](HOSTED_RELAY_ARCHITECTURE.md#passkey-only-administrator-access),
   with explicit owner provisioning, revocable sessions, and server-local recovery.
   Google/OIDC, password, email/SMS, and Telegram sign-in are not part of this stage.
3. **Pool/catalog:** revisioned management, route validation, grants, scoped
   model/context projection, and explicit OAuth `stage_and_drain` transfer.
4. **Accounting:** journal, guarded reservations, settlement, reconciliation,
   payment idempotency, refunds, and uncertainty. Prove real-PostgreSQL races
   and crash recovery before paid traffic.
5. **Inference:** tenant/key admission through `relay-core`, SDK-shaped errors,
   usage attribution, and committed continuation recovery. Enable only
   commercially permitted routes with supported billable usage and finite bounds.
6. **Channels/transfer:** website and Telegram use the same account API;
   revisioned local/hosted transfer does not synchronize secrets implicitly.
7. **Operations:** Docker/TLS, monitoring, restore drills, migrations, load and
   real-client/provider acceptance.

Excel/BPS deep acceptance and additional subscription connectors are not a
prerequisite for a Hosted release that does not offer those credential classes.
No shared paid route is inferred from a working personal account.

## Local runtime P0 — finish and accept the current runtime

### Model, provider, and harness compatibility

The shared typed pipeline, four native paths, multiple source/model bindings,
and native-upstream selection are already implementation contracts. Remaining
work is qualification and specific missing semantics, not another adapter engine.

| Work | Acceptance |
| --- | --- |
| Catalog provenance and refresh | Source failures preserve valid data; changed identity/price/capability is traceable; generated exports update atomically without private inventory or manual model rows. |
| Endpoint feature evidence | Advertised protocol does not grant unsupported continuation, tools, reasoning, cache, media, or usage semantics. Pins and explicit exclusions survive reload. |
| Conversation compatibility | Each advertised source/client/version passes text → call → result → next answer in JSON and SSE, including its supported reasoning/media/schema features. |
| Client startup context | Establish a lossless foreign representation for standalone Responses results without `call_id` and interleaved system/developer instructions, including Messages-to-Gemini history, before claiming full client parity; do not invent calls, erase context, or hoist instructions. |
| Native fidelity | Ordered schemas, precise numbers, repeated permitted headers, opaque state, and native extensions survive without lossy conversion. |
| Accounting and errors | One logical outcome plus retained attempts; partial/unknown usage remains explicit; the failing stage and origin are safe and understandable. |
| Client profiles | ChatGPT, Claude Desktop, and OpenCode expose only usable models/efforts/context for that route; attachment and recovery preserve user settings. |

Use the [official contracts](../../.agents/skills/relay-protocol-adapters/references/contracts.md)
and [conversation matrix](../../.agents/skills/relay-protocol-adapters/references/acceptance.md).
Synthetic coverage, a working endpoint, and live client acceptance are different
evidence. Discovery must not generate paid probes across a catalog.

### Client and provider acceptance

| Area | Cases still requiring permitted live qualification |
| --- | --- |
| Provider presets | Exact endpoint/region/key scope, Moonshot USD/CNY statistics, Kimi Code discovery/tools; quota readers only after their units/reset contract is verified. |
| Lifecycle | Add, refresh, disable, trash/restore, removal, credential replacement, failed save/rollback, and restart on Desktop and server. |
| Outcomes | Completed, incomplete, refusal, malformed terminal, interruption, late usage, cancellation, and safe pre-output retries. Check state, not only displayed text. |
| Tools/state | Parallel calls, fragmented arguments, namespace/custom identity, Gemini optional IDs, signed reasoning, full-history and ID-based continuations, missing state after restart. |
| Native extensions | Gemini continuation-token resumes with unchanged contents and owner; provider cache references/hosted tools; client-executed Responses discovery and PDF inputs/results on supported pairs. |
| Authentication | OAuth renewal, same-token reauthentication, delayed 401/invalid grant, routing-cookie expiry; old turn/WS state must not cross authorization incarnations. |
| Transport | Compressed input, compaction, HTTP SSE versus reusable WS, socket loss, and unchanged owner/budget rules. Measure first output and usage without claiming quality from transport choice. |
| Limits | `turn_rate_limit_exceeded` keeps its delay without account-wide rotation; provider quota/access/model blocks keep their own scope. |
| Inventory | Endpoint failure does not erase source inventory or grant a different route. |

### Excel / Basis Points acceptance

- PKCE callback, cancellation, proxy, contextual Help, separate native/BPS
  connections, import/export/transfer, and concurrent login replacement.
- Compact versus full versus local-discovery catalogs with matched real client
  tasks: cold-start input, total lookup+answer usage, tool-selection accuracy,
  latency, context floor added by the native application, and session caching.
  Synthetic byte reduction is not a token/cost or quality guarantee.
- Actual entitlements, effort settings, full tool cycles, inline/file-ID tool
  images, collaboration attribution, and no privilege increase from history.
- Early text, terminal-only tool release, malformed/missing completion,
  heartbeat timeout, cancellation, and first/last output timing.
- Explicit local structured-output validation, refusals, tool turns, invalid
  schemas/output, and no automatic regeneration of a completed answer.
- Next-request 401 recovery, model-scoped access blocks, shared aliases,
  two-account rotation, quota refresh, removal, and operator disable/drain.
- Native Free versus Free BPS behavior after import, downgrade, restart, and
  transfer; timezone rewriting leaves ordinary text/API-source bodies intact.
- Quality comparisons only with authorized isolated routes and matched tasks,
  models, and effort. Keep inconclusive observations separate. Local solvers
  do not establish live account quality or eliminate degradation.

### Concurrency and load

- Finish native Responses WS multiplexing: independent lanes/FIFO, bounded
  queues, scoped errors, continuation forks, retry/usage and disconnect ownership.
- Measure mixed HTTP/SSE/WS/image admission fairness, queue age, lock waits,
  retained memory, cancellation, and lease release before adding caches/limits.
- Complete delayed-result, token-ownership, concurrent-refresh, rollback, and
  deleted/re-added account cases on Desktop and server.

### Server acceptance

1. HTTPS pairing with separate management/request credentials and vault key.
2. Explicit permitted connection transfer and redacted management state.
3. Request/quota/usage/timing parity with Desktop open and closed.
4. Restart, upgrade, interrupted migration, backup/restore, and a request from
   the restored runtime.
5. No secrets or prompt/response bodies in diagnostics/support exports;
   credential-bearing account exports use `no-store`.

## Local server P1 — make server publication explicit

Finish one desired-state workflow: preview a revisioned diff, validate routes
and references, apply/rollback atomically, report the active revision, and
recover after a lost management connection. Portable settings, credentials,
local leases, and server-owned changes remain separate. Credential transfer
needs explicit confirmation; no implicit two-way synchronization.

## P1 — measured routing and performance work

- Measure startup, page reads, saves, pool switches, history sizes, first-output
  latency, refresh work, and lock waits. Optimize the measured path, not a
  guessed protocol or assumed cache-hit benefit.
- Reduce the many mutable attempt flags passed through
  `gateway/execution/request/drive.rs` into cohesive typed execution state.
  Preserve one request identity, budget, lease, and recorded outcome across
  HTTP, account-only, and WS paths; add parity cases before extracting helpers.
- Group unrelated `GatewayRuntime` model/session/provider state into internal
  owners with explicit lock boundaries. Do not duplicate continuation stores
  or replace locks with a new actor/cache layer without measurement.
- Exercise policy-only updates without losing listeners, active streams,
  leases, affinity, or authorization provenance. Measure restart persistence
  needs for hard deadlines separately from disposable health hints.

## Maintenance

- Upgrade Tauri/GTK as a tested platform change, not incidental cleanup.
- Keep profile/history recovery, Windows paths, partial cleanup, transactional
  bulk mutations, and concurrent policy edits covered by their owning checks.
- Keep current contracts, future work, user Help, and release history in their
  existing documents. Do not append investigation transcripts or completed
  implementation steps to this roadmap.

## Future connectors

Add subscription providers individually, with permission and evidence for
authentication, renewal/revocation, protected storage, entitlements, quota units,
usage, models, execution, and recovery. A generic API codec is not a subscription
connector. Kimi Code subscription and MiniMax M Plan readers remain unqualified
until their exact cloud contracts are verified.

## Demand-gated future work

- Distributed scheduling only after measured demand and shared admission,
  member leases, durable ownership, and idempotent delivery exist.
- Additional aliases/groups/profiles need scoped identities, collision checks,
  independent ordering, enablement, and price fields; presentation cannot alter
  protocol or scheduler policy.
- Any Zenith integration needs its own request/design. Hosted Relay owns its
  identities, tariffs, wallets, payments, and route policy; production Zenith
  credentials, customer inventory, and internal business logic stay separate.
