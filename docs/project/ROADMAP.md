# Zenith Relay roadmap

Open Desktop work and acceptance criteria.
Current contracts are in [PLANNING.md](PLANNING.md); development and release
checks are in [CONTRIBUTING.md](../../CONTRIBUTING.md).
Future Hosted work has its own [delivery plan](../hosted/HOSTED_RELAY_ROADMAP.md).

## Local runtime P0 — finish and accept the current runtime

### Model, provider, and harness compatibility

The shared typed pipeline, four interoperable native paths plus native-only
Gemini Interactions, multiple source/model bindings,
and native-upstream selection are implemented. Remaining work covers
qualification and specific missing semantics.

| Work | Acceptance |
| --- | --- |
| Catalog provenance and refresh | Source failures preserve valid data; changed identity/price/capability is traceable; generated exports update atomically without private inventory or manual model rows. |
| Endpoint feature evidence | Advertised protocol does not grant unsupported continuation, tools, reasoning, cache, media, or usage semantics. Pins and explicit exclusions survive reload. |
| Conversation compatibility | Each advertised source/client/version passes text → call → result → next answer in JSON and SSE, including its supported reasoning/media/schema features. |
| Client startup context | Establish a lossless foreign representation for standalone Responses results without `call_id` and interleaved system/developer instructions, including Messages-to-Gemini history, before claiming full client parity; do not invent calls, erase context, or hoist instructions. |
| Native fidelity | Ordered schemas, precise numbers, repeated permitted headers, opaque state, and native extensions survive without lossy conversion. |
| Accounting and errors | One logical outcome plus retained attempts; partial/unknown usage remains explicit; the failing stage and origin are safe and understandable. |
| Client profiles | ChatGPT, Claude Desktop, and OpenCode expose only usable models/efforts/context for that route; attachment and recovery preserve user settings. |
| Claude Desktop effort menus | Qualify the actual Desktop picker/settings → embedded Code handoff and real-provider acceptance of Max/XHigh on catalog-derived opaque IDs. Find a supported metadata path for exact per-model lists. `maxEffort` is only a cap; `modelCatalogUrl` requires Anthropic-signed content. Do not hide higher choices, infer Max from received High, or patch client signature checks. |

Use the [official contracts](../../.agents/skills/relay-protocol-adapters/references/contracts.md)
and [conversation matrix](../../.agents/skills/relay-protocol-adapters/references/acceptance.md).
Synthetic coverage, a working endpoint, and live client acceptance are different
evidence. Discovery must not generate paid probes across a catalog.

### Client and provider acceptance

Gemini Interactions needs permitted live qualification of the exact source,
model and key scope for JSON/SSE text → calls → results → next answer, terminal
usage and interruption. Account OAuth, agents/background execution, remote
retrieval and cross-protocol conversion require separate qualification. Other
account connectors are paused; their current scope is frontend and
credential-family metadata.

BPS multi-tool batching has a bounded [Relay-local contract](BASIS_POINTS.md) and
synthetic roundtrip coverage for JSON/SSE, discovery, tool restrictions,
disabled parallel policy, typed results, and opaque reasoning. Remaining live
qualification covers larger batches, custom tools, discovery, media, explicit
disabled-parallel policy, interruption and malformed-batch outcomes on intended
harnesses. Qualify other declared model/effort/client pairs individually, including
multi-call envelope adoption, reversible IDs and grouped-result continuations.
Measure matched cold/warm cache reuse, TTFT and end-to-end latency separately,
across the complete client execution cycle.

Request-stage diagnostics distinguish selected-attempt preparation, upstream
headers/collection, normalized first stream output, and response/tool readiness.
Qualify their coverage on the intended HTTP/SSE/discovery paths after installation;
client receipt and between-request harness delays require client-side evidence.
Cold-preparation qualification should compare request building, its initial
schema-validation subset, catalog preparation, and attachment work. Use the
attachment breakdown to distinguish authorization, staging, upload/shared-wait
time and physical upload/reuse counts on the installed build. Locate live
bottlenecks with request-stage timings alongside synthetic validator benchmarks.
Use context-size and cache-prefix diagnostics to verify the cause of TTFT and
the safety of any history reduction before removing work.

| Area | Cases still requiring permitted live qualification |
| --- | --- |
| Provider presets | Exact endpoint/region/key scope, Moonshot USD/CNY statistics, Kimi Code discovery/tools; quota readers only after their units/reset contract is verified. |
| Lifecycle | Add, refresh, disable, trash/restore, removal, credential replacement, failed save/rollback, and restart on Desktop. |
| Outcomes | Completed, incomplete, refusal, malformed terminal, interruption, late usage, cancellation, and safe pre-output retries. Distinguish local pre-dispatch rejections from dispatched attempts across HTTP, account-only and WebSocket paths. Check state, not only displayed text. |
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
  identical images: qualify shared uploads within the existing account/credential
  scope using physical upload/reuse counts; include failure, cancellation and
  cache-capacity cases.
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
- Free native/BPS provider access after import, plan changes, restart, and
  transfer; timezone rewriting leaves ordinary text/API-source bodies intact.
- Quality comparisons only with authorized isolated routes and matched tasks,
  models, and effort. Keep inconclusive observations separate. Local solvers
  do not establish live account quality or eliminate degradation.

### Concurrency and load

- Qualify HTTP/2 refusal behavior on installed TLS/proxy/provider paths: no hidden
  reqwest resend inside a recorded dispatch, with Relay's retry eligibility
  preserved. Local prior-knowledge and certificate-verified TLS/ALPN/CONNECT
  fixtures cover physical-send accounting, reset/failover budgets and fail-closed
  proxy errors; installed binaries and actual provider/proxy combinations still
  require their own evidence.
- Qualify native Responses WS retry exhaustion, route/authorization changes,
  queued-parent eviction and installed-provider behavior across native reuse
  and automatic HTTP fallback.
- Qualify mixed HTTP/SSE/WS/image admission on an authorized live pool. Local
  fixtures cover repeated mixed waves, lifecycle churn, bounded queue retention,
  and scheduler lock/reservation timings; they do not establish process-memory
  stability, provider latency, or long-running live fairness.
- Complete delayed-result, token-ownership, concurrent-refresh, rollback, and
  deleted/re-added account cases on Desktop.

## P1 — measured routing and performance work

- Measure startup, page reads, saves, pool switches, history sizes, first-output
  latency, refresh work, and lock waits. Optimize the measured path, not a
  guessed protocol or assumed cache-hit benefit. Qualify the existing native,
  renderer, snapshot store-lock and command-roundtrip samples on the installed
  build; distinguish refresh acknowledgement from completed work and isolate
  lock waits separately.
- Measure restart persistence needs for explicit provider backoff deadlines
  beyond durable quota-reset snapshots, using recorded provider clocks and scope.
  Analyze durations from the original normalization time when available, not
  storage/finalization timestamps; unknown observation times remain unknown.
  Keep credential ownership separate from disposable health and circuit hints.
  Any restore must preserve policy-only edits while rejecting expired evidence,
  replacement credentials and deleted/re-added members.

## Maintenance

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
