# Zenith Relay roadmap

Remaining acceptance and future work only. Current contracts are in
[PLANNING.md](PLANNING.md); release/check commands are in
[CONTRIBUTING.md](../../CONTRIBUTING.md). Recheck source before implementing a
backlog item. Live account operations remain deferred until explicitly resumed
with permitted accounts. Test the local path before the user-managed server.

## Pool rotation — remaining acceptance gates

The shared admission/lease engine and request budgets are connected. This is
not full acceptance of [ROTATION_DESIGN.md](ROTATION_DESIGN.md). Complete
these gates before calling the replacement ready. Keep the small old-format
reader so existing saved data can upgrade without a separate prompt:

- Finish installed-client acceptance of the automatic 1.1.3 startup upgrade.
  Local/server conversion is idempotent and preserves enabled state and user
  settings without a separate confirmation. An older executable/database
  downgrade is not a supported migration operation.
- Finish shared refresh host integration. Desktop/server account quota/models
  use the common asynchronous service, join manual/background callers and
  apply observations behind durable revisions. API-source models/stats also
  share that owner; their old periodic workers are removed. The desktop queue,
  separate single-flight lifecycle and account timers are removed; rollback
  reconciles current registrations and future reset events accelerate quota
  work. Persisted passive inference quota can defer automatic account polls;
  both hosts have synthetic reset-due tests. Account quota/models and initial
  reset-credit checks now join an on-demand, non-cached Auth prerequisite with
  reserved capacity and the existing authority. Source/account snapshots expose
  independent refresh evidence; saved observations are stale after restart.
  Per-HTTP management sends, including Auth recovery, now share a process-local
  gate with reserved Auth capacity and revision fences on account/source reads.
  Finish installed-client quota/reset and refresh-reason acceptance, adapter
  capability coverage, and sustained large-pool worker/HTTP traffic acceptance.
  Source-statistics cache is runtime-only; restart loses the last value. Core
  no-progress/unsupported state alone is not host acceptance.
- Measure admission fairness and retained-memory bounds under sustained mixed
  HTTP/SSE/WS/image load, including recovery-to-capacity handoff. Count/byte
  limits, event-driven waits and accumulated wait budgets are connected; local
  synthetic tests are not large-pool performance acceptance.
- Complete the final-dispatch matrix for token/login, proxy/endpoint, owner,
  policy and principal revisions, including delayed results after reconfigure
  or restart. Server quota/model 401 and token/Agent-task persistence now fence
  replaced credentials; import/delete also serialize runtime builds and fence
  the old account before durable credential changes. Superseded
  server runtimes and restarted desktop runtimes now reject new reservations and
  final dispatches while allowing started attempts to settle. Existing
  scope/quota/rate/circuit/runtime fences are not the entire permission/evidence
  matrix. Prepared OAuth slot and Agent-task revisions now gate HTTP and
  WebSocket payload dispatch within its budget debit; reused WebSocket
  connections also require the same in-memory credential incarnation. Final
  dispatch now also checks a bound response owner's revision,
  principal scope and candidate permission revisions, Auth execution fences,
  capability blocks and protected quota reserve without charging a rejected
  send. Account model-inventory reads update the live runtime without dropping
  leases or health. Internal desktop/server key scopes and pool policy now
  update in one routing transaction. Server single-account policy edits fence
  dispatch across durable save and hot apply/rebuild; a failed restore retires
  stale runtime permissions. Server single-source updates/deletion fence every
  protocol route; catalog refresh also holds the runtime-build lock and fences
  changed source routes before applying its observation. Gateway stop retires
  the live runtime before persisting the stopped state, and background rebuilds
  cannot reopen it. Server profile-key commit/abort retire the old runtime
  before changing vault keys; rotation operations serialize with runtime builds
  and fail closed if restoration fails. Server batch membership fences changed
  members across its durable commit and scope update. Server proxy edits fence
  affected accounts across saved transport changes and replacement. Desktop membership,
  account/source policy and proxy/endpoint edits now fence changed physical
  candidates across save and hot apply/replacement; preset writes fence the
  previous pool, including during local request-key rotation. Source generation
  probes on both hosts reject late results from a deleted-and-readded source
  even when its visible configuration is identical. Desktop re-import
  and OAuth completion fence replaced account logins. Remote ownership transfer
  now fences local candidates through import and verified cleanup; startup
  excludes pending moves and remote-owned records. Reconciliation updates the
  live scope and failure paths keep remote-owned local routes closed. Desktop
  account deletion closes the gateway before releasing a fence if its rollback
  fails. The core rejects a removed token slot after delayed refresh or
  asynchronous persistence; the desktop refresh and persistence adapters now
  fence their final writes and account deletion waits for the same credential
  lock across delete and re-add. Finish remaining token, ownership and
  delayed-result coverage,
  including failed rollback and concurrent refresh cases. Ordinary server
  quota reads also apply without replacing the runtime.
