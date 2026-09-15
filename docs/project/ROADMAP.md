# Zenith Relay roadmap

Remaining acceptance and future work only. Current contracts are in
[PLANNING.md](PLANNING.md); release/check commands are in
[CONTRIBUTING.md](../../CONTRIBUTING.md). Recheck source before implementing a
backlog item. Live account operations remain deferred until explicitly resumed
with permitted accounts. Test the local path before the user-managed server.

## P0 — Installed client and live-provider acceptance

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
- Test real Responses/Messages/Gemini bindings per claimed provider: initial
  function/namespace/custom call, actual tool execution, result continuation,
  JSON/SSE, cache/reasoning usage, pre-output fallback, and fresh turn on restart.
- Verify source prices, manual fallback, metadata provenance, and unknown cache
  counters remain distinct through refresh. Catalog reachability does not prove
  inference, and missing prices cannot suppress account inventory.
- Verify OpenCode desktop/CLI reload, model/image/reasoning refresh, failed-write
  rollback, and JSON/JSONC restore on supported platforms.
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

Measure warm startup, page open, policy-save, local/remote pool switch, and
disk/SQLite/history/rollout bytes with representative data before optimizing.
Instrumentation alone is not a measured result. Prove policy-only hot updates
preserve the listener, active leases, affinity, and runtime state. Add a focused
regression check for a demonstrated bottleneck rather than speculative caches.

### Unified routing contract still to implement

Replace API-first/stabilizer/reserve roles with one versioned policy shared by
accounts and sources. Protocol-specific candidates remain internal details.

- Modes: default **Smart**, **In order**, **Round robin**. Store one atomic
  tagged member order; new members append, temporarily unhealthy members keep
  position, and reordering does not interrupt requests or move owned responses.
- Pipeline: mandatory response/connection ownership; eligibility by model,
  binding, lane, health, quota/capacity; guarded soft affinity; ranking; bounded
  pre-output fallback. Scope affinity by client/model/lane/protocol as required.
  Bind soft affinity only after verified success.
- Smart factors: manual preference, quota/capacity, bounded reliability/TTFT
  observations with decay/hysteresis, reset urgency, and optionally confirmed
  cost. Unknown evidence is neutral. Profiles: Cache default, Balanced, Speed,
  Economy, Custom. Purchase cost/payback never become scheduling inputs;
  current prices-do-not-route behavior stays until cost-aware work is complete.
- Distribute within bounded Top-K (proposed default three): stable session
  assignment, smooth weighted rotation for unscoped traffic, validated weights
  and safe rotation-credit updates. Avoid a single noisy sample monopolizing work.
- Failure feedback: request errors do not penalize members; model/credential
  failures affect the narrowest proven identity. Persist bounded cooldown/health
  with expiry, closed/open/half-open circuits, and bounded recovery probes.
- Keep hot policy application, redacted routing traces, revision IDs, factual
  activity, and model-scoped route preview. Preview is not a dispatch promise.
- UI: one draggable mixed-member list and distribution dialog; advanced
  coefficients/weights stay optional. Local/server use the same negotiated DTO.
- Migrate legacy roles/imports with preview, CAS, backup/restore, and rollback;
  failed conversion keeps the previous policy. Remove old roles and scheduler
  branches only after compatibility; never maintain two active schedulers.

Acceptance: deterministic local/server decisions from identical state, score/
weight/unknown-evidence tests, bounded fallback/partial-stream tests, affinity
and concurrency tests, old-import/interrupted-upgrade tests, and evidence that
policy changes preserve live state. Design/version types before implementation.

## P2 — Recovery and persistence acceptance

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
