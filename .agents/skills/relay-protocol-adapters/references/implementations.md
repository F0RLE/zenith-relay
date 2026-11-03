# Adapter implementation references

Current source pointers and separately pinned historical comparisons. Start with
[official contracts](contracts.md); qualify Relay behavior and model quality
separately. Check file-specific licensing and attribution before copying code.

Default-branch revisions, listed files and repository license declarations were
rechecked on **2026-10-23**. The refreshed source index and earlier comparison
revisions identify the exact code behind each observation. Adopt upstream
behavior only after evaluating the affected Relay contract.

| Repository | Rechecked revision | Earlier comparison | License | Rechecked paths |
| --- | --- | --- | --- | --- |
| QuantumNous/new-api | `33c61a24234aa20cb10e8a644304322c476361de` | `ba0e478e061ee0c824922d89c05620de878103cf` | AGPL-3.0 | `relaykit/relayconvert/{request_registry,response_registry}.go`, `relaykit/dto/reasoning_state.go` |
| router-for-me/CLIProxyAPI | `d946c4fba6fc9a5cfbca1f3d85184d62ad2fb683` | `091ff27f31285ec4fd1dd5823b44f7f51fb8f48b` | MIT | `sdk/translator/registry.go` |
| router-for-me/EasyCLIProxyAPI | `1d5198eb672a0e0e3257a275bd2b9e42951dbdeb` | `74ef3e16ab347161c7dbb7bddb531b0a3b950770` | MIT | `src-tauri/src/core_runtime.rs` |
| lidge-jun/opencodex | `3d9ba72aa5b04b4bbea5a2871a181942afa720e7` | `33b321853aff76045617824793cc7cbc1d4084e7` | MIT | `src/responses/reasoning-envelope.ts`, `src/adapters/openai-chat/tool-name-registry.ts` |
| zyycn/codex-proxy-rs | `e033e21994d08e86d4df71cf5c72e374a7a09987` | `1280c437b4376518130455bcc63e844c845f8852` | Apache-2.0 | `backend/crates/providers/openai/src/{request_identity.rs,transport/headers.rs}` |
| ranxi2001/sub2api | `7c6851cae25518f6e8e2541fc5394aad7c3c426c` | `05fa22f8f977b3c591712a2cecec09e42308f724` | LGPL-3.0 | `backend/internal/service/basispoints/{tool_images,history_messages,stream,structured_output}.go` |
| vercel/ai | `1bf6d85bfc0b395194e1a7aa79d0057b0c74a75a` | `ba05943b69ad070558bb5bc912da99ec66d7d263` | Apache-2.0 | `packages/openai/src/responses/convert-to-openai-responses-input.ts` |
| IuCC123/CLIProxyAPI-Rust | `b07936ba6b1a9faf557af44575f7d982f38636b0` | `7a3c8c2e017fafe9a3dc9a72dba5697feffcb992` | Unlicense | `src/{ir,sse}.rs`, `src/formats/claude.rs` |
| jeremychone/rust-genai | `7f221f150bcf4d8063f654e70ecf64cb5fbfe13a` | `092bc0ff53c203454f23393cac5825cfb1943a06` | MIT OR Apache-2.0 in Cargo.toml | `src/chat/{tool/tool_call,chat_request}.rs` |

## Additional inspected revisions

The **2026-11-01 BPS comparison** checked these default-branch snapshots:

