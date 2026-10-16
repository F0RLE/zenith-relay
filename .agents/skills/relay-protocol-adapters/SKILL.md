---
name: relay-protocol-adapters
description: Review or repair Relay conversions between Responses, Chat Completions, Anthropic Messages, and Gemini Generate Content, including tool roundtrips and opaque reasoning replay. Excludes UI and production Zenith routing.
---

# Relay protocol adapters

Preserve the conversation contract across a client API and an upstream API.
The minimum supported cycle is text, tool call, tool result, then the next
assistant response. A successful first answer does not establish compatibility.

Follow the official provider contract workflow in
[CONTRIBUTING.md](../../../CONTRIBUTING.md#api-and-protocol-references).
Use [PLANNING.md](../../../docs/project/PLANNING.md) for implemented contracts
and [ROADMAP.md](../../../docs/project/ROADMAP.md) for unfinished acceptance.

## Trace the failing contract

- Identify the client API, selected upstream endpoint, requested features,
  continuation mode, and whether the failure occurs in JSON or streaming.
  A model group supplies a preference; it does not prove endpoint support.
- Follow source resolution, request preparation, response conversion, and the
  next request containing tool results. Inspect both `contracts/source.rs`
  dispatch and the owning converter under `crates/relay-core/src/protocol/adapter`.
  All twelve cross-protocol routes use the shared typed translation pipeline;
  four matching-protocol routes bypass it and preserve native wire extensions.
- For continuation, trace `contracts/bridge.rs`, `store.rs`, and
  `translation/continuation.rs` with their callers. Distinguish restored native
  state from client-supplied history and foreign opaque payloads.
- Read the exact endpoint's [official contract](references/contracts.md) first.
  For undocumented account transports, mark observed behavior explicitly.
  Use [implementations.md](references/implementations.md) only for a targeted
  implementation comparison; record its inspected SHA and file paths if used.

## Adapt in the owning layer

Reuse Relay's existing adapter contracts, continuation store, and stream state
machines. Consolidate conflicting rules at their owner instead of adding a
third conversion engine or a repair branch keyed by a harness or model name.

Keep these distinctions explicit:

- A response item ID and its tool `call_id` are different identities. Preserve
  call/result associations, tool kind, namespace, declared name, and argument
  fragments. If names require aliases, make the mapping reversible and reject
  collisions. Do not invent a missing name or turn an orphan result into text.
- Preserve schema constraints, tool-choice semantics, and parallel-call policy.
  A conversion that cannot preserve a requested feature needs a specific error,
  not a silent switch to `auto`, disabled thinking, or a weaker schema.
- Provider ciphertext and signatures remain opaque. Decode only a
  Relay-owned carrier, if one is used, and unwrap it before native forwarding.
  Decoding a carrier is not decrypting OpenAI reasoning. Never translate a
  foreign blob into an Anthropic or Gemini signature.
- Replay the original native blocks in order. Bind saved state to the
  conversation and originating route/credential identity; bound its lifetime,
  count, and bytes. Inspect existing isolation before adding another cache.
  Keep sensitive state in Relay's existing volatile store unless persistence is
  explicitly part of the requested change.
- A documented provider sentinel for synthetic history is not a replacement
  for lost state from a real response. Do not use it automatically to hide a
  broken roundtrip.
- Distinguish completed, incomplete, and invalid streams. Preserve known usage
  after interruption and avoid duplicate output or tool calls. Continuation
  replay does not make an unknown execution safe to retry.

Use native forwarding when the selected route supports the requested contract.
For converted routes, retain the original provider state separately from the
portable text/tool representation.

## Establish acceptance

Use [acceptance.md](references/acceptance.md) for the affected protocol pair.
Inspect existing coverage first; run or extend checks only within task and
harness permissions. Use synthetic conversations and signatures, never real
prompts, credentials, or captured provider bodies.

Report the reference revisions, adopted behavior, intentional differences,
and checks actually performed. Distinguish static review, automated checks,
and live provider acceptance. Preserve source attribution and applicable
license notices when copying or adapting code.
