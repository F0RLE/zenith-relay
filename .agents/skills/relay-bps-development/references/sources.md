# BPS source map

Use the section matching the task, not the entire list. These are research
entrypoints, not dependencies or a server specification. The frontend snapshot
below was refreshed on 2026-11-05; third-party references were inspected on
2026-10-31. Pinned third-party revisions and asset hashes are
historical evidence and must not be presented as a permanently current release.

## Local owners and official comparison contracts

- [Relay BPS contract](../../../../docs/project/BASIS_POINTS.md): executor,
  native-envelope replay, catalogs/discovery, attachments, authorization,
  diagnostics, and capability limits. Current source and focused tests establish
  Relay behavior; this source map does not replace them.
- [Inspected adapter implementations](../../relay-protocol-adapters/references/implementations.md):
  existing adoption decisions and attribution. Consult before repeating an
  upstream comparison or recording a new adoption in a parallel document.
- [Function calling](https://developers.openai.com/api/docs/guides/function-calling)
  and [streaming Responses](https://developers.openai.com/api/docs/guides/streaming-responses):
  public tool lifecycle and stream events, not a BPS feature guarantee.
- [Prompt caching](https://developers.openai.com/api/docs/guides/prompt-caching)
  and [compaction](https://developers.openai.com/api/docs/guides/compaction):
  public API cache/context behavior. In particular, encrypted compaction items
  and server-side context management need separate BPS compatibility evidence.
- For another public API feature, use the
  [provider contract index](../../relay-protocol-adapters/references/contracts.md)
  instead of assuming that similarly named BPS fields have the same semantics.

No complete official public BPS API specification was found in this research.
That is a search result, not proof that documentation or a capability cannot exist.

## Primary BPS artifacts

### Official Office manifest

[Public manifest](https://bps.openai.com/basispoints/api/office/manifest.xml)
identifies the publisher and add-in entrypoint. The inspected version was
`2.0.0.1`, publisher `OpenAI`, display name `ChatGPT`, declaring Workbook,
Presentation, and Document hosts. Host declarations do not prove account access
or full feature availability in every Office application.

Use its current `SourceLocation` to locate the public application, then inspect
the assets actually referenced by that page. Do not treat a dated hashed asset
filename, extension path, or root-page 404 as a stable API contract.

### Inspected frontend snapshot

[Public client asset](https://bps.openai.com/basispoints/extension/360590d7-f8f9-4d88-bf75-0edfe0a4b9f3/assets/x-square-B_-jh0MM.js),
SHA-256 `e7d27638b63a2f78e5c049e1ade619578077563efd42d86cd014dd152f5a7859`.
Located through the current manifest, entrypoint and module imports on
2026-11-05. ChatGPT authorization still uses
`https://bps.openai.com/basispoints/api/responses/access?include_models=true`;
the alternate `/access` rewrite belongs to API-key mode, not ChatGPT OAuth.
This public-client observation does not prove an authenticated route is healthy
or explain an individual proxy/account's HTTP 404.

Useful search anchors, rather than unstable minified offsets:

- `basispoints_proxy`, `direct`, `reasoning_effort`, `model_selection`: separate
  request modes and preparation. A direct-mode option does not prove BPS support.
- `preferWebSocket`, `allowWebSocketFallback`, `responses.create`: transport
  selection. Trace eligibility and retry conditions before claiming support;
  the presence of a WebSocket branch is insufficient.
- `/responses/access`, `/responses/models`, `/responses/compact`,
  `/responses/prompt-versions`: client-observed paths, not permission to probe
  them or a documented public endpoint surface.
- `maxRequestSerializedChars`, `maxMatrixWriteCells`: Office sandbox broker
  validation, not HTTP body limits, model context limits, or Relay tool limits.
- `CompressionStream`, `enable_log_event_compression`: the observed gzip path
  belongs to Statsig telemetry. No `Content-Encoding` or `zstd` match was found
  in this file; this does not prove the server rejects compressed generation.

Inspect public artifacts as text, without executing downloaded application code.
Keep minimal attributable observations rather than entire bundles or prompts.

## Research on the Office client

### hewliyang/reversing

Inspected revision: `f0320f894496f9fd1fb461d65de8029460c5894a`
(commit dated 2026-10-13). Public reverse engineering, not official documentation;
license/reuse permission was not established by this research.

- [Excel sandbox](https://github.com/hewliyang/reversing/blob/f0320f894496f9fd1fb461d65de8029460c5894a/chatgpt-for-excel/sandbox.md):
  QuickJS/WASM in an isolated iframe, MessageChannel message flow, proxy objects,
  `ctx.sync()` batching, serialization, and execution limits. The older captured
  build and the researcher's test fixes do not establish current production
  limits or a current BPS defect.
- [Excel tools](https://github.com/hewliyang/reversing/blob/f0320f894496f9fd1fb461d65de8029460c5894a/chatgpt-for-excel/tools.md):
  19 tools reconstructed from a client labeled v2025. Useful for native tool
  semantics, not an authoritative current catalog.
- [PowerPoint capture description](https://github.com/hewliyang/reversing/blob/f0320f894496f9fd1fb461d65de8029460c5894a/chatgpt-for-powerpoint/README.md):
  an event dated 2026-10-13 with 27 function tools plus hosted web search. Useful
  for host-specific catalogs and envelope shape, not universal model access,
  cache performance, or current tool availability. Read the description first;
  do not copy the captured conversation/system prompt into Relay fixtures.

## BPS bridge implementations

These are design comparisons, not normative contracts. Re-check each revision's
license and attribution before reuse; do not execute or install a proxy merely
to read its implementation. GitHub's inferred license metadata is not enough.

### Sub2API: catalog, history, and stream handling

[Source tree](https://github.com/ranxi2001/sub2api/tree/55da3ead72ce6f6126a977c75abf72cb4ad3f394/backend/internal/service/basispoints)
at `55da3ead72ce6f6126a977c75abf72cb4ad3f394` (2026-10-31).
The inspected project declares LGPL-3.0; retain applicable notices for any reuse.

- [Compact catalog](https://github.com/ranxi2001/sub2api/blob/55da3ead72ce6f6126a977c75abf72cb4ad3f394/docs/bps-compact-tool-catalog.md):
  schema presentation and a synthetic catalog-size benchmark. Its reduction is
  not a whole-request token, latency, or billing guarantee.
- [Dynamic catalog](https://github.com/ranxi2001/sub2api/blob/55da3ead72ce6f6126a977c75abf72cb4ad3f394/docs/bps-dynamic-tool-catalog.md):
  changing tools while retaining historical identity and current permissions.
- [Stream progress timeout](https://github.com/ranxi2001/sub2api/blob/55da3ead72ce6f6126a977c75abf72cb4ad3f394/docs/bps-stream-progress-timeout.md):
  meaningful progress versus transport activity. Compare with Relay's existing
  timeout and terminal owner rather than transplanting retry policy.
- `structured_output.go` demonstrates local validation. It does not establish
  native BPS constrained decoding. `NOTICE.md` records codex2api attribution.

### Standalone plugin: cache prefix and serialization

[zhu961212/sub2api-oai-basispoints](https://github.com/zhu961212/sub2api-oai-basispoints/tree/7c6a0132d94301778d8fe4e87dad5dff919f0c53),
revision `7c6a0132d94301778d8fe4e87dad5dff919f0c53` (2026-09-30):

- [Cache prefix](https://github.com/zhu961212/sub2api-oai-basispoints/blob/7c6a0132d94301778d8fe4e87dad5dff919f0c53/docs/CACHE-PREFIX-2026-09-30.md):
  avoiding rewrites of old conversation prefixes when tools change. Reported
  cache hits are not controlled before/after evidence.
- [Relay serialization](https://github.com/zhu961212/sub2api-oai-basispoints/blob/7c6a0132d94301778d8fe4e87dad5dff919f0c53/docs/RELAY-SERIALIZATION-2026-09-30.md):
  deterministic framing, historical call identity, and rejecting ambiguity.

No license file was found in the inspected archive; copying permission remains
unestablished. Use the documents for comparison, not unqualified code reuse.

### Earlier Go and Rust bridges

- [JaxsonWang/cpa-plugin-oai-basispoints](https://github.com/JaxsonWang/cpa-plugin-oai-basispoints/tree/a5f698da41ed4d33eb1fe532116a96383e1df821),
  revision `a5f698da41ed4d33eb1fe532116a96383e1df821`: inspected as archived,
  MIT. `docs/credentials-and-protocol.md` and `internal/basispoints/protocol.go`
  explain that implementation's envelope and limits. Its unsupported
  `text.format` and `Accept-Encoding: identity` are not universal server rules.
- [2han9wen71an/cpr-plugin-oai-basispoints](https://github.com/2han9wen71an/cpr-plugin-oai-basispoints/tree/bdb5302beacbe09c227bcc978ae56809e02beaf9),
  revision `bdb5302beacbe09c227bcc978ae56809e02beaf9` (2026-10-06), MIT:
  `backend/src/{protocol,middleware,upstream}.rs` provides a Rust comparison for
  request lowering and transport. It is not a ready-made Relay adapter.
- [hloolx/codex2api](https://github.com/hloolx/codex2api/tree/2caf579bc1e47905f5260ccfa3982b586a6d76a0),
  revision `2caf579bc1e47905f5260ccfa3982b586a6d76a0` (2026-10-08):
  `internal/basispoints/` and `proxy/basispoints_transport.go` are original BPS
  bridge references. `proxy/codex_request_compression.go` concerns the Codex
  transport, not evidence of BPS request compression. License verification is
  still required; another project's attribution does not settle reuse rights.

## Questions these sources do not settle

- **Request gzip/zstd:** require evidence for the exact BPS generation endpoint.
  `Accept-Encoding`, a 401/422, response decompression, telemetry gzip, and native
  Codex compression are insufficient. Follow Relay's
  [request-encoding contract](../../../../docs/project/BASIS_POINTS.md#http-request-encoding).
- **Context and cache:** distinguish compact tool descriptions, stable-prefix
  cache reuse, and semantic/history compaction. Preserve images, executable
  schemas, call identities, and opaque state; smaller JSON alone proves none of
  these semantics or a reduction in model tokens.
- **Structured output:** distinguish JSON mode, prompt-only formatting, local
  schema validation, and native constrained decoding for the exact route.
- **Reliability:** client retry settings and public captures do not explain
  time-of-day failures. Separate provider status, local rejection, first-byte
  delay, stream progress, and terminal success using the owning diagnostics.

Keep the missing evidence explicit. A read-only research task should end with
attributable findings and gaps, not an unrequested live probe or implementation.
