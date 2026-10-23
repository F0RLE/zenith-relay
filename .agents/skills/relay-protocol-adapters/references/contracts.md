# Official protocol contracts

Read the contract for the actual endpoint, not just the model family. These
links are starting points, not snapshots; check the current documented fields
and corresponding official SDK types before changing behavior.

| Protocol | Primary sources | Boundaries to verify |
| --- | --- | --- |
| OpenAI Responses | [API reference](https://developers.openai.com/api/reference/resources/responses/methods/create), [function calling](https://developers.openai.com/api/docs/guides/function-calling), [tool search](https://developers.openai.com/api/docs/guides/tools-tool-search), [streaming](https://developers.openai.com/api/docs/guides/streaming-responses) | Item ID versus call ID, namespaces, client versus server execution, terminal status, encrypted reasoning, continuation |
| OpenAI Chat Completions | [API reference](https://developers.openai.com/api/reference/resources/chat/subresources/completions/methods/create), [streaming events](https://developers.openai.com/api/reference/resources/chat/subresources/completions/streaming-events), [reasoning](https://developers.openai.com/api/docs/guides/reasoning) | Message/tool grouping, supported options, streamed argument fragments, requested usage chunks and finish reason |
| Anthropic Messages | [Messages](https://platform.claude.com/docs/en/api/messages/create), [streaming](https://platform.claude.com/docs/en/build-with-claude/streaming), [thinking](https://platform.claude.com/docs/en/build-with-claude/thinking), [tool use](https://platform.claude.com/docs/en/agents-and-tools/tool-use/overview) | Ordered content blocks, tool use/result IDs, signatures, stop reasons, input/output/cache usage |
| Gemini Generate Content | [API reference](https://ai.google.dev/api/generate-content), [Schema SDK type](https://googleapis.github.io/js-genai/release_docs/interfaces/types.Schema.html), [function calling](https://ai.google.dev/gemini-api/docs/function-calling), [thought signatures](https://ai.google.dev/gemini-api/docs/thought-signatures) | Candidate/part order, function call IDs, native signatures, schema dialect, finish reason and usage metadata |

Provider-compatible APIs need their own official capability and endpoint
documentation. OpenAI wire compatibility alone does not establish stateful
Responses support, tool search, structured outputs, or a reasoning level.
For native Responses sockets, also check the official
[WebSocket mode guide](https://developers.openai.com/api/docs/guides/websocket-mode).
Connection reuse is not evidence of multiplexing or safe replay after output.
The [rate-limits guide](https://developers.openai.com/api/docs/guides/rate-limits)
distinguishes request and token budgets. A TPM rejection is not by itself evidence
of context-window overflow or a request-size 413.
The [Structured Outputs guide](https://developers.openai.com/api/docs/guides/structured-outputs)
defines schema adherence and separate refusals. A command-approval request must
not silently lose its schema or acquire a prompt-only strict-mode substitute.
The [function-calling strict-mode contract](https://developers.openai.com/api/docs/guides/function-calling#strict-mode)
distinguishes omitted `strict`: Responses attempts strict normalization with a
best-effort fallback, while Chat Completions defaults to non-strict. Foreign
best-effort tools sent to Responses need explicit `strict: false`; omission is
not a protocol-neutral default. Do not rewrite their optional fields or schemas.
Gemini's [Generate Content request](https://ai.google.dev/api/generate-content#v1beta.GenerationConfig)
places temperature under `generationConfig`, not at the request root. Apply
existing model capability exclusions at that path before route admission;
the endpoint's general numeric range does not establish model-specific support.

For cache optimization, use OpenAI's [prompt caching guide](https://developers.openai.com/api/docs/guides/prompt-caching)
and [cache diagnostics](https://developers.openai.com/api/docs/guides/prompt-caching/diagnostics).
Keep stable instructions, tool definitions and ordering ahead of changing input;
measure observed cached input tokens, not presumed hits. A stable cache key is
an advisory routing hint, not conversation authorization or a guaranteed hit.
Public API retention/breakpoint support does not establish support on undocumented
account transports; do not add those fields without endpoint-specific evidence.

The Responses [streaming-event reference](https://developers.openai.com/api/reference/resources/responses/streaming-events)
defines `error` with top-level `code`, `message`, `param` and `sequence_number`.
A nested HTTP error body is not the same wire event. Relay-local BPS argument
frames are private encoding inside `run_officejs.code`, not a public provider
feature; reconstruct and validate the original client arguments before release.

The [OpenAI Python SDK retry implementation](https://github.com/openai/openai-python/blob/553703e2d3db379a83e3ead9e40436b7a66f639e/src/openai/_base_client.py)
checks `x-should-retry: false` before status-based retries. Relay adds this
advisory to buffered local errors after an upstream adapter failure: another
HTTP request would otherwise regenerate. This is SDK behavior, not a guarantee
that every client honors the header or will avoid an explicit resend.

The [Messages stop-reason guide](https://platform.claude.com/docs/en/build-with-claude/handling-stop-reasons)
distinguishes server-tool `pause_turn` from `max_tokens` and context-window
truncation. Foreign APIs have no equivalent server-loop continuation here;
reject the unsupported result instead of inventing a token-limit cause.
The [fine-grained tool-streaming guide](https://platform.claude.com/docs/en/agents-and-tools/tool-use/fine-grained-tool-streaming)
allows accumulated tool input to end mid-JSON under `max_tokens`. This is not
a completed executable call. The inspected OpenAI SDK
[function-call item](https://github.com/openai/openai-python/blob/7db9d884c33b0d5ef252e9a4b6e7ae0bbd217e5b/src/openai/types/responses/response_function_tool_call.py)
supports `status: incomplete` with string arguments, and the
[Chat function-call type](https://github.com/openai/openai-python/blob/7db9d884c33b0d5ef252e9a4b6e7ae0bbd217e5b/src/openai/types/chat/chat_completion_message_function_tool_call.py)
requires callers to validate generated argument strings before execution.
OpenAI's SDK types below define output messages as `assistant` and Chat choice
indexes as positions in the choice list; conversion requests one choice only.
Reject invalid supplied identities instead of renaming them.

Keep Gemini Generate Content Part signatures distinct from the Interactions
API's dedicated thought steps. The current
[thinking guide](https://ai.google.dev/gemini-api/docs/thinking#signatures)
explains that endpoint distinction; Generate Content uses the Part types in
the API reference and protobuf below. Native Parts are not merged or reordered
to reduce continuation size.

For response formats, the official
[OpenAI structured-output guide](https://developers.openai.com/api/docs/guides/structured-outputs)
distinguishes JSON mode from schema adherence. Do not silently drop schema or
strict controls from a format variant that cannot carry them. Gemini's
[structured-output guide](https://ai.google.dev/gemini-api/docs/structured-output)
and the Google SDK below define the MIME string and alternative schema fields;
wrong-type MIME values are not an absent setting.

## Typed contract references

### Current SDK source index

Default-branch SDK revisions and the linked source files were rechecked on
**2026-10-23**. This index refresh does not update Relay's dependencies or prove
support for every field exposed by a newer SDK. Read the exact endpoint's
official documentation and the relevant type before changing the contract.

| SDK | Rechecked revision | Rechecked source |
| --- | --- | --- |
| OpenAI TypeScript | `4c11b87c3e5e4fcddeeffc85b55502b2296f49fd` | [Responses types](https://github.com/openai/openai-node/blob/4c11b87c3e5e4fcddeeffc85b55502b2296f49fd/src/resources/responses/responses.ts) |
| OpenAI Python | `81792ff040a2ca2b70348163049d3d69d5ba95cf` | [Retry owner](https://github.com/openai/openai-python/blob/81792ff040a2ca2b70348163049d3d69d5ba95cf/src/openai/_base_client.py), [function-call type](https://github.com/openai/openai-python/blob/81792ff040a2ca2b70348163049d3d69d5ba95cf/src/openai/types/responses/response_function_tool_call.py) |
| Anthropic TypeScript | `bc9cbb0a5bce1d50f2589e556f7e81f7fcbc4aab` | [Messages types](https://github.com/anthropics/anthropic-sdk-typescript/blob/bc9cbb0a5bce1d50f2589e556f7e81f7fcbc4aab/src/resources/messages/messages.ts) |
| Google Gen AI TypeScript | `0dd787c23d76139982d7acb01e3c99978f563007` | [Generate Content types](https://github.com/googleapis/js-genai/blob/0dd787c23d76139982d7acb01e3c99978f563007/src/types.ts) |

### Earlier field-level checks

The pinned sources below preserve the revisions used for individual field-level
comparisons. They are historical evidence, not newest-version labels or copied
converters; do not replace their hashes without rechecking the associated claim.

- [Responses nullable output fields](https://github.com/openai/openai-node/blob/17e0f08e83e2381efa075b1bbe9ed5eb7b63a6e1/src/resources/responses/responses.ts):
  message `phase`, reasoning `encrypted_content`, and function/custom tool
  `caller` are optional and nullable. Completed-item comparisons treat omission
  and `null` alike only for these fields. Other values and execution state
  remain immutable after applying completed-reasoning precedence below.
- [Completed Responses reasoning](https://github.com/openai/openai-node/blob/4c11b87c3e5e4fcddeeffc85b55502b2296f49fd/src/resources/responses/responses.ts):
  streaming replay uses the reasoning item's `encrypted_content` from
  `response.output_item.done`, not the potentially incomplete added item.
  Relay retains that exact completed ciphertext if the terminal has another
  copy, before comparing its other fields; it does not decode or regenerate it.
- [Responses additional-tools input type](https://github.com/openai/openai-node/blob/e35205033764ec2a2adc7fc71f3d2df9b0ea8679/src/resources/responses/responses.ts):
  input `additional_tools` requires `role: "developer"`; the wider output-item
  role union is not the request contract. SIWC-generated tool items must retain
  this role as well as the original declarations and call identities.
- [OpenAI SDK](https://github.com/openai/openai-node/blob/e9f788ebfb1b6e2f5aa59022eee3da202110390e/src/resources/chat/completions/completions.ts):
  protocol-specific message roles, nullable Chat token-limit fields, and
  name-only function selectors versus separate custom-tool selectors. Assistant
  history supports the `refusal` field and refusal content parts; a participant
  `name` is not a function result's name or a tool call ID.
- [Responses SDK](https://github.com/openai/openai-node/blob/bb024d41522d0b5a6b6c64055471111068811ccd/src/resources/responses/responses.ts):
  function/custom result content lists, item versus call identity, and direct
  versus program callers. Function results have optional names and nullable
  call IDs; custom results require call IDs and do not define a name field.
  The official [async tool guide](https://developers.openai.com/api/docs/guides/async-tool-calling)
  and [programmatic tool guide](https://developers.openai.com/api/docs/guides/tools-programmatic-tool-calling)
  define execution semantics; they are not optional tracing metadata.
  Native BPS preserves named function outputs with missing/null `call_id`;
  only IDs belonging to its transport are rewritten. Foreign bridges still
  require a representable call/result association, and custom outputs require IDs.
- [Anthropic SDK](https://github.com/anthropics/anthropic-sdk-typescript/blob/ca50812ed769a40510bb2f9e5e232e95cdd376f1/src/resources/messages/messages.ts):
  message roles, content-block identity, thinking signatures, stop reasons, and
  variant-specific tool choices (`none` has no parallel-use modifier).
  JSON replies and streamed `message_start.message` use the assistant role.
  Claude clients also send initial system attachments after a user message.
  Before the first assistant turn, Relay collects this initial instruction text
  into a foreign target's system field, retaining instruction and user-content
  order separately. SDK acceptance of an inline system role does not justify
  hoisting it past an earlier assistant turn on a different endpoint.
- [Google SDK](https://github.com/googleapis/js-genai/blob/177ee684ada5d2fd39450e88a9416b301eaa6ff4/src/types.ts):
  optional Content roles, function-call identity and arguments, and calling modes.
  The official [Generate Content protobuf](https://github.com/googleapis/googleapis/blob/bc7e3baa28fbb223fa93782e130260fab8205bfc/google/ai/generativelanguage/v1beta/content.proto)
  also defines the Part data oneof and function-calling defaults.

## Documents, discovery and native state

For user attribution, the official [Anthropic metadata type](https://github.com/anthropics/anthropic-sdk-python/blob/bcd55fe1d3f0f1240765f8cef04295b67337f55a/src/anthropic/types/metadata_param.py)
defines `user_id` as an opaque UUID/hash usable for abuse detection, never personal
information. [Generate Content](https://ai.google.dev/api/generate-content)
documents `labels.safety_identifier` for aggregators and the 63-character,
lowercase label restriction. The inspected [Google SDK converter](https://github.com/googleapis/python-genai/blob/1d12358a1398b9a510ae8ba25dd31e746d8e9be9/google/genai/models.py)
writes labels at the request root for both Developer API and Vertex, not in
`generationConfig`. Relay maps Messages attribution to a stable 63-hex-character
SHA-256 pseudonym, without adding prompt text or treating arbitrary OpenAI
metadata as a safety identifier. Empty/null values retain their distinction.

The official [OpenCode model documentation](https://github.com/anomalyco/opencode/blob/ba06ff9e0bf2dcefbbbc3a1d6094f3572d77c84e/packages/web/src/content/docs/models.mdx)
defines built-in SDK reasoning variants, override/merge behavior, and
`variants.<name>.disabled`. Generated profiles must explicitly disable unsupported
standard efforts; an omitted entry does not prevent OpenCode adding `medium` or
another default. Keep Relay admission checks authoritative rather than silently
downgrading such a request.

Claude Desktop's official [third-party configuration](https://claude.com/docs/third-party/claude-desktop/configuration#models)
defines per-model `maxEffort` as an upper bound, not an enum of allowed efforts.
Cap at the highest executable pool-allowed Messages effort: a missing `medium`
or `xhigh` must not suppress a supported `high` or `max`. Desktop cannot express
holes in that menu through this field. As an explicit client compatibility
policy, reserved Desktop reasoning aliases on Messages map Low/Medium/High/
XHigh/Max to the nearest executable, pool-allowed lower level, or the lowest
allowed level if none is lower. This resembles the documented
`defaultModelEffort` fallback, but applying it to incoming picker selections
is Relay policy, not a provider guarantee. Resolve across configured routes
before selection, preserving exact choices even when another member lacks
them. Admission and preparation share the effective choice; usage preserves
the requested value. Off, budgets, unknown levels, Ultra, ordinary API model
IDs and other client protocols stay strict. Never infer a reasoning budget
or enable a capability from the picker alias.
OpenCode's built-in `Default` choice means no variant override, not a model
effort named `default`; disabling a named variant does not remove that client UI.

For PDF wire formats use [OpenAI PDF inputs](https://developers.openai.com/api/docs/guides/pdf-files),
[Anthropic PDF support](https://platform.claude.com/docs/en/build-with-claude/pdf-support)
and the Generate Content Part/Blob types above. Transfer PDF bytes without local
text extraction. Keep file IDs provider-owned; URL support and document
citations/rendering are endpoint-specific. Gemini Blob `displayName` is a native
label whose use depends on the provider operation, not a promised citation ID.

The [tool-search guide](https://developers.openai.com/api/docs/guides/tools-tool-search)
distinguishes `execution: client` from hosted search. Client calls carry object
arguments and paired `tool_search_output` catalogs. Preserve that native client
boundary while lowering to an upstream function; do not execute the lookup in
Relay or turn hosted services into ordinary functions. Removing a definition
from the loaded search results revokes its deferred availability; historical
calls do not load it again. An explicit current tool choice remains authoritative.

The [Generate Content reference](https://ai.google.dev/api/generate-content)
defines candidate `CONTINUATION` tokens and the request's `continuationToken`.
The subsequent `contents` must exactly repeat the original prompt/history.
Keep tokens opaque and route-owned; do not classify the pause as output-token
exhaustion or automatically generate another hop. `cachedContent` refers to a
provider cache resource, not portable prompt text or a Relay TTL guarantee.

## Context accounting

For context accounting use the provider's [OpenAI token-counting guide](https://developers.openai.com/api/docs/guides/token-counting),
[Claude token-counting guide](https://platform.claude.com/docs/en/build-with-claude/token-counting),
and [Gemini token guide](https://ai.google.dev/gemini-api/docs/tokens).
The [OpenAI image-input guide](https://developers.openai.com/api/docs/guides/images-vision)
defines model-specific `detail` modes and the omitted `auto` default for both
Responses and Chat Completions.
Loaded instructions, history, tools, and supported media contribute to input;
JSON byte counts are not interchangeable with provider token counts. Deferred
loading requires the exact client's and route's supported discovery contract,
not name-based tool filtering or fabricated hosted-tool results.

## Cache accounting and retention

For cache accounting and retention, use the exact provider's contract:
[OpenAI prompt caching](https://developers.openai.com/api/docs/guides/prompt-caching),
[Anthropic prompt caching](https://platform.claude.com/docs/en/build-with-claude/prompt-caching),
and [Gemini cached-content expiry](https://ai.google.dev/api/caching).
OpenAI's applicable 30-minute policy and Anthropic's 5-minute/1-hour policies
refresh on prefix reuse without turning reads into new writes. Gemini explicit
cache resources have their own expiry/update API; do not infer a sliding TTL
from a read counter. Retention is not a live cache inventory or a cumulative
token total. Unknown routes/windows remain unknown, and mixed prefix lifetimes
cannot be collapsed into their maximum.

## Account transport boundary

Public OpenAI API documentation does not specify ChatGPT account or Excel / BPS
transport behavior. Describe those capabilities as observed, preserve their
native opaque state, and qualify them with the existing synthetic acceptance
matrix and separately authorized live acceptance. Never weaken a public API
contract merely because a private transport cannot represent it.

Historical third-party comparisons remain in `implementations.md` for
traceability and licensing. They do not override these public contracts.
