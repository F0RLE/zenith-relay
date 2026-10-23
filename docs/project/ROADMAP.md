# Zenith Relay roadmap

Open Desktop work and acceptance, not shipped features or a progress log.
Current contracts are in [PLANNING.md](PLANNING.md); development and release
checks are in [CONTRIBUTING.md](../../CONTRIBUTING.md).
Future Hosted work has its own [delivery plan](../hosted/HOSTED_RELAY_ROADMAP.md).

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

BPS multi-tool batching has a bounded [Relay-local contract](BASIS_POINTS.md) and
synthetic roundtrip coverage for JSON/SSE, discovery, tool restrictions,
disabled parallel policy, typed results, and opaque reasoning. Remaining live
qualification must verify model adoption of the envelope, client acceptance of
reversible call IDs and result grouping, and the complete tool-results/next-turn
cycle on each intended harness. Measure cache reuse, TTFT and end-to-end latency
separately. Local protocol support is not evidence of concurrent client execution
or a measured speedup, and a first response with several calls is insufficient.

Request-stage diagnostics distinguish selected-attempt preparation, upstream
headers/collection, normalized first stream output, and response/tool readiness.
Qualify their coverage on the intended HTTP/SSE/discovery paths after installation;
client receipt and between-request harness delays require client-side evidence.
Cold-preparation qualification should compare request building, its initial
schema-validation subset, catalog preparation, and attachment work. A synthetic
validator benchmark does not identify a live cold-start bottleneck by itself.
Use context-size and existing cache-prefix diagnostics before removing any work;
token reuse alone does not establish the cause of TTFT or safe history reduction.

| Area | Cases still requiring permitted live qualification |
| --- | --- |
| Provider presets | Exact endpoint/region/key scope, Moonshot USD/CNY statistics, Kimi Code discovery/tools; quota readers only after their units/reset contract is verified. |
| Lifecycle | Add, refresh, disable, trash/restore, removal, credential replacement, failed save/rollback, and restart on Desktop. |
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
  Include explicit retries after a completed lookup and failed answer or exhausted
  dispatch budget: verify history-scoped preloading in JSON/SSE, one usage event
  per physical attempt at budget exhaustion, and total sends/input, not only
  cache hits.
- Repeated and overlapping catalog lookups: verify earlier-result references,
  unchanged schema validation, and total input across all generations. Concurrent
  identical images should share uploads within the existing account/credential
  scope; include failure, cancellation and cache-capacity cases.
- Warm discovery-to-tool-less continuations, including client compaction through
  ordinary Responses: measure observed cached input and end-to-end latency
  against the cold path at matched model/effort/account. Check restart/expiry,
  changed instructions/context, zero current tool authority, and retained
  history. Stable prefix tests alone do not establish a live performance gain.
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

- Verify HTTP/2 refusal behavior against the installed executable: no hidden
  reqwest resend inside a recorded dispatch; preserve Relay's retry eligibility.
- Finish native Responses WS multiplexing: independent lanes/FIFO, bounded
  queues, scoped errors, continuation forks, retry/usage and disconnect ownership.
- Qualify mixed HTTP/SSE/WS/image admission on an authorized live pool. Local
  fixtures cover repeated mixed waves, lifecycle churn, bounded queue retention,
  and scheduler lock/reservation timings; they do not establish process-memory
  stability, provider latency, or long-running live fairness.
- Complete delayed-result, token-ownership, concurrent-refresh, rollback, and
  deleted/re-added account cases on Desktop.

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

- Establish a maintainer-approved private vulnerability-reporting channel, then
  document it in `SECURITY.md`. Enable GitHub private reporting or confirm another
  confidential contact before advertising it; do not invent an address, supported
  version policy or response-time promise. Public issues must not contain secrets
  or nonpublic exploit details.
- Upgrade Tauri/GTK as a tested platform change, not incidental cleanup.
- Keep profile/history recovery, Windows paths, partial cleanup, transactional
  bulk mutations, and concurrent policy edits covered by their owning checks.
- Keep current contracts, future work, user Help, and release history in their
  existing documents. Do not append investigation transcripts or completed
  implementation steps to this roadmap.

## Future connectors

- Qualify supported SIWC plan/quota telemetry before adding a quota reader;
  opaque token metadata and native Codex endpoints are not a compatible API.
- SIWC local acceptance: authorize a real grant with user consent, verify catalog,
  text → tool call → result → next answer, refresh/restart/revocation and denied
  plan permission. Qualify the Tauri WebView flow on supported platforms, including
  MFA/social-login restrictions, proxy, cancel/reopen and copy-link browser recovery;
  the provider documents system-browser sign-in, not embedded compatibility.
  Local tests do not establish live provider acceptance. Native
  upstream WebSockets, compaction, Hosted use and token transfer without a fresh
  sign-in remain unqualified. Test Zenith export/import and reauthorization on a
  second host separately; do not infer live support from local round-trip tests.

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
