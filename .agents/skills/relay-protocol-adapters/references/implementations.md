# Adapter implementation references

These revisions identify inspected source. Recheck relevant upstream changes
before adoption; a reference is not proof that Relay's route works.
Choose by the broken contract rather than importing an entire proxy or SDK.

| Source | Inspected revision | License at revision | Useful area |
| --- | --- | --- | --- |
| QuantumNous/new-api | `ba0e478e061ee0c824922d89c05620de878103cf` | AGPL-3.0 | Converter registry, reasoning state, stream lifecycle |
| router-for-me/CLIProxyAPI | `091ff27f31285ec4fd1dd5823b44f7f51fb8f48b` | MIT | Protocol translators, tool names, Gemini signature carrier |
| router-for-me/EasyCLIProxyAPI | `74ef3e16ab347161c7dbb7bddb531b0a3b950770` | MIT | Desktop lifecycle for a separate CLIProxyAPI binary |
| lidge-jun/opencodex | `33b321853aff76045617824793cc7cbc1d4084e7` | MIT | Reasoning carrier, scoped signature replay, tool validation |
| zyycn/codex-proxy-rs | `1280c437b4376518130455bcc63e844c845f8852` | Apache-2.0 | Native Responses fidelity, root/thread identity, raw transport fixtures |
| ranxi2001/sub2api | `05fa22f8f977b3c591712a2cecec09e42308f724` | LGPL-3.0 | BPS tool images, collaboration history, atomic streamed tools, quality observations |
| vercel/ai | `ba05943b69ad070558bb5bc912da99ec66d7d263` | Apache-2.0 | Provider metadata and native conversation serialization |
| IuCC123/CLIProxyAPI-Rust | `7a3c8c2e017fafe9a3dc9a72dba5697feffcb992` | Unlicense | Rust intermediate representation and SSE handling |
| jeremychone/rust-genai | `092bc0ff53c203454f23393cac5825cfb1943a06` | MIT OR Apache-2.0 in Cargo.toml | Rust outbound adapters and captured assistant turns |

## New API: conversion ownership and stream lifecycle

The BPS stream-normalization comparison also inspected New API revision
`09109ec315a5086366f94bcce00f177ec831dbe1`,
`relay/channel/openai/relay_responses.go` and `relay/helper/stream_scanner.go`,
against OpenAI's Responses streaming-event reference. New API forwards decoded
Responses frames and separately accumulates terminal/usage evidence. Relay's
proprietary BPS tool envelope has no equivalent in these files: it keeps atomic
local tool validation, the shared Responses terminal aliases, and explicit
incomplete outcomes. A bare `[DONE]` cannot prove successful BPS completion.
The optimization passes owned parsed values/events between existing helpers;
it does not adopt another converter or copy reference implementation code.

