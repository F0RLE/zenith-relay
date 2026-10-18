# Official protocol contracts

Read the contract for the actual endpoint, not just the model family. These
links are starting points, not snapshots; check the current documented fields
and corresponding official SDK types before changing behavior.

| Protocol | Primary sources | Boundaries to verify |
| --- | --- | --- |
| OpenAI Responses | [API reference](https://developers.openai.com/api/reference/resources/responses/methods/create), [function calling](https://developers.openai.com/api/docs/guides/function-calling), [tool search](https://developers.openai.com/api/docs/guides/tools-tool-search), [streaming](https://developers.openai.com/api/docs/guides/streaming-responses) | Item ID versus call ID, namespaces, client versus server execution, terminal status, encrypted reasoning, continuation |
| OpenAI Chat Completions | [API reference](https://developers.openai.com/api/reference/resources/chat/subresources/completions/methods/create), [reasoning](https://developers.openai.com/api/docs/guides/reasoning) | Message/tool grouping, supported options, streamed argument fragments, usage and finish reason |
| Anthropic Messages | [Messages](https://platform.claude.com/docs/en/api/messages/create), [streaming](https://platform.claude.com/docs/en/build-with-claude/streaming), [thinking](https://platform.claude.com/docs/en/build-with-claude/thinking), [tool use](https://platform.claude.com/docs/en/agents-and-tools/tool-use/overview) | Ordered content blocks, tool use/result IDs, signatures, stop reasons, input/output/cache usage |
| Gemini Generate Content | [API reference](https://ai.google.dev/api/generate-content), [Schema SDK type](https://googleapis.github.io/js-genai/release_docs/interfaces/types.Schema.html), [function calling](https://ai.google.dev/gemini-api/docs/function-calling), [thought signatures](https://ai.google.dev/gemini-api/docs/thought-signatures) | Candidate/part order, function call IDs, native signatures, schema dialect, finish reason and usage metadata |

Provider-compatible APIs need their own official capability and endpoint
documentation. OpenAI wire compatibility alone does not establish stateful
Responses support, tool search, structured outputs, or a reasoning level.

## Typed contract references

Use these inspected official SDK revisions to reproduce field-level checks;
fetch the exact endpoint's current documentation before changing the contract.
They are references, not dependencies or copied converters.

- [OpenAI SDK](https://github.com/openai/openai-node/blob/bb024d41522d0b5a6b6c64055471111068811ccd/src/resources/chat/completions/completions.ts):
  protocol-specific message roles and nullable Chat token-limit fields.
- [Responses SDK](https://github.com/openai/openai-node/blob/bb024d41522d0b5a6b6c64055471111068811ccd/src/resources/responses/responses.ts):
  function/custom result content lists, item versus call identity, and direct
  versus program callers. The official [async tool guide](https://developers.openai.com/api/docs/guides/async-tool-calling)
  and [programmatic tool guide](https://developers.openai.com/api/docs/guides/tools-programmatic-tool-calling)
  define execution semantics; they are not optional tracing metadata.
- [Anthropic SDK](https://github.com/anthropics/anthropic-sdk-typescript/blob/04a4c28c70de498d6d0e930e43edb5edb9ac545e/src/resources/messages/messages.ts):
  message roles, content-block identity, thinking signatures, and stop reasons.
  SDK acceptance of an inline system role does not justify hoisting that turn
  into the initial prompt on a different endpoint.
- [Google SDK](https://github.com/googleapis/js-genai/blob/177ee684ada5d2fd39450e88a9416b301eaa6ff4/src/types.ts):
  optional Content roles, function-call identity and arguments, and calling modes.
  The official [Generate Content protobuf](https://github.com/googleapis/googleapis/blob/master/google/ai/generativelanguage/v1beta/content.proto)
  also defines the Part data oneof and function-calling defaults.

## Context accounting

For context accounting use the provider's [OpenAI token-counting guide](https://developers.openai.com/api/docs/guides/token-counting),
[Claude token-counting guide](https://platform.claude.com/docs/en/build-with-claude/token-counting),
and [Gemini token guide](https://ai.google.dev/gemini-api/docs/tokens).
Loaded instructions, history, tools, and supported media contribute to input;
JSON byte counts are not interchangeable with provider token counts. Deferred
loading requires the exact client's and route's supported discovery contract,
not name-based tool filtering or fabricated hosted-tool results.

## Account transport boundary

Public OpenAI API documentation does not specify ChatGPT account or Excel / BPS
transport behavior. Describe those capabilities as observed, preserve their
native opaque state, and qualify them with the existing synthetic acceptance
matrix and separately authorized live acceptance. Never weaken a public API
contract merely because a private transport cannot represent it.

Historical third-party comparisons remain in `implementations.md` for
traceability and licensing. They do not override these public contracts.