- Verify real installed clients and permitted providers across local/server
  HTTP, SSE, WebSocket, images and compaction; measure fairness, recovery,
  management traffic and bounded memory. Local synthetic tests do not prove
  live-provider behavior or performance.

## P0 — Installed client and live-provider acceptance

- Verify current clients' compressed requests, account compaction through both
  legacy and Responses-trigger paths, retained-context continuation, and
  turn-state ownership across OAuth refresh and AgentAssertion task replacement.
  Include sparse terminal compaction events, ChatGPT routing-cookie expiry,
  and credential replacement during a live WebSocket conversation.

- Exercise account streaming recovery: safe pre-output failure with complete
  history can retry; opaque response ownership, unpaired tool outputs, and
  already-forwarded output cannot silently move to a different owner.
- Verify full account/source inventory independent of endpoint-support flags,
  then verify executable client projections and explicit user filters. Newly
  discovered IDs must survive refresh without a hardcoded allowlist.
- Run Codex attach/refresh/disable/remove/restore, including IDs containing `/`,
  native metadata preservation, image input, and a failed catalog refresh.
  Previous verified config must remain usable; running-client deferrals and
  background failures must be visible.
- Test real Responses/Chat Completions/Messages/Gemini bindings per claimed provider: initial
  function/namespace/custom call, actual tool execution, result continuation,
  JSON/SSE, cache/reasoning usage, pre-output fallback, and fresh turn on restart.
- Implement and accept Responses WebSocket multiplexing instead of the current
  single in-flight response and single named lane: independent concurrent lanes,
  FIFO within a lane, bounded queuing and named-lane limits, continuation forks,
  scoped events/errors and per-lane ownership, retry, usage and disconnect
  accounting. Exercise both native upstream WebSocket and HTTP/SSE fallback,
  including malformed/opaque events and credential changes; compare the
  implementation against the current official WebSocket mode contract.
- Verify source prices, manual fallback, metadata provenance, and unknown cache
  counters remain distinct through refresh. Catalog reachability does not prove
  inference, and missing prices cannot suppress account inventory.
- Verify key-balance adapters against permitted live Sub2API, New API, One API,
  OpenRouter and DeepSeek sources, including dashboard restrictions, quota
  conversion and subscription allowance. Track a future official SiliconFlow
  account API before re-enabling its retired balance probe. Mocked format tests do not prove a
  particular reseller has enabled the endpoint for its inference keys.
- Verify OpenCode desktop/CLI reload, model/image/reasoning refresh, failed-write
  rollback, and JSON/JSONC restore on supported platforms. Exercise all four SDK
  groups, preserved model IDs, and Codex HTTP/SSE selection for converted routes.
- Verify upgrades from legacy source records and older servers with installed
  clients, catalog refresh and stale-result handling during key/address changes,
  and preset rollback. Normal setup and refresh must remain generation-free.
- Exercise two healthy permitted personal accounts, rotation, proxy, quota
  refresh, cooldown/recovery, removed-member admission, and redacted usage.

### User-managed server, after local acceptance

1. Use HTTPS with distinct management and request credentials and vault key.
2. Add/transfer only permitted user-owned connections and verify redacted state.
3. Stream requests with the desktop open and closed; compare quota, usage, and
   timings after reconnect.
4. Prove restart, upgrade/interrupted migration, backup to a clean location,
   restore, and a successful request from the restored runtime.