- [ranxi2001/sub2api](https://github.com/ranxi2001/sub2api/tree/3c88bd92e569bd697e8ee77b4315f17d4f24e3fb),
  LGPL-3.0 `LICENSE` and adapter `NOTICE.md`: `backend/internal/service/basispoints/{stream,progress_timeout}.go`
  and `docs/bps-stream-progress-timeout.md`. Relay already distinguishes upstream
  read progress from comments and downstream waiting. Its no-regeneration policy
  remains stricter than the reference's optional tool-correction callbacks.
- [2han9wen71an/cpr-plugin-oai-basispoints](https://github.com/2han9wen71an/cpr-plugin-oai-basispoints/tree/bdb5302beacbe09c227bcc978ae56809e02beaf9),
  MIT `LICENSE`: `backend/src/upstream.rs`, particularly bounded response-body
  reads. Comparing bounds and terminal ownership exposed Relay's coalesced-tail
  rejection and lost partial usage; Relay fixes them independently in its existing
  collector/discovery owners and resumes fragmented SSE scans using its own parser.
- [hloolx/codex2api](https://github.com/hloolx/codex2api/tree/2caf579bc1e47905f5260ccfa3982b586a6d76a0),
  `proxy/basispoints_transport.go` and `README.md`: the README declares MIT, but
  no root license text was found. Its timeout disabling is not imported.
- [zhu961212/sub2api-oai-basispoints](https://github.com/zhu961212/sub2api-oai-basispoints/tree/7c6a0132d94301778d8fe4e87dad5dff919f0c53),
  `docs/CACHE-PREFIX-2026-09-30.md`: no license for the fork's own code was found.
  Stable prefixes and historical replay versus current permissions are already
  covered by Relay's scoped catalog/history contracts, not an unbounded cache.
- [JaxsonWang/cpa-plugin-oai-basispoints](https://github.com/JaxsonWang/cpa-plugin-oai-basispoints/tree/a5f698da41ed4d33eb1fe532116a96383e1df821),
  MIT `LICENSE`, `docs/credentials-and-protocol.md`: its fixed encoding and
  unsupported structured-output choices do not redefine BPS capabilities.

These are implementation comparisons, not provider guarantees. No third-party
code, account policy, automatic regeneration, or unverified request compression
is imported. Synthetic regressions cover the terminal/budget boundary with the
model guard both enabled and disabled; no authenticated generation is implied.

CLIProxyAPI main snapshot `dbb706215a3afd2f0ea3c96482d4233856466caa`, alongside
release `v8.0.48` (2026-10-31 comparison, MIT), informed legacy tool-search item IDs, inline Messages
tool changes and structured usage regression scenarios. Relevant changes:
`90c447d3a8727d115aa7552ec93ce023736ed492`
(`internal/runtime/executor/helps/codex_input_ids.go`,
`internal/translator/gemini/openai/responses/gemini_openai-responses_tool_search.go`),
`12d97cfe196be266b45f4f7e0cc394c9fdc2e099`
(`internal/translator/{codex,openai}/claude/*_claude_request.go`),
and PR #7074 (usage). Relay implements these independently through its Rust
adapter and usage owners. A synthetic comparison of compression-context reuse
(`16c6b4bd8c033b770ebd432d1b39fe53e61d1ee2`, `internal/api/response_compression.go`)
did not justify changing Relay's bounded endpoint-specific request compression
path. No Go implementation, retry policy or inferred charges are imported.

Official `openai/codex` at `970111f6cb62cc02fe36a82ea7d71b37a2889c97`
(2026-10-23 main snapshot, not a released-feature assertion) informed the local
optimization boundaries: `codex-rs/core/src/context/world_state/top_level_tools.rs`,
`codex-rs/codex-api/src/common.rs`, `codex-rs/core/src/client.rs`, and
`codex-rs/core/tests/suite/request_compression.rs`. Its incremental catalog
behavior is Responses-Lite-specific; its request compression is Codex-backend-
specific, not a generic public API capability. Relay implements its own bounded
presentation cache, stable transport prefix, payload telemetry and scoped
compression; no Codex source is copied. See the pinned
[client implementation](https://github.com/openai/codex/blob/970111f6cb62cc02fe36a82ea7d71b37a2889c97/codex-rs/core/src/client.rs).

The same pinned snapshot also informed bounded acknowledged persistence
(`codex-rs/rollout/src/recorder.rs`), prepared-image reuse
(`codex-rs/utils/image/src/lib.rs`), catalog-version signals
(`codex-rs/models-manager/src/manager.rs` and
`codex-rs/codex-api/src/sse/responses.rs`), and optional backend timing
(`codex-rs/otel/src/events/session_telemetry.rs`). Relay retains its own SQLite
commit contract and fenced refresh scheduler, caches only attachment digests/IDs
rather than image bytes, and does not copy image-resizing policy. Native account
headers/timing events remain observed backend behavior, not a guarantee for BPS,
SIWC or public API-compatible sources. The local implementation is independent;
no Codex source is copied.

The 2026-10-31 follow-up compared official main snapshot
`67f0994b532b8740f14b5b738af7b7b2a915b1d6` with the 2026-10-23 snapshot.
Released HTTP output retention (`d93071fae87f73ba1d251ebb3808773f18f0b77a`)
and prompt lifetime (`04e365c698b94651b6ea7e3b3af5c1dd697bd30c`) informed
Relay's post-dispatch BPS response context. Cancellation cleanup was compared
with `f276b82185f021e7b8d532a3851413bc79a67594` (main-only at review).
Network-only diagnostics followed the read-only/no-generation boundary from
`939f5decb264` and redaction evidence from `b538d2b340aee571fa1a81989cafb41f9d35b013`.
Relay keeps its own HTTP/WS protocols, dispatch fencing and credential store;
these comparisons do not enable Codex-specific compression on other APIs.

Historical behavior comparisons below retain their original revisions and
release labels. They are not a list of the newest releases.

CLIProxyAPI at `8289ec4c71ad914d0612c2776352b76ba3d355a6` (MIT) was inspected
on 2026-10-24 for translator regressions/benchmarks (`internal/translator/`),
session hints (`sdk/cliproxy/session/`), Interactions handlers
(`sdk/api/handlers/gemini/interactions_handlers.go`), and executor/watcher
extension points (`sdk/cliproxy/{service_executors,watcher}.go`). Relay implements
the selected scenarios independently in its existing Rust owners: bounded soft
opening-prefix affinity rather than a separate LCP/Merkle history store,
native-only Interactions against Google's official contract, and existing
runtime publication rather than external plugin loading. Foreign account
connectors remain metadata-only and paused; no upstream Go modules are copied.

codex-proxy-rs `v3.30.2` at `76291f448cdf1d1da2027da32b7c949deb998c7a`
(Apache-2.0) was inspected for local diagnostics, bounded retention and optional
WebSocket setup fallback: `frontend/src/views/usage/components/DiagnosticsDownloadButton.vue`,
`backend/crates/gateway-host/src/retention.rs`, and
`backend/crates/gateway-host/src/outbound/websocket.rs`. Relay implements these
concepts through its existing Desktop telemetry, IPC and execution owners;
no upstream modules or server/key-owner features are imported. Relay's strict
pre-execution fallback and report privacy contracts take precedence.

Sub2API `production` at `974a47f2ed722798566727a342df1eab3140dc94`
(LGPL-3.0) supplied comparison scenarios in
`openai_gateway_chat_completions.go`, `openai_upstream_session_error.go`,
`openai_http_phase_guard.go` and `basispoints/unknown_tool_repair.go` under
`backend/internal/service/`. Relay independently implements bounded declarations,
diagnostic labels and an opt-in response-header bound in Rust. It does not copy
the Go implementation, replay executable tools, import SIWC, or claim a separate
network upload-completion signal from reqwest.

Sub2API `production` at `12a49ef4ef6a2f116389701ac8f1fe620f27bbf5`
(2026-10-29 comparison, LGPL-3.0) informed typed capacity-rejection regressions,
strict refresh-error precedence and latency observations through
`backend/internal/service/{openai_gateway_upstream_errors,credential_operation_failure,priority_observation}.go`.
Relay implements these independently through its existing Rust classifier,
shared strict JSON parser and scheduler diagnostics. Its execution-certainty,
credential-rotation and EWMA selection policies retain their existing owners.

Sub2API `production` at `ec007920de042797299fc7cf2c034ad24c136462`
(2026-10-30 comparison, LGPL-3.0) informed independent HTTP SSE boundary
diagnostics and synthetic ordering/identity regressions through
`backend/internal/pkg/requesttiming/{http,account_connection}.go`,
`backend/internal/service/{openai_codex_snapshot_contract,session_id}.go` and
`openai_ws_forwarder_ingress.go`. Relay records only observable decoded-body,
SSE JSON and meaningful-output offsets; it does not invent raw decompression or
DNS/TLS phases. Existing proxy probes gain bounded timing without extra network
work. Relay's flat native WS request contract, quota-window clocks, credential
authority and scheduler remain authoritative. No Go implementation, multi-grant
database/Redis ownership or automatic sticky clearing is imported.

| Source | Revision or digest | Files / reason |
| --- | --- | --- |
| Sub2API (`v2.17.0`, LGPL-3.0) | `7c6851cae25518f6e8e2541fc5394aad7c3c426c` | `backend/internal/service/basispoints/{history_messages,request,tools}.go`, `openai_request_scoped_403.go`, `ratelimit_service.go`: observed Codex chat-handoff shape, internal WS refusal and Cloudflare 1010 evidence. Relay implements narrow Rust lowering/classification independently, rejects extra handoff fields, ignores echoed input, and retains its own retry/credential policy. No Go code, hosted moderation, persistent prompt capture or cluster ownership is imported. |
| Sub2API (`v2.16.0`, LGPL-3.0) | `5f779474d89a37953bda6e15f3df5a59921a0f21` | `backend/internal/service/priority_error_rate.go`: compared account/model/endpoint isolation. Relay retains its existing route-scoped health/circuit owner, with independent peer-cancellation and route-isolation regressions rather than an additional scoring engine. |
| Sub2API (`v2.15.0`, LGPL-3.0) | `8b36fe7d647c28e8189147c6bfce52a41b6e26db` | `backend/internal/service/{ratelimit_service,openai_siwc_errors,openai_encrypted_content_locator}.go`: compared forwarded subscription quota ownership, structured eligibility refusals and unique ciphertext locators. Relay independently implements these behaviors in its existing Rust policy/repair owners; no Go code, Redis lifecycle or retry-after-uncertain-execution policy is imported. |
| Sub2API (`v2.14.0`, LGPL-3.0) | `f3f72fa9893b7cc7ba76b32a59748ef84ee1004c` | `backend/internal/service/{openai_gateway_upstream_errors,openai_upstream_session_error,openai_siwc}.go`, `backend/internal/service/basispoints/structured_output.go`, `docs/responses-stream-keepalive.md`: compared error scope, opaque SIWC metadata, local schema validation and waiting-stage heartbeats. No Go code, retry policy or login changes copied; Relay retains token-rate failover and request-scoped policy handling, and does not substitute prompt-only validation for native strict output. |
| Sub2API (`v2.12.3`, LGPL-3.0) | `7cf8892c1d20cab786736e524cba983d80938fe8` | `docs/gateway-transport-diagnostics.md`, `backend/internal/service/basispoints/tool_schema_diagnostics.go`: separate ingress/egress observations and safe schema-error categories. Relay implements these independently in its existing routing JSON and adapter errors; no Go code or retry policy is copied. Credential/proxy regressions reuse Relay's existing authority and synthetic endpoints. |
| Sub2API (`production`, LGPL-3.0) | `6bc2eb2ffe858c60f1bd600c53683b66ae3cc1c6` | `backend/internal/service/basispoints/catalog.go`, `docs/bps-{compact,dynamic}-tool-catalog.md`: compact schema presentation and current-over-history catalogs; no native deferred search. Relay independently implements its own renderer and bounded local discovery, not copied Go code. |
| New API | `09109ec315a5086366f94bcce00f177ec831dbe1` | `relay/channel/openai/relay_responses.go`, `relay/helper/stream_scanner.go`: terminal and usage observation |
| New API | `d032204733db31208f10bac118b6e683aab3bf29` | `relaykit/relayconvert/internal/claude_messages/to_oai_responses_req.go`, `relaykit/dto/claude.go`, Responses-to-Messages JSON/SSE converters: history, metadata, thinking, deferred tools |
| JaxsonWang/cpa-plugin-oai-basispoints | `a5f698da41ed4d33eb1fe532116a96383e1df821` | `internal/basispoints/protocol.go`: metadata comparison, not an official BPS contract |
| Public BPS frontend | SHA-256 `87e120bee63d76f1204afd0a1b28b0e22adf6859aca716ecd917df2707b35fe8` | `assets/x-square-CXmFY5vk.js`: transport-owned metadata; historical observation only |

## Relay decisions

- Keep Relay's shared codecs and bounded volatile continuation store. Do not
  import another converter runtime, persistence layer, or host/model-specific
  repair policy.
- Native transport keeps wire fidelity. Reference SSE/WebSocket fixtures help
  test framing and numbers, but do not justify weaker terminal validation.
- Preserve provider-owned reasoning and original tool identities. Never replace
  missing signatures with sentinels, disable thinking to hide lost state, turn
  orphan results into user text, or weaken schemas and forced tool choice.
- BPS uses Relay's own envelope, attachment validation, metadata, and atomic
  tool release. Generic metadata forwarding, remote image fetching, or another
  project's public Responses converter is not a BPS specification.
- Client-selected deferred tools are expanded through the validated catalog;
  hosted search is not emulated. Original provider state stays separate from
  the client-visible thinking summary.
- Reference serializers and outbound SDKs are not four-protocol server
  replacements. Their warning/omission paths need review before reuse.
- No listed project establishes model-quality equivalence. Do not adopt paid
  external judging, identity spoofing, TLS camouflage, credential mixing, or
  retries after unknown execution as an anti-degradation feature.

Implementation contracts belong in
[PLANNING.md](../../../../docs/project/PLANNING.md#protocol-adapters);
conversation checks belong in [acceptance.md](acceptance.md).
