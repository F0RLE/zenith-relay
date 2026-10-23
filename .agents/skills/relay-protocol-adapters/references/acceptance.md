# Protocol acceptance

Apply these cases to the affected client/upstream pair. Distinguish native
forwarding from conversion, and JSON from SSE. The client formats are Responses,
Chat Completions, Anthropic Messages, and Gemini Generate Content; advertising
a route is not evidence that its complete conversation cycle works.

## Minimum conversation cycle

| Case | Required observable behavior |
| --- | --- |
| Plain text | Preserve instructions, roles, text, and the request's supported generation options. |
| One function call | Return the declared tool name, stable call ID, kind/namespace, and complete arguments. |
| Tool result and next turn | Associate the result with its original call and return the next assistant response without losing provider state. |
| Two parallel calls | Keep both calls and results distinct, preserving their grouping and metadata. Respect the requested parallel-call policy. |
| Mixed text, reasoning, and calls | Preserve block order and opaque state; do not reconstruct signed blocks from a summary. |

Exercise both `previous_response_id` and full-history replay where the route
claims to support them. In the latter case, the absence of a response ID must
not silently discard state or cause duplicate historical calls.

## Tool and streaming boundaries

- Validate roles using the incoming protocol, including Gemini's omitted-role
  default. Initial system attachments after the first user message must reach
  the target's system field in instruction order, with user-content order intact,
  and survive JSON/SSE tool continuations. Do not reinterpret a foreign role or
  move an instruction past an earlier assistant turn into the initial prompt.
- Nullable or alternate token-limit fields must not erase an explicit limit.
  Conflicting non-null limits need a named error instead of an arbitrary winner.
- Gemini Parts retain one data payload. Reject conflicting content, malformed
  supplied call IDs/arguments, and unrepresentable function-name restrictions;
  preserve documented defaults for omitted calling modes and empty name sets.
- Replay Chat refusals through the next user turn, covering both the top-level
  field and content parts, duplicate/conflicting representations, and assistant
  role restrictions. Unrepresentable participant names must not disappear.
- Separate a Responses output item ID from `call_id`. Verify result matching
  after aliasing, namespace conversion, and any required provider-ID mapping.
- Cover repeated identity fields, late ID/name metadata, fragmented arguments,
  interleaved calls, and a call with no final name. A completed call cannot be
  dispatched with an invented name, conflicting identity, or malformed arguments.
- Preserve function/custom-tool semantics and schema constraints. Unsupported
  hosted tools, structured outputs, or tool-choice modes need a named error
  rather than removal or weakening.
- Cover custom results as text and typed content lists, including media and
  literal JSON-looking text. Reject unrepresentable content without removing it.
- Preserve execution semantics: unexpected async/program callers or namespaces
  cannot become direct calls during conversion, including late stream metadata.
  Matching native Responses must retain those fields without this conversion guard.
- Require consistent event/block/item order and one terminal outcome. An EOF,
  error, or output limit is not successful completion. Preserve reported usage
  even when output is partial.
- Converted Chat/Messages tool stop reasons require a call; their ordinary
  stop reasons cannot turn a tool invocation into a successful text-only turn.
  Check JSON and SSE, retaining the native routes' original wire contract.
- Required/named tool choices reject a normal text-only completion, not a valid
  refusal or output limit. Cover positive calls and negative JSON/SSE responses
  across all twelve conversions; never regenerate the response automatically.
- Removing a deferred tool from client-search results prevents another call,
  including after historical use. Keep the old call/result replayable; only
  current discovery or explicit tool choice can expose it again.
- Check that duplicate terminal events and reconnect/replay handling do not
  execute a tool or emit its result twice.
- Native JSON preserves ordered schema properties, annotated union branches,
  unknown extension objects, and arbitrary-precision numbers through actual
  parsing/preparation/serialization. Body whitespace is not an invariant.
- Native HTTP and WebSocket requests retain all permitted repeated list-header
  values, but never local credentials or Connection-nominated extensions.
  Ambiguous session/state singleton headers must not reach the upstream.
  For successful SSE/WebSocket forwarding, compare original payload bytes,
  including CRLF, comments, event/id/retry fields, text/binary frames, unknown
  valid events, exact numbers, and split UTF-8/chunk boundaries. SSE terminates
  at the first complete protocol terminal event; it does not wait for a trailing
  [DONE], transport EOF, or an optional LF after the terminal CR blank line.
- Root-session hints can share soft account affinity across child threads;
  thread contexts remain distinct. Malformed/oversized metadata must not
  override a valid fallback. Explicit cache keys remain stronger, scoped by
  local key and model. None of these hints grants opaque continuation ownership.

## Reasoning and route ownership

- PDF input: preserve base64 bytes/labels through every supported pair and
  retain PDF tool-result IDs on Responses/Messages. Keep supported HTTPS URLs
  unchanged without Relay downloads. Native file IDs, citations/context and
  rendering controls must never silently degrade during conversion.
- Client tool search: query, native client call, paired catalog result, loaded
  function call/result, then final answer in JSON/SSE. Cover namespaces, deferred
  schemas, forced choice, both continuation modes, wrong result kinds and no
  duplicate schema text in the foreign model's conversation.
- Native Gemini continuation: JSON/SSE preserve all candidate tokens and late
  usage without fabricating success, repeating a hop or warming health. Resume
  only with the recorded owner/local key/model and identical original contents;
  reject unknown, changed and expired bindings. Cache references and hosted
  service tools retain their native request fields.

| State | Required behavior |
| --- | --- |
| Native OpenAI encrypted reasoning | Return it unchanged to its originating compatible route; never reinterpret it as another provider's signature. |
| Anthropic thinking/redacted thinking | Replay the original blocks, signatures/data, and order with the associated tool turn. |
| Gemini thought signatures | Keep signatures on their original parts, including parallel groups where only one call carries a signature. |
| Relay-owned carrier | Validate its version, bounds, and ownership; unwrap it before native forwarding. |
| Missing, expired, or foreign state | Return a specific continuation/unsupported error. Do not disable reasoning or insert a dummy signature to conceal lost state. |
| Conflicting state under one key | Reject the conflict without overwriting valid state. |

Inspect replay isolation with different conversations, route destinations,
credential incarnations, models, and protocols. Cover expiry and count/byte
bounds when changing the store. Synthetic identities are enough for local
coverage.

Source capabilities and endpoint configuration determine the upstream contract.
Group preferences do not establish support for tools, reasoning, caching, or a
particular endpoint. A change of route or credential must not reuse incompatible
state. Retry policy still requires a known not-sent or pre-execution rejection.

## Evidence

Reuse the owning checks from CONTRIBUTING within current permissions. Record
which cases were reviewed or run and which remain unknown. Compilation,
fixture checks, and live provider/harness acceptance are different evidence.
Keep raw prompts, arguments, signatures, and provider bodies out of diagnostics.