5. Inspect management responses, diagnostics, usage, and ordinary exports for
   secret/prompt/body leakage. Never use Zenith production inventory.

## P1 — Measured performance and adaptive routing

- Tool-catalog optimization: verify installed clients and permitted providers
  across native/converted JSON, SSE, WebSocket, continuation and policy changes.
  Measure actual token usage, latency, prompt-cache reuse and tool-selection
  quality; reduced catalog JSON bytes alone do not prove net savings.
- Measure provider-native deferred tool search across permitted native
  Responses providers, installed clients, SSE, continuation and policy changes.
  Compare actual input tokens, latency, prompt-cache reuse and tool-selection
  quality; reduced catalog JSON bytes alone do not prove net savings. A local
  semantic search round-trip remains separate experimental work and must not
  become the default without quality regression evidence.

Measure warm startup, page open, policy-save, local/remote pool switch, and
disk/SQLite/history/rollout bytes with representative data before optimizing.
Instrumentation alone is not a measured result. Prove policy-only hot updates
preserve the listener, active leases, affinity, and runtime state. Add a focused
regression check for a demonstrated bottleneck rather than speculative caches.

### Rotation state persistence and measured refinements

Current runtime behavior is in PLANNING; the unfinished rotation gates are above.

- Persist bounded mandatory cooldown/health state with expiry and verify
  restart recovery without restoring stale authority or unknown remote work.
- Verify local/server policy changes during streaming, busy-limit waits,
  automatic startup conversion and concurrent membership edits.
- Keep the small old-format policy reader for upgrades/imports. Obsolete V1
  scalar settings are discarded at startup/import, excluded from new snapshots
  and presets, and ignored on old-client requests. Performance, quota and price
  observations must not become undocumented automatic ranking inputs.

## P2 — Recovery and persistence acceptance

- Update the desktop dependency chain when a compatible stable Tauri/GTK stack
  removes the remaining RustSec warnings. The current GTK path requires
  `glib` 0.18; RUSTSEC-2024-0429 is fixed in 0.20 and is not a drop-in lockfile
  update. The same chain retains `proc-macro-error`, while `tauri-utils`
  retains retired `unic-*` packages through `urlpattern`. Verify the complete
  platform upgrade and Linux desktop behavior; keep advisories visible.
- Validate the application-first recovery layout on upgraded installations,
  both history-repair directions, Windows extended paths, partial failure, and
  cleanup failure without losing the rollback handle.
- Exercise transactional bulk account changes and concurrent reasoning-policy
  edits through actual client flows; preserve canonical mutation ownership.
- Maintain error-origin, cooldown, continuation, and redaction contracts during
  future changes. Do not reopen completed ownership refactors for cosmetic moves.

## Demand-gated future work

- **Subscription connectors:** only on explicit demand with permitted live
  accounts. Prove auth/refresh/revocation, vault storage, native entitlement/
  reset/usage units, models, execution, and recovery. Keep entitlement, observed
  usage, API-equivalent, and actual API spend separate; never infer a monetary
  entitlement from a quota percentage or copy another provider's counters.
- **Server scale:** require demonstrated need before distributed state,
  candidate leases, shared affinity, and cross-node storm coordination.
- **Named profiles:** extend existing preset preview/CAS/rebuild/rollback.
  Keep immutable secret-free revisions, local active state, and each server's
  publication independent. Validate target references/capabilities before apply;
  secret transfer remains separate. No implicit bidirectional synchronization.
- **Model aliases and groups:** explicit source/binding-scoped identities,
  collision/cycle checks, and independent display order, enablement, and price
  fields. Presentation grouping never implies protocol or scheduler policy.
  Existing metadata sorting is not a complete alias/profile contract.
- **Zenith convergence:** deferred until P0–P2 and platform correctness are
  proven and separately requested. Follow the workspace
  [convergence gates](../../../ROADMAP.md#optional-relay-runtime-convergence);
  Control retains customer/money authority and rollback remains available.

For a release, apply the existing localized Help, screenshot, changelog,
packaging, and live-acceptance requirements in `CONTRIBUTING.md`; do not keep
test counts, review diaries, or completed checklists in this roadmap.