- [Request registry](https://github.com/QuantumNous/new-api/blob/ba0e478e061ee0c824922d89c05620de878103cf/relaykit/relayconvert/request_registry.go)
  and [response registry](https://github.com/QuantumNous/new-api/blob/ba0e478e061ee0c824922d89c05620de878103cf/relaykit/relayconvert/response_registry.go)
  describe conversion direction, quality, and diagnostics.
- [Reasoning conversion state](https://github.com/QuantumNous/new-api/blob/ba0e478e061ee0c824922d89c05620de878103cf/relaykit/dto/reasoning_state.go)
  carries reasoning configuration internally without serializing it as a wire field.
- [Responses to Messages](https://github.com/QuantumNous/new-api/blob/ba0e478e061ee0c824922d89c05620de878103cf/relaykit/relayconvert/internal/oai_responses/to_claude_messages_req.go)
  and its [stream response converter](https://github.com/QuantumNous/new-api/blob/ba0e478e061ee0c824922d89c05620de878103cf/relaykit/relayconvert/internal/oai_responses/to_claude_messages_stream_resp.go)
  are useful for tool attachment and event sequencing.

Check conversion diagnostics against Relay's contract. In particular,
`to_gemini_chat_req.go` can omit unsupported media with a diagnostic; that does
not justify dropping meaningful input in a strict Relay conversion.

The Messages-to-Responses options comparison also inspected New API revision
`d032204733db31208f10bac118b6e683aab3bf29`:
`relaykit/relayconvert/internal/claude_messages/to_oai_responses_req.go` copies
request metadata but does not copy Anthropic context edits into Responses;
`relaykit/dto/claude.go` defines the corresponding metadata and cache fields.
Relay independently validates portable metadata and advisory cache hints against
the provider contracts. It does not adopt permissive omission of required
features or claim equivalent foreign cache TTLs/pricing.

The same Messages-to-Responses file at `d032204733db31208f10bac118b6e683aab3bf29`
uses compact `{role, content}` message items. OpenAI's Responses contract permits
this shorthand, including an omitted `type` and string content. Relay retains
that portable codec contract but expands these messages into explicit message
and text-block variants at its BPS executor boundary; this transport-specific
normalization does not alter other native provider routes.

For client-owned deferred discovery, the same file's
`claudeToolsToResponsesTools` converts every declaration into a function;
`claudeToolResultToResponsesOutput` handles text/images/documents but has no
explicit reference expansion. Relay intentionally differs: it validates the
whole catalog, loads only client-selected deferred definitions and preserves a
compact discovery marker within the original tool result. This independently
written behavior follows Anthropic's
[custom tool search contract](https://platform.claude.com/docs/en/agents-and-tools/tool-use/tool-search-tool#custom-tool-search-implementation)
and [Desktop gateway setting](https://claude.com/docs/third-party/claude-desktop/gateway#mcp-tool-search).
It does not copy a search engine, emulate hosted search or promise identical
foreign prompt-cache placement. Native Messages is not rewritten.

The Responses-to-Messages JSON converter at the same revision,
`relaykit/relayconvert/internal/oai_responses/to_claude_messages_resp.go`,
represents public reasoning text as an unsigned `thinking` block and maps
function calls to `tool_use` with `stop_reason: tool_use`. Relay independently
uses that display representation for JSON and SSE; it does not manufacture
Anthropic signatures. Original encrypted/signed upstream turns remain in
Relay's existing bounded, route-and-credential-scoped continuation store.
Its stream converter, `to_claude_messages_stream_resp.go` at the same revision,
also tracks thinking deltas and reconciles final values in sequential block
state. Relay's independently written BPS SSE emitter supplies the standard
reasoning-summary part/text lifecycle before tools, so its existing strict
Messages bridge can preserve that order. It does not weaken terminal checks or
turn foreign encrypted state into a client signature.

The request converter at `d032204733db31208f10bac118b6e683aab3bf29`,
`relaykit/relayconvert/internal/claude_messages/to_oai_responses_req.go`,
maps calls/results by their original call ID without a signature carrier.
Relay additionally accepts an empty/null signature on its portable thinking
summaries, matching Claude's stream-assembled history. It rejects nonempty
foreign signatures instead of silently discarding them, and still requires the
existing scoped native continuation for reasoning tool turns, consistent with
[Anthropic's thinking encryption contract](https://platform.claude.com/docs/en/build-with-claude/thinking#thinking-encryption).

The generic metadata copy is not valid for the strict BPS transport. Relay
keeps client metadata locally, sends only its own task/turn/iteration fields,
and restores client metadata in translated JSON/SSE. The public BPS frontend
`assets/x-square-CXmFY5vk.js` (SHA-256
`87e120bee63d76f1204afd0a1b28b0e22adf6859aca716ecd917df2707b35fe8`)
constructs transport-owned metadata rather than forwarding arbitrary client
maps. The separately inspected third-party `JaxsonWang/cpa-plugin-oai-basispoints`
revision `a5f698da41ed4d33eb1fe532116a96383e1df821`,
`internal/basispoints/protocol.go`, merges generic metadata; Relay does not adopt
that behavior or treat this plugin as an official provider contract.

## codex-proxy-rs: native fidelity without another converter

Inspected revision `1280c437b4376518130455bcc63e844c845f8852` exposes the
Responses API, not a universal four-protocol replacement:

- `backend/crates/providers/openai/Cargo.toml` enables ordered objects and
  arbitrary-precision JSON numbers. Relay adopts these serde_json features
  alongside its existing raw-value support, including numeric memory accounting.
- `backend/crates/gateway-api/src/openai/responses/request.rs` and
  `backend/crates/providers/openai/src/transport/headers.rs` retain eligible
  repeated headers while excluding credentials and hop-by-hop state. Relay
  keeps its protocol allowlists instead of forwarding arbitrary extensions.
- `backend/crates/providers/openai/src/request_identity.rs` and
  `backend/crates/gateway-protocol/src/openai/codex.rs` distinguish logical root
  sessions from child threads and request-local metadata. Relay adapts bounded
  hint extraction into its existing affinity/context layer, not its strict
  response-owner or turn-state authorization. Explicit cache-key preference,
  local-key/model isolation, and credential provenance stay Relay-owned.
- `backend/crates/providers/openai/tests/transport/canonical.rs` checks raw SSE
  framing and WebSocket numeric bytes independently of parsed observations.
  Relay's synthetic regressions exercise its own stream observers and real
  loopback WebSocket forwarding without importing a second transport engine.

The source and tests are Apache-2.0; these are independently adapted behaviors
and synthetic fixtures, not copied modules. Do not adopt prompt-derived
identity, persistent session storage, identity spoofing, TLS camouflage, or
weaker malformed-stream acceptance. No controlled model-quality comparison is
established by this implementation reference. OpenAI's prompt-caching guide
describes prefix reuse, not a change in model output generation.

## Sub2API: BPS compatibility and bounded quality observations

Inspected revision `05fa22f8f977b3c591712a2cecec09e42308f724` on `production`:

- `backend/internal/service/basispoints/tool_images.go` and
  `history_messages.go` preserve tool-image context and normalize agent history.
  Relay independently lowers images into marked attachment messages through its
  existing validator/uploader. Remote URL fetching is deliberately not adopted.
- `backend/internal/service/basispoints/stream.go` and
  `terminal_tools_test.go` hold executable tools for authoritative completion.
  Relay uses its existing Responses emitter and client protocol bridges, while
  streaming ordinary text, enforcing bounded state, and retaining raw usage and
  timing. Invalid/incomplete terminals never release a valid tool subset.
- `backend/internal/service/basispoints/structured_output.go` motivates an
  explicit local-validation mode. Relay keeps the original offline JSON Schema,
  buffers this mode, rejects native `strict: true`, and never silently drops the
  request's format or retries a completed invalid answer.
- `backend/internal/service/quality_random_question.go`, `quality_judge.go`,
  `docs/account-quality-operations.md`, and `docs/quality-bps-observation.md`
  separate question generation, judging and observations. Relay instead uses
  independent bounded local solvers and no paid external judge. Its manual
  loopback example does not control account selection or bypass ordinary gateway
  health/quota effects, so it is not Sub2API's isolated BPS observation channel.

This is an independently written Rust implementation, not copied Go modules or
prompts. Preserve LGPL-3.0 requirements if source is copied in future changes.
Do not adopt silent tool/history omission, unknown-execution retries, credential
mixing, forced reasoning defaults, TLS camouflage, or ticket length as quality
evidence. Neither project's feature list proves model-quality equivalence.

## OpenCodex: state that a harness may not echo

- [Reasoning envelope](https://github.com/lidge-jun/opencodex/blob/33b321853aff76045617824793cc7cbc1d4084e7/src/responses/reasoning-envelope.ts)
  uses a versioned `ocxr1:` carrier for provider state. Native encrypted payloads
  have a model/route binding; native OpenAI blobs are not decrypted by the proxy.
- [Thought-signature replay](https://github.com/lidge-jun/opencodex/blob/33b321853aff76045617824793cc7cbc1d4084e7/src/responses/thought-signature-replay.ts)
  keys state by thread, destination, provider, adapter, model, credential
  identity, and call ID. It bounds TTL, entry count, and total bytes, and detects
  conflicting signatures.
- [Tool-call validation](https://github.com/lidge-jun/opencodex/blob/33b321853aff76045617824793cc7cbc1d4084e7/src/adapters/openai-chat/tool-call-validation.ts)
  and [tool-name registry](https://github.com/lidge-jun/opencodex/blob/33b321853aff76045617824793cc7cbc1d4084e7/src/adapters/openai-chat/tool-name-registry.ts)
  are references for complete identity, bounded aliases, and collision handling.
  `response-events.ts` distinguishes absent metadata from malformed fields.

Keep Relay's volatile storage contract. Do not copy this project's JSON replay
persistence or its host/model-specific repairs in `tool-schema.ts` by default.
Flattening schema unions can change which arguments are valid.

## CLIProxyAPI: protocol pairs and call grouping

EasyCLIProxyAPI's
[core runtime](https://github.com/router-for-me/EasyCLIProxyAPI/blob/74ef3e16ab347161c7dbb7bddb531b0a3b950770/src-tauri/src/core_runtime.rs)
manages a separate CLIProxyAPI binary. The translators live in CLIProxyAPI:

- [Translator registry](https://github.com/router-for-me/CLIProxyAPI/blob/091ff27f31285ec4fd1dd5823b44f7f51fb8f48b/sdk/translator/registry.go).
- [Claude tool names](https://github.com/router-for-me/CLIProxyAPI/blob/091ff27f31285ec4fd1dd5823b44f7f51fb8f48b/internal/translator/claude/openai/responses/claude_openai-responses_tool_names.go).
- [Gemini request translator](https://github.com/router-for-me/CLIProxyAPI/blob/091ff27f31285ec4fd1dd5823b44f7f51fb8f48b/internal/translator/gemini/openai/responses/gemini_openai-responses_request.go)
  and [signature carrier](https://github.com/router-for-me/CLIProxyAPI/blob/091ff27f31285ec4fd1dd5823b44f7f51fb8f48b/internal/translator/gemini/openai/responses/signature_carrier.go).
- [Chat/Responses tool conversion](https://github.com/router-for-me/CLIProxyAPI/blob/091ff27f31285ec4fd1dd5823b44f7f51fb8f48b/internal/translator/openai/openai/responses/openai_openai-responses_tools.go).

The Gemini translator includes a missing-signature sentinel, model-name
conditions, and conversion of orphan tool results into user text. Compare each
behavior with the requested contract; do not adopt these fallbacks as universal
normalization.

## Vercel AI SDK: preserve native provider metadata

- [OpenAI Responses input](https://github.com/vercel/ai/blob/ba05943b69ad070558bb5bc912da99ec66d7d263/packages/openai/src/responses/convert-to-openai-responses-input.ts)
  distinguishes response item IDs from tool call IDs and carries encrypted
  reasoning through provider metadata.
- [Anthropic prompt conversion](https://github.com/vercel/ai/blob/ba05943b69ad070558bb5bc912da99ec66d7d263/packages/anthropic/src/convert-to-anthropic-prompt.ts)
  retains thinking signatures and redacted data.
- [Google message conversion](https://github.com/vercel/ai/blob/ba05943b69ad070558bb5bc912da99ec66d7d263/packages/google/src/convert-to-google-messages.ts)
  associates thought signatures with their original parts and function calls.

These TypeScript serializers are references, not a Rust server adapter.
Inspect warning paths too: filtering unavailable reasoning metadata or filling
missing signatures is not automatically acceptable for Relay.

## Rust candidates: useful components, incomplete replacements

CLIProxyAPI-Rust's
[IR](https://github.com/IuCC123/CLIProxyAPI-Rust/blob/7a3c8c2e017fafe9a3dc9a72dba5697feffcb992/src/ir.rs)
and [SSE parser](https://github.com/IuCC123/CLIProxyAPI-Rust/blob/7a3c8c2e017fafe9a3dc9a72dba5697feffcb992/src/sse.rs)
are useful Rust examples. Its
[Claude builder](https://github.com/IuCC123/CLIProxyAPI-Rust/blob/7a3c8c2e017fafe9a3dc9a72dba5697feffcb992/src/formats/claude.rs)
disables manual thinking when tool history lacks a signature, can replace forced
tool choice with `auto`, and omits some foreign blocks. It also contains
model-name conditions. These behaviors prevent a claim of lossless replacement.

Rust GenAI's
[ToolCall](https://github.com/jeremychone/rust-genai/blob/092bc0ff53c203454f23393cac5825cfb1943a06/src/chat/tool/tool_call.rs),
[ChatRequest](https://github.com/jeremychone/rust-genai/blob/092bc0ff53c203454f23393cac5825cfb1943a06/src/chat/chat_request.rs),
and [Gemini adapter](https://github.com/jeremychone/rust-genai/blob/092bc0ff53c203454f23393cac5825cfb1943a06/src/adapter/adapters/gemini/adapter_impl.rs)
show captured assistant turns and thought-signature handling. It is an outbound
SDK, not a server accepting all four client protocols. The
[Anthropic serializer](https://github.com/jeremychone/rust-genai/blob/092bc0ff53c203454f23393cac5825cfb1943a06/src/adapter/adapters/anthropic/adapter_shared.rs)
drops thinking when reasoning/signature lists cannot be paired. Inspect that
loss boundary before reusing its representation.

## Official contracts

- [OpenAI reasoning](https://developers.openai.com/api/docs/guides/reasoning):
  replay reasoning items alongside calls/results; ciphertext stays opaque to
  Relay. Check the current API's defaults rather than assuming `include` is
  always required.
- [Anthropic Messages](https://platform.claude.com/docs/en/api/messages/create)
  and [thinking](https://platform.claude.com/docs/en/build-with-claude/thinking):
  preserve original thinking/signature and redacted blocks during tool replay.
  Tool-choice restrictions depend on the thinking mode and supported contract.
- [Gemini Generate Content thought signatures](https://ai.google.dev/gemini-api/docs/generate-content/thought-signatures):
  preserve the original parts and call grouping. A parallel call group may have
  a signature only on its first call. The documented sentinel applies to
  synthetic or migrated history, not automatic recovery of lost real state.
  Generate Content and Interactions use different state representations.

When adapting code, retain provenance and applicable license notices for the
specific files. A repository license label does not replace checking the
material being copied.
