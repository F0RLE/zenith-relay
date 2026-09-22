# Zenith Relay roadmap

Remaining acceptance and future work only. Current contracts are in
[PLANNING.md](PLANNING.md); release/check commands are in
[CONTRIBUTING.md](../../CONTRIBUTING.md). Recheck source before implementing a
backlog item. Live account operations remain deferred until explicitly resumed
with permitted accounts. Test the local path before the user-managed server.

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
- Verify source prices, manual fallback, metadata provenance, and unknown cache
  counters remain distinct through refresh. Catalog reachability does not prove
  inference, and missing prices cannot suppress account inventory.
- Verify key-balance adapters against permitted live Sub2API, New API, One API,
  OpenRouter, DeepSeek and SiliconFlow sources, including dashboard restrictions, quota
  conversion and subscription allowance. Mocked format tests do not prove a
  particular reseller has enabled the endpoint for its inference keys.
- Verify OpenCode desktop/CLI reload, model/image/reasoning refresh, failed-write
  rollback, and JSON/JSONC restore on supported platforms. Exercise all four SDK
  groups, preserved model IDs, and Codex HTTP/SSE selection for converted routes.
- Verify upgrades from manual source records and older servers with installed
  clients, explicit probes during key/address changes, and preset rollback.
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

### Smart routing refinements and live acceptance

The shared three-mode policy is implemented; current behavior is documented in
PLANNING. Live-provider and installed-client acceptance above remains required.

- Add bounded, model/lane-scoped reliability and TTFT observations with decay,
  minimum sample counts, and hysteresis. Unknown observations must stay neutral.
- Evaluate reset urgency and optional confirmed cost profiles only with evidence.
  Purchase cost/payback must never become scheduling inputs.
- Persist bounded cooldown/health state with expiry and verify restart recovery.
- Verify real local/server policy changes during streaming, busy-limit waits,
  preset migration and rollback, and concurrent membership edits.
- Retire legacy import fields only after a documented compatibility window.

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
