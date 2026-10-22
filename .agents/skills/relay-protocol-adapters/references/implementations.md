# Adapter implementation references

Historical source pointers for targeted comparisons. They do not define Relay's
contract or prove model quality. Use [official contracts](contracts.md) first;
recheck the exact revision, file license, and attribution before copying code.

| Repository | Inspected revision | Recorded license | Relevant paths |
| --- | --- | --- | --- |
| QuantumNous/new-api | `ba0e478e061ee0c824922d89c05620de878103cf` | AGPL-3.0 | `relaykit/relayconvert/{request_registry,response_registry}.go`, `relaykit/dto/reasoning_state.go` |
| router-for-me/CLIProxyAPI | `091ff27f31285ec4fd1dd5823b44f7f51fb8f48b` | MIT | `sdk/translator/registry.go`, `internal/translator/` |
| router-for-me/EasyCLIProxyAPI | `74ef3e16ab347161c7dbb7bddb531b0a3b950770` | MIT | `src-tauri/src/core_runtime.rs`; manages a separate CLIProxyAPI binary |
| lidge-jun/opencodex | `33b321853aff76045617824793cc7cbc1d4084e7` | MIT | `src/responses/{reasoning-envelope,thought-signature-replay}.ts`, `src/adapters/openai-chat/tool-{call-validation,name-registry}.ts` |
| zyycn/codex-proxy-rs | `1280c437b4376518130455bcc63e844c845f8852` | Apache-2.0 | `backend/crates/providers/openai/src/{request_identity.rs,transport/headers.rs}`, `backend/crates/providers/openai/tests/transport/canonical.rs` |
| ranxi2001/sub2api | `05fa22f8f977b3c591712a2cecec09e42308f724` | LGPL-3.0 | `backend/internal/service/basispoints/{tool_images,history_messages,stream,terminal_tools_test,structured_output}.go` |
| vercel/ai | `ba05943b69ad070558bb5bc912da99ec66d7d263` | Apache-2.0 | `packages/openai/src/responses/convert-to-openai-responses-input.ts`, Anthropic and Google prompt serializers |
| IuCC123/CLIProxyAPI-Rust | `7a3c8c2e017fafe9a3dc9a72dba5697feffcb992` | Unlicense | `src/{ir,sse}.rs`, `src/formats/claude.rs` |
| jeremychone/rust-genai | `092bc0ff53c203454f23393cac5825cfb1943a06` | MIT OR Apache-2.0 in Cargo.toml | `src/chat/{tool/tool_call,chat_request}.rs`, `src/adapter/adapters/{gemini,anthropic}/` |

## Additional inspected revisions

Sub2API `production` at `974a47f2ed722798566727a342df1eab3140dc94`
(LGPL-3.0) supplied comparison scenarios in
`openai_gateway_chat_completions.go`, `openai_upstream_session_error.go`,
`openai_http_phase_guard.go` and `basispoints/unknown_tool_repair.go` under
`backend/internal/service/`. Relay independently implements bounded declarations,
diagnostic labels and an opt-in response-header bound in Rust. It does not copy
the Go implementation, replay executable tools, import SIWC, or claim a separate
network upload-completion signal from reqwest.

| Source | Revision or digest | Files / reason |
| --- | --- | --- |
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
