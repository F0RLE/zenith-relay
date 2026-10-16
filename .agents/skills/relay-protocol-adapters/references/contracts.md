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

## Account transport boundary

Public OpenAI API documentation does not specify ChatGPT account or Excel / BPS
transport behavior. Describe those capabilities as observed, preserve their
native opaque state, and qualify them with the existing synthetic acceptance
matrix and separately authorized live acceptance. Never weaken a public API
contract merely because a private transport cannot represent it.

Historical third-party comparisons remain in `implementations.md` for
traceability and licensing. They do not override these public contracts.
