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

- Separate a Responses output item ID from `call_id`. Verify result matching
  after aliasing, namespace conversion, and any required provider-ID mapping.
- Cover repeated identity fields, late ID/name metadata, fragmented arguments,
  interleaved calls, and a call with no final name. A completed call cannot be
  dispatched with an invented name, conflicting identity, or malformed arguments.
- Preserve function/custom-tool semantics and schema constraints. Unsupported
  hosted tools, structured outputs, or tool-choice modes need a named error
  rather than removal or weakening.
- Require consistent event/block/item order and one terminal outcome. An EOF,
  error, or output limit is not successful completion. Preserve reported usage
  even when output is partial.
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
