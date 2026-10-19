# Zenith Relay architecture

Current implementation contracts. Code and focused tests define exact behavior.
Use [ROADMAP.md](ROADMAP.md) for open work, [CONTRIBUTING.md](../../CONTRIBUTING.md)
for development checks, and [Help](../help/en/README.md) for user steps.

## Scope and ownership

Relay is a local-first desktop application and a single-owner, user-managed
server. It is separate from production Zenith Gateway and Control. Production
credentials, customer data, inventory, billing, and routing logic do not belong
here. [Hosted Relay](HOSTED_RELAY_ARCHITECTURE.md) is a proposal, not a shipped mode.

| Area | Owns |
| --- | --- |
| `src/src` | React UI, i18n, snapshots, typed Tauri calls |
| `src-tauri/src` | Desktop I/O, credentials, OAuth, profiles, process lifecycle |
| `crates/relay-core` | Shared discovery, routing, protocols, quota, scheduling, usage |
| `relay-server` | Single-owner management API, SQLite, encrypted vault, persistence |

The renderer does not read secrets, call providers, or decide routing. Desktop
and server use the same core. Closing the desktop window leaves the local
process running; explicitly quitting stops it. A remote server runs independently.

## Core layout

Paths in this table are under [relay-core/src](../../crates/relay-core/src).

| Module | Responsibility |
| --- | --- |
| `gateway/` | HTTP/WebSocket ingress, body limits, request execution, streaming, error responses |
| `runtime/` | Configured sources and keys, authorization, candidate leases, model projection, session state |
| `scheduler/` | Capacity, selection, affinity, cooldowns, refresh and rotation policy |
| `protocol/` | Wire contracts, native forwarding, typed codecs, terminal validation, replay |
| `accounts/`, `providers/` | Token authority and provider-specific account transport, including ChatGPT/BPS |
| `sources/` | API-source endpoint configuration, discovery, service profiles, statistics |
| `model_metadata/`, `catalog/` | Reference identity, capabilities, native-protocol and display policies |
| `quota/` | Provider quota observations and their freshness |
| `usage/`, `pricing/` | Request/attempt telemetry and API-equivalent estimates |
| `automations/`, `request_timezone/`, `tool_policy/` | Shared explicit background and request policies |

A pooled request follows this lifecycle:

1. Decode the client's contract; authenticate its request key and scope.
2. Resolve the real model, continuation owner, eligible routes, and admission.
3. Lease one physical member; prepare credentials, policy, and wire request.
4. Execute through the selected transport within the logical request's budget.
5. Validate the terminal outcome and retain known usage, including failures.
6. Release capacity; publish usage and only valid reusable continuation state.

The owning execution paths are `gateway/execution/request/`,
`gateway/execution/account/`, and `gateway/websocket/`. They share runtime,
protocol, authorization, and scheduler policy; transport-specific orchestration
does not grant a different retry or ownership contract.

## Desktop workspace

Data pages use the workspace beside the sidebar. Card tracks remain compact;
settings, dialogs, and reading content have bounded readable widths. User-facing
labels and workflows belong in localized Help, not this architecture document.
Sidebar auto-collapse follows viewport changes until the user explicitly chooses
a sidebar state for the current session.

Startup paints a themed, localized HTML loader before importing the React UI.
It reports interface/data loading, not invented progress percentages. The shell
is revealed after a successful initial snapshot and paint; slow or failed startup
offers a window reload without clearing settings. Render failures reveal the
existing error boundary. Secondary pages load on navigation intent, not all at
startup. Existing first-frame and interactive metrics track renderer readiness,
not provider or gateway health.

## State and secret boundaries

- Credential access belongs to the desktop credential store or the server's
  encrypted vault. Database, recovery, export, cache, and log areas are separate.
- Snapshots, usage, and diagnostics are redacted. Account exports are explicit
  credential-bearing documents, not support bundles.
- Management tokens and inference request keys are different credentials.
- Server migrations are append-only. Backup/restore validates encrypted
  references before activation.
- Runtime replacement fences delayed dispatches. Already dispatched streams can
  finish; a save acknowledges durable storage and runtime publication, not a
  later client-catalog refresh. Late UI reads cannot undo a successful command.

## Connections, discovery, and metadata

Connections stores accounts, API sources, and proxies. Saving a connection does
not enable pool membership. Access owns listener/key settings and account
request policies; Pool owns membership and per-model rules.

ChatGPT has separate native (`codex`) and Excel/Basis Points (`excel_bps`)
OAuth clients. They remain separate connections even for the same principal.
Refresh, reauthentication, import, export, and transfer preserve the issuing
client. Legacy credentials without a client hint remain Codex; captured BPS
headers do not establish an OAuth client.

Native ChatGPT accounts support pool use and direct launch. BPS is pool-only;
it does not write native Codex authentication or provide Agent Identity. Known
Free BPS connections cannot be imported, discovered, or dispatched. Native Free
accounts remain eligible under normal admission. Unknown plans do not prove access.

Discovery preserves inventory separately from executable routes. A model name,
`/models` row, group label, or reference catalog does not prove endpoint access.
Normal refresh does not send generation probes. Endpoint declarations retain
their origin; address/key changes invalidate old evidence.

Reference metadata supplies model identity, limits, modalities, tools, reasoning,
service tiers, and native-protocol policy. Declared, confirmed, unsupported, and
unknown evidence stay distinct. Missing limits/capabilities remain unknown;
participant metadata does not become universal model truth.

Known model identities use the native upstream selected by
`model_metadata/protocol_policy.rs` and `catalog/protocol-policy.json`:
OpenAI Responses, Anthropic Messages, Google Gemini, and xAI/Z.ai Chat Completions.
An incompatible endpoint or pin leaves no route; the client's protocol does not
override that requirement. Unknown identities use their exact endpoint evidence
and configured preferences/fallback. Several protocol bindings still share one
physical member, quota, and capacity.

Display identity never rewrites route IDs. Exact qualified IDs precede a unique
leaf; ambiguous leaves stay separate. [Catalog policy](../../catalog/README.md)
owns display order and public export. Saved manual order overrides the default;
inventory order and member rotation order are independent.

Standard is the baseline speed. Fast/Ultrafast require model and transport
support; explicit source tiers may narrow the set. BPS carries Standard only.
The pool default is captured once per logical request, explicit client tiers
are preserved, and temporary health does not rewrite configured choices.

## Statistics and refresh

- Balance, key allowance, subscription allowance, quota windows, and provider
  credits retain their own units and observation times. Unknown is not zero;
  failed reads preserve the last value as stale. Statistics alone grant no route.
- Model, balance, quota, and usage refreshes are independent resource-scoped
  jobs. Readers join in-flight work. Manual refresh does not cancel shared work.
  Configuration/credential/delete revisions fence late results.
- Passive window headers do not refresh credit-ledger evidence. A reset time
  without a remainder does not refresh that remainder. Traffic cannot postpone
  a required full poll indefinitely; stale credit-only admission expires.
- Shared credit totals count one provider ledger once, using its newest valid
  observation, not emails or the newest unrelated header timestamp.
- Service profiles own endpoint roots and supported statistics readers. An
  unsupported billing reader does not probe unrelated APIs or block inference.
- Native model discovery has a bounded timeout and one paced transient-read
  retry inside the same refresh job. HTTP rejection, malformed input, permission
  failure, and provider Retry-After follow their own policies.
- Token refresh/invalidation is revision-bound. A late 401 or invalid grant
  cannot overwrite a newer login, delete replacement credentials, restore a
  deleted account, or change operator disable/drain/membership choices.
- Account trash retains credentials and usage but excludes the registration
  from routing, monitoring, exports, and presets. Restore returns it disabled
  and outside the pool with a new revision. Permanent deletion is explicit.
  Server trash requires `account_trash_v1`.

## Pool and execution

Automatic selection first filters compatible, enabled, available members, then
compares normalized load. Fresh positive quota takes precedence; candidates
less than five percentage points below the best fresh quota remain in the same
selection band. At a lead of five points or more the lower remainder yields.
Fresh provider credits are the fallback. Soft health/first-output latency and request weights
resolve remaining choices. Wallet prices do not rank members. Missing latency
does not exclude a member. Soft affinity can yield to a better eligible member;
opaque response ownership cannot.

Automatic response-owner yield probes the same admission policy with the current
physical owner preferred on ties. Load, fresh quota, independent credit evidence,
and soft health therefore cannot disagree with ordinary request selection.
A recovery trial or another protocol alias is not a reason to move ownership.
The executor must still materialize reusable history before clearing an opaque
binding; active generations and unknown execution are never moved or replayed.

Runtime snapshots publish `resourceHeadroom` from the same quota and credit
freshness checks used by admission, including protected quota reserves. Desktop
cards use this evidence instead of treating refresh-job status or saved counters
as routing capacity. A provider ledger must explicitly be usable; its reported
balance alone is not spendable capacity. Cards rank resource headroom, not a
model-independent execution queue. The read-only next-member preview is absent
when eligible model/protocol routes disagree and never advances rotation.

Round-robin mode follows saved member order, skips unavailable/full members,
and wraps. Equal cycling ignores saved weights and soft affinity. Optional
weighted cycling uses physical-member request shares; shares are not token or
spending targets. Equal-credit ties use saved order, not protocol-alias IDs.
Failover selects the first ready member in saved priority order, without cycling.
A due lower-priority recovery route does not displace a healthy primary; a due
primary recovery still follows the bounded recovery admission policy.
A member limit covers all its protocol routes; zero means no extra member cap,
not unlimited provider capacity. Policy saves are serialized and identity-checked.

Management protocol three rejects older clients before exposing new saved
strategy variants; current clients can still negotiate protocol-two servers.
Rotation policy version three is advertised as `rotation_v3`. Known version-one
and version-two policies upgrade without changing inventory, member order,
weights, concurrency limits, or their equal-cycling semantics. Stored `in_order`
and `round_robin` remain equal cycling; `weighted_round_robin` explicitly opts
into shares, and `failover` explicitly opts into primary/reserve behavior.
Unknown versions and invalid legacy pairs are rejected, not normalized.
The editor may still update version-two servers through `rotation_v2`, but
never offers them the new strategy variants or presets.
Portable preset imports may migrate validated legacy policies on newer servers;
direct updates stay version-exact, and new policies are never downgraded for
older servers.

Presets restore equal request shares and no extra member concurrency limits.
Strategy-only changes preserve those controls. Recommended and even-distribution
preserve member order. Accounts-first places accounts before API sources while
preserving relative order inside each group; its API reserve may spend balance.
Presets rebase onto current membership and never restore a removed member.
Policy-only updates retain active leases, continuation owners, resource evidence,
and cooldowns. No application or gateway restart is required.

Runtime strategies use explicit names: Automatic, RoundRobin,
WeightedRoundRobin, and Failover. Legacy saved `in_order` maps to equal
RoundRobin without changing the stored policy contract. Round-robin cursors
survive member changes while their route remains configured; removing the last
route discards its cursor, including during model-inventory replacement.

Admission queues, waits, and generation attempts are bounded. One logical
request owns its dispatch, transport, auth-repair, retry, and irreversible-send
budgets across HTTP, SSE, WebSocket, and account paths. Retries require:

- a proven not-sent outcome or pre-execution rejection;
- repeatable input and valid continuation ownership;
- remaining request budget and no client-visible response bytes;
- provider Retry-After/reset deadlines respected without shortening them.

An uncertain or completed generation is not safe to repeat. Local saturation
does not vote against provider health. Request-preparation incompatibilities
stay excluded for that logical request without consuming a generation or
cooling the member. Recovery cannot loop over the same unchanged invalid route.
Quota, cooldown, health, access, and per-model blocks remain separate states.

## Continuations, errors, and redaction

An opaque continuation retains its physical owner unless validated local replay
materializes the complete history before dispatch. A complete portable call/result
pair without an opaque predecessor needs only soft affinity. Partial histories,
unpaired results, unresolved references, or foreign ciphertext cannot release
ownership. Unknown response references return
`409 response_continuation_unavailable`. Compaction checkpoints explicitly
replace the predecessor; they are not discarded to force a retry.

Native ChatGPT turn state is scoped to the key, session, account, model, endpoint,
credential, and authorization incarnation. Client-supplied state cannot establish
that provenance. Soft root/thread and prompt-cache hints are separately bounded
and namespaced; they neither authorize state nor transfer an opaque owner.

A generic HTTP 400 is terminal, not evidence for rotation or history repair.
Buffered local errors derived from upstream adapter output carry
`x-should-retry: false` without changing their status or accounting, so clients
that honor this header do not regenerate automatically. This cannot prevent
an explicit resend or retries by clients that ignore the header.
Repair needs a specific validated response-reference/tool-link rejection.
`turn_rate_limit_exceeded` ends the current turn with the known retry delay,
without account-wide cooldown or automatic rotation.

Errors retain origin (`Relay`, `Account`, `Provider`), category, status, timings,
and safe provider IDs. Prompts, raw bodies, arguments, credentials, and
authorization headers do not enter diagnostics. Stable codes live in
`error_codes.rs` and both localized Help tables.
Adapter diagnostics retain static field names, value types and object-field counts,
never arbitrary payload keys or values. Known output-limit/filter terminals remain
incomplete responses with their usage, even when no visible output was generated.

## Protocol adapters

### Shared conversion architecture

Four wire contracts: Responses, Chat Completions, Anthropic Messages, and Gemini
Generate Content. Matching contracts use native forwarding. The twelve converted
directions use one typed pipeline:

`client codec → translation/model.rs → upstream codec`, with the inverse return path.

Dispatch is in `protocol/adapter/contracts/source.rs`; preparation distinguishes
`PreparedAdapterRequest::Native` from `Translated`. Portable content is separate
from opaque provider state in `AdapterContinuationState` and the bounded volatile
`AdapterContinuationStore`. No pair-specific runtime or second replay cache is needed.

Native JSON retains object order and arbitrary-precision numbers, not original
whitespace/escaping. Eligible repeated headers retain their bytes; credentials,
hop-by-hop fields, and ambiguous state headers are excluded. Native SSE/WS frames
retain wire bytes except explicit terminal validation, bounded-frame rejection,
and safe error rewriting. The first valid terminal ends consumption without
waiting for EOF or trailing data.

SSE parsing borrows single-line data payloads and consumes coalesced adapter,
upstream-usage, and HTTP-to-WebSocket frames in place, moving the incomplete
tail once per chunk. Native Responses forwarding uses the validated delta fast
path, including escaped text and tool arguments; usage, model identity, terminal
events, and opaque compaction stay on their full validation paths. These local
optimizations do not change service tiers, reasoning, or client cancellation.
Native WebSocket forwarding retains ordinary text/binary buffers when no error
rewrite is needed. Committed output bypasses inapplicable retry parsing, not
served-model checks. HTTP-to-WebSocket fallback consumes parsed JSON and opaque
payloads without cloning them or decoding a serialized copy. Converted streams
retain unchanged text, reasoning, and refusal snapshots; closed Messages blocks
still reject mutations.
The ChatGPT WebSocket setting controls client ingress and the managed profile,
not provider transport. Catalog changes do not override it. BPS and converted
routes use the HTTP executor behind the same client socket; usage records the
client transport. A failed client restart does not roll back a saved setting.
Messages streams finalize native text/thinking snapshots when their block closes
and append signature fragments in place. Portable deltas still stream immediately;
only a fully closed response can supply replayable native continuation state.

Converted Chat Completions streams validate `stream_options.include_usage`.
Only an explicit `true` adds `usage: null` to ordinary chunks and a final
usage-only chunk. Upstream usage collection is independent of this client flag.
Native usage capture ignores data after its terminal marker, including later
usage, tier, and model changes. Gemini candidate endings do not end capture:
its separate trailing usage remains observable until the stream ends.
Chat and Messages tool choices are validated using their own wire shapes and
per-variant fields. Messages `none` carries no parallel-use modifier: no tools
can execute, so a separate parallel restriction is redundant. Native forwarding
keeps its existing extension policy; Responses selectors use their own resolver.
Responses custom-input deltas and missing suffixes in final text snapshots append
in place after the existing identity, prefix, and closed-block checks.

Completed streams move existing history and native continuation state into the
saved checkpoint without cloning the entire prepared request. Validation and
response encoding precede the transfer; final client emission keeps its checks.
Gemini native Parts retain their original order and signature/thought boundaries.

Responses stream interruptions emit one explicit `response.failed` at a safe
SSE boundary, even after visible output. An already forwarded partial frame
cannot be replaced or have another event inserted into it. Interrupted streams
retain known usage, never fabricate `response.completed`, and do not authorize
replaying an execution with unknown effects.
Transport diagnostics retain fixed timeout/body/decode and I/O categories, not
error strings, URLs, credentials, or provider payloads.

Every meaningful converted field is represented, handled locally under a stated
contract, or rejected. Conversion does not invent capabilities, weaken a schema,
disable reasoning, or turn unsupported controls into prompt text. Official
references and the [conversation acceptance matrix](../../.agents/skills/relay-protocol-adapters/references/acceptance.md)
own field-level review; third-party proxies are comparisons, not specifications.

### Conversation and routing contracts

- Messages `metadata.user_id` is opaque user attribution, not prompt content.
  Gemini conversions send its stable SHA-256 pseudonym (first 63 hexadecimal
  characters) as the documented `labels.safety_identifier`; null remains absent
  and an empty ID remains empty. Native requests stay unchanged. Arbitrary
  OpenAI metadata is not safety attribution and still requires a compatible
  endpoint rather than being discarded or relabeled.
- Responses client-executed tool search keeps its call/result kind, IDs,
  namespaces and original catalog across JSON/SSE and both continuation modes.
  Foreign models receive a callable search function; the client still executes
  the lookup and receives native `tool_search_call` items. Deferred functions
  become callable only after discovery or an explicit choice. Historical calls
  remain replayable but cannot reactivate a tool removed from discovery results.
  Exact schemas stay in the catalog/checkpoint; foreign search results carry
  loaded names rather than a second schema copy. Provider-hosted search, MCP,
  file search and code execution remain native service features, not fake
  client functions.
- Base64 PDF inputs preserve bytes and labels across the twelve conversions.
  Responses/Messages also exchange HTTPS PDF URLs without Relay fetching them
  and preserve PDF tool results. Chat/Gemini URL conversion, provider file IDs,
  document citations/context and non-default rendering controls require a
  compatible native endpoint rather than lossy extraction or invented metadata.
- Native Gemini `CONTINUATION` is an incomplete generation with a required opaque
  candidate token. JSON/SSE retain it and trailing usage. Its route owner is
  bound through the existing affinity store to the local key, model and exact
  original `contents`; unknown/changed contexts cannot rotate to another owner.
  Relay never resumes a generation automatically. `cachedContent` stays native
  and provider-owned; cache creation/expiry are not emulated by conversion.
- Messages `pause_turn` means an unfinished server-tool loop, not token
  exhaustion. Converted routes reject it with `stop_reason` rather than invent
  a token-limit result; matching native routes keep their continuation contract.
- Converted JSON responses reject supplied non-assistant Responses output roles
  and invalid/nonzero Chat choice indexes instead of relabeling their identity.
- Converted Messages replies validate supplied assistant roles in JSON and SSE.
  Chat history accepts both the `refusal` field and refusal content parts,
  preserving the text once and rejecting conflicts or non-assistant refusals.
  Responses-to-Chat history retains refusal parts instead of ordinary text.
- Historical Chat `function` results pair by function name, not an invented call
  ID. Ordinary Chat participant names have no lossless mapping here and fail
  with `messages[].name`; matching native requests retain them unchanged.
- Converted Chat/Messages terminal reasons must agree with their calls: a tool
  stop needs a tool call, while ordinary completion cannot contain one. The
  shared JSON/SSE completion boundary rejects contradictions with `finish_reason`
  before publishing a successful terminal or reusable continuation.
- Required or named tool choices need an actual call before a converted reply
  can complete normally. A plain-text success is rejected with `tool_choice`;
  refusals, content filtering and token-limit outcomes retain their own meaning.
- Preserve instruction/block order and protocol-specific roles. For targets
  with a top-level system field, initial system attachments before the first
  assistant turn join that field in instruction order; user content order stays
  unchanged. Do not hoist instructions past an earlier assistant turn.
  Nullable aliases cannot erase explicit limits;
  conflicting non-null limits fail before transport.
- Preserve call kind, namespace, declared name, item ID versus `call_id`,
  parallel grouping, arguments, and results. Only argument deltas concatenate;
  repeated identity fields do not. Tool names retain their declared identity;
  new calls require distinct nonblank call IDs, preserved without normalization.
  The shared conversion boundary rejects whitespace-only IDs in history, JSON,
  and streams. Malformed Chat call lists, mixed legacy/current call fields, and
  calls attached to tool-result messages fail instead of being silently discarded.
  Fresh calls to the same tool, including repeated stateful observations, are
  not suppressed by name or argument equality; replaying an old call is distinct.
- An explicit client serial-call policy remains binding. Native Responses Lite
  defaults an omitted policy to serial only after route selection and rejects
  an explicit parallel policy before dispatch; its marker does not serialize
  BPS or API-source calls. Ordinary tool support does not establish parallel
  support. Native catalog transport exclusions and reference exclusions win.
- Responses custom outputs accept strings and typed content lists, including
  representable images. Empty lists mean no content, not literal `[]`. Untyped
  custom results fail. Chat tool messages cannot carry images.
- Responses function-result names, when supplied, must match their recorded call; a
  name or item ID cannot replace `call_id`. Standalone outputs without a call
  have no foreign tool-history mapping and fail with `input.call_id`; matching
  native Responses input is preserved, not converted to invented calls or text.
- Equivalent `caller: direct` / `async: false` history is accepted. Async/program
  execution and an unexpected upstream namespace cannot silently become direct
  calls. Native Responses retains those fields unchanged.
- Schemas retain constraints. Gemini's OpenAPI and JSON Schema alternatives are
  mutually exclusive; Part payloads and supplied IDs/arguments are validated.
  Unsupported ordering/name restrictions fail instead of disappearing.
- Missing reasoning settings keep provider defaults. Effort/budget rendering
  uses model evidence; a budget-based model needs a budget. Messages requires an
  output limit: an omitted limit defaults to at most 8,192, capped by a known
  model limit. Explicit incompatible limits fail rather than being rewritten.
- Provider ciphertext, Messages signatures/redacted blocks, and Gemini thought
  signatures are replayed unchanged only to their compatible owner. Public
  summaries are not signatures. Missing opaque state cannot be reconstructed
  from visible reasoning or an invented sentinel.
- Image history fingerprints retain non-default `detail`. Omitted detail and
  `auto` share the documented default; other changes cannot reuse an older
  native checkpoint. Wrong detail types and misplaced controls fail before
  conversion instead of silently becoming the default.
- Output formats validate fields by variant. Text/JSON-object modes cannot
  silently discard schema controls; nested and flat schema envelopes cannot
  compete. Invalid schema names and Gemini MIME types fail before conversion.
  Absent/null optional controls retain their defaults. Messages/Gemini encoding
  does not build an unused OpenAI-format copy of the response schema.
- Chat plaintext `reasoning_content` / `reasoning` aliases must agree. Messages
  clients may replay translated summaries with absent/null/empty signatures;
  nonempty foreign signatures fail. A new ordinary user turn can carry completed
  portable history, not an unfinished foreign tool loop.
- Preserve supported URL citations and Unicode offsets. Unsupported citations
  or meaningful output extensions fail rather than disappearing.
- Metadata and advisory cache hints follow the target contract. Valid foreign
  cache hints do not promise equivalent TTLs, hits, or pricing. Required context
  edits and unsupported hosted tools are not advisory and must fail.
- JSON, gzip, and zstd share bounded request decoding. Completed, Incomplete,
  and Invalid remain separate across JSON and streaming. SSE/WS lifecycle and
  terminal status must agree; EOF or bare `[DONE]` is not proof of completion.
- Incomplete/refused/invalid responses retain known usage but do not establish
  reusable converted continuation or successful route health. Temporary native
  WS continuation is limited to the same live connection and owner.

Gemini conversion supports one candidate and drains late usage after a finish
reason under a deadline. The native WS bridge supports one response and at most
one named lane per connection; full multi-lane multiplexing remains open.
HTTP-only transport observations are bounded, expiring, and credential/route-scoped.

### Excel / Basis Points

BPS is a separate account executor, not the public Responses API. Its behavior
is observed and regression-tested, not guaranteed by public OpenAI documentation.

- Permission discovery is token-incarnation-scoped, coalesced, bounded, and
  cached briefly. A changed token or failed access read cannot reuse old access.
  Restricted models and account policy constrain execution, not just display.
- Before rejecting an unknown WebSocket model, refresh scoped BPS access and
  resolve it again. This applies to initial frames and later turns, including
  HTTP fallback; known routes and malformed frames need no discovery I/O.
  BPS-only models use their HTTP executor without a native Responses upgrade.
- Compact messages, attribution, tool history, and inline attachments are
  normalized at this boundary. Collaboration context is lower-authority user
  context; encrypted agent messages and remote image fetching are unsupported.
- Metadata sent upstream contains only Relay's task/turn/iteration fields.
  Client metadata stays local and is restored in the translated response.
  Turn fingerprints borrow the relevant history prefix; JSON hashing streams
  through a bounded buffer shared with cross-protocol continuation keys. Their
  serialized meaning and identifiers stay unchanged, without extra full-history
  clones or serialized copies.
- Ordinary text streams incrementally. Executable tools are withheld until the
  authoritative terminal validates identity, arguments, and original schemas.
  Failed/incomplete/truncated output releases no tool subset and triggers no
  regeneration. Synthetic client events are not timing or usage evidence.
  JSON and SSE retain the same tool-validation category and safe cause: malformed
  JSON, a non-object argument, and a schema mismatch remain distinguishable from
  a broken stream. Argument contents are never included in these diagnostics.
  Inner JSON syntax errors and premature EOF have separate safe parameter
  suffixes and static causes; both survive JSON and streamed error reporting.
  Their diagnostics retain only an allowlisted parser reason, payload-shape
  category, byte length, and failing decoder line/column (after any supported
  whitespace normalization). No key, value, source excerpt, or raw parser error
  enters diagnostics. Buffered and streaming discovery preserve these fields
  in both the client error and request usage record, without regeneration.
  Completed client calls cannot reuse a call ID already present in supplied
  history, in compact/full mode as well as local discovery.
- Complex function string arguments may use Relay's explicit private frame:
  `relay-function-string-v1 {"field":"cmd","args":{}}`, one newline, then
  the unchanged string value. Other arguments live in the JSON header. The
  reconstructed object still passes the original schema and tool-choice checks;
  duplicate fields and malformed headers fail. Ordinary JSON calls remain
  supported and custom tools stay opaque, even if their text resembles a frame.
  Replayed client function calls and generated examples share this encoder:
  the top-level string needing the most JSON escapes becomes the raw frame body;
  other arguments retain their types in its header. Plain arguments stay JSON.
  This removes one JSON-escaping layer, not context or model reasoning.
  For compatibility, the inner function JSON decoder also escapes literal LF,
  CR, and tab characters inside quoted strings without changing their decoded
  bytes. Unknown printable string escapes such as `\s`, `\q`, or `\'` retain
  their literal backslash instead of invalidating the call. Standard JSON
  escapes keep their existing meaning; malformed Unicode, escaped control
  characters, quotes, incomplete objects, and schema violations still fail.
  Custom input and explicit frame bodies remain opaque. This compatibility
  grammar is private to BPS, not a change to client JSON or the public API.
- The legacy single-call `name(JSON)` compatibility parser also handles
  referenced function calls. The parsed name must exactly match the one
  reference; normal schemas and tool choice still apply. No code is evaluated,
  no tool identity is inferred, and custom input is never parsed this way.
- Local BPS stream failures use the Responses `error` event's top-level
  `code`, `message`, `param` and ordered `sequence_number`. They do not fabricate
  a successful completion or reveal tool arguments. Text still streams before
  final tool validation; transport framing is not a provider latency guarantee.
- Catalogs use compact schema presentation without changing the original
  validator. Complex/unknown JSON Schema constructs remain complete JSON.
  Automatic Relay-local discovery applies from 24 tools and 24 KiB of complete
  declarations (including descriptions, schemas, and custom formats):
  the initial JSON index groups names by their declared kind and namespace;
  qualified names reconstruct exactly, without aliases or inferred namespaces.
  Lookup searches the original names, descriptions, and schema text and returns
  the complete trusted definitions, including usage rules; descriptions and
  argument names are not repeated for every unloaded tool. Previously
  called tools start loaded with lossless compact schema presentation after
  history, without rewriting the stable index prefix. The notation legend is
  sent only with schema presentation, not with an index-only request. Lookup
  still returns the original full definitions. Client instructions, history,
  reasoning effort, and original validators are not pruned or reduced.
- Discovery is a request-local read-only operation, not hosted `tool_search`.
  It allows at most eight lookup rounds, eight tools per lookup, 4 MiB retained
  input added by discovery, and 16 MiB per response. Admitted conversation
  history is not charged again against the discovery continuation limit.
  Every generation consumes the original dispatch budget and retains its
  OAuth incarnation/route/lease; no
  background task, owner change, paid probe, or post-uncertainty replay exists.
  A budget with fewer than two remaining sends selects compact mode before
  the first generation. Repeated queries return the same trusted definitions
  with a cached marker under the original budget, not an immediate error.
  Invalid arguments to a recognized local catalog lookup return a bounded
  tool error for correction under the same budget and owner; they do not
  authorize replay or repair of executable client calls.
  A mixed completed response consumes local lookups and returns its validated
  client calls without another generation; it never executes those tools inside
  Relay or clones schemas that will not be sent. Those lookups do not count as
  rendered/loaded contracts. Completed calls cannot reuse an identity from
  supplied history or an earlier lookup round. Incomplete answers retain their
  status, visible text, and reported usage without releasing partial calls or
  starting another generation. Exhaustion and malformed completed calls fail.
- `relay_bps_tool_catalog` on Responses requests selects `auto` (default),
  `compact`, `full`, or `discovery`; it never reaches the BPS endpoint. Converted
  clients use the same automatic policy after their existing typed conversion.
  Native routes keep their own tool-loading semantics unchanged.
- Discovery preserves upstream SSE for streaming inference clients. Ordinary
  text streams before terminal validation; executable calls remain withheld.
  After a withheld tool, later output whose index may shift waits for the
  terminal response. Local lookups continue inside the same response body,
  preserving one lifecycle and output order across generations. Explicit local
  output validation and upstream JSON fallbacks remain buffered; synthetic SSE
  is not first/last-token timing evidence. Lookup attempts record actual usage
  separately. Final response usage remains the last generation's counters;
  catalog diagnostics report additional lookup input/output without counting it
  twice. Schema bytes count compact preloads and every full lookup, including
  repeated definitions. Intermediate visible text and reasoning are preserved;
  native internal items replay only inside that request, not in a cross-request
  discovery store. Dropping the response drops discovery and its upstream body;
  no detached generation survives client cancellation.
- Opaque `previous_response_id`, non-Standard tiers, native strict output,
  native web search/image generation, and Agent Identity are unsupported.
  Full supplied history reconstructs the tool catalog; there is no BPS session store.
- Explicit `relay_bps_output_validation` buffers output and validates JSON or a
  self-contained offline schema locally. It is not constrained decoding;
  `strict: true`, invalid/incomplete output, and external references fail.
- Generation 401 invalidates only its rejected token incarnation and returns
  without another generation. The next request prepares fresh authorization.
  Account-wide 429 and model-access blocks retain separate scopes/deadlines.
  A BPS 422 invalid-body rejection is not a retryable server failure.

## Request timezone

Timezone rewriting uses account policy, validated as IANA zones in Rust.
Absent an explicit account choice, the assigned proxy's timezone is automatic;
an account disable or explicit account source still wins. Proxy-sourced policy
does not fall back to an account zone. Host-local bindings are not presets.

Desktop proxy import and opening storage automatically check unlocated proxies once;
manual checks refresh an existing result. The bounded HTTPS lookup uses
`https://ipwho.is/?fields=success,ip,country_code,timezone.id`, following its
[official field contract](https://ipwhois.io/documentation), through the selected
proxy without account credentials, redirects, or a direct-network fallback.
Only a valid returned IANA identifier is stored, never a guess from a country,
IP prefix, or numeric UTC offset. Geolocation failure can fall back to the
existing Cloudflare connectivity trace inside the same 12-second budget; that
trace does not invent a timezone. A failed check retains the known zone; a
confirmed changed exit without a valid zone clears stale state. Proxy identity
and check timestamps fence stale results. Persistence precedes a hot update of
the running bound accounts, including common-proxy inheritance; the scheduler,
listener, and applications are not restarted. There is no polling loop.

Only existing unambiguous timezone fields in user `environment_context` and
web-search location fields change. Ordinary text, dates, non-user messages,
missing fields, OS settings, and API-source bodies stay unchanged. Every attempt
uses its selected account's policy. Updates are revision-checked and do not
rebuild the scheduler. Remote support requires `request_timezone_v1`.

## Tool catalog

Native routes forward the client's catalog without local relevance search,
hidden tools, or legacy optimization flags. Catalog telemetry grants no permission.

Tool-search diagnostics reflect an explicitly supplied search control together
with deferred definitions, not an inferred provider capability. A text answer
without a tool call is normal for `auto` or `none`, not a missing-tool warning.
BPS renders shared invocation/schema guidance once and indexes merged tool
definitions without changing current-over-history precedence or schema contents.
Its compact/discovery presentation and bounds are owned by the BPS executor
contract above, not by native-route relevance filtering.

Converted Responses catalogs merge root declarations with loaded historical
definitions; current roots win. Nested namespaces have reversible opaque aliases
and retain descriptions. Custom tools use one `input` string. Unsupported hosted
or client-search execution fails explicitly. Only direct client execution is
representable; empty/non-direct caller policies are not widened.

Forced/allowed tool choices cannot enable undeclared tools or broaden a restricted
set. Choices reset per request; a continuation omitting the catalog retains its
definitions. Historical call kind/namespace remains independent of later catalogs.

Messages client-owned discovery validates the complete catalog, then exposes
non-deferred, explicitly referenced, historically called, or forced tools.
References must resolve to root definitions in successful paired user results.
Relay supplies exact schemas once and preserves the discovery marker; it does
not search, execute tools, emulate hosted search, or promise foreign cache placement.

Parallel calls stay in one assistant turn. Every result must match an outstanding
call before ordinary user text or the next model turn. Unknown/duplicate/missing
results fail. A disabled parallel policy rejects a second generated call, including
on protocols where this must be enforced locally.

Tool results preserve text, supported media, and error meaning. Gemini IDs are
mapped separately from client IDs, and signatures remain on original Parts.
Custom input is unwrapped once; exact integral function arguments may be rendered
as integer syntax without changing fractional, string, negative-zero, or large values.

## Usage and prices

New request diagnostics retain only JSON byte counts for client and prepared
instructions, conversation, tool catalog, and whole body, across all four APIs.
These are not token estimates and do not include provider-injected context.
BPS's final prepared instructions include its embedded catalog; recording the
intermediate converted catalog alone is not evidence of final wire size.
Cached tokens still occupy context; never subtract them from the displayed
window or remove valid instructions/tools merely to reduce the counter.

Usage records one final row per logical request and separate retained physical
attempts. Exact `requestId` history is not an additional cost/tokens total.
Deleting/expiring usage removes corresponding attempts; missing earlier history
is not reconstructed. Actual retry/stop decisions are recorded when known.
An interim successful-HTTP `tool_discovery` observation without error metadata
is pending, not a failed or completed logical request. Attempt timings are
elapsed from the logical request start, not additive per-send durations.

Counters, served model, tier, timings, and safe provider error IDs come from
upstream observations. First output excludes heartbeats, empty deltas, lifecycle
events, terminal snapshots, and synthetic client SSE. Unknown timing stays unknown.
Partial cumulative counters merge without null erasure; checked derived totals
stay unknown on overflow, and reported totals are retained.

Cache read/write counters remain per-request observations, including explicit
zeroes. Local cache activity is scoped by chat, account/source identity, and
upstream model. Only observed positive reads/writes move the touch; misses and
future or foreign retention records cannot supply its policy. The UI shows a
separate retention estimate, refreshes it through the existing usage revision,
and keeps previous observations after a failed read without extending them.
Model names do not prove an endpoint's TTL. Mixed prefix windows have no single
countdown, and an elapsed estimate is not evidence of provider eviction.

Managed account Responses requests reuse a client-supplied `prompt_cache_key`.
If absent, an identified client context supplies a deterministic opaque key
scoped to the local key and resolved model, shared by HTTP and native WebSocket.
Unscoped requests and API-source bodies receive no generated key. Compact and
search endpoints keep their own contracts. Cache hints never authorize a
continuation or override quota, load, eligibility, or client cache controls.
BPS places non-default per-turn tool controls after history, not in the stable
catalog prefix; default automatic tool selection adds no extra suffix.

API-equivalent cost is an estimate, not a customer bill, subscription debit, or
routing input. Price resolution uses observed provider prices, exact reference,
canonical reference, then manual fallback. Components, service tiers, and
published long-context bands stay separate; missing prices are not zero and do
not inherit another tier. Actual input usage selects the highest strictly
exceeded band. A model's context limit does not create a pricing threshold.

Debug context comparison uses bounded, salted in-memory fingerprints, never
stored prompts or arguments. It does not prove provider prefix reuse or a cache
hit. The manual `quality_probe` is opt-in, loopback-only, bounded, and separate
from background monitoring; see [operator steps](../../CONTRIBUTING.md#manual-quality-observations).

## Profiles and user-managed server

Profile operations follow inspect → protected snapshot → apply → verify → restore.
Rollback changes only unchanged Relay-owned fields and preserves newer user edits
and sign-ins. Credential snapshots use protected storage. Invalid executable
overrides fail rather than launching another installation.

Context evidence and policy remain separate:

| Policy | Projection |
| --- | --- |
| Short | Published default, capped by a known hard maximum; only without a default use 25% of a known maximum, with a 1,024-token floor |
| Extended | Full known maximum; otherwise preserve an available provider default |
| Custom | 1,024–2,000,000 tokens, capped only by known hard model/route limits |

Native context comes from the owning account's exact model card, not reference
API maxima or an installed Codex default. API routes use their own reference
evidence. Unknown windows remain unknown; no guessed 128k default. Known
short/long windows are not replaced by percentages or pricing bands.

Compaction is projected at 90% of the selected window, capped by known input
and output/hard-limit constraints. Relay never silently truncates requests.
Managed root context overrides are recoverable; explicit newer user settings
remain user-owned and do not block subsequent catalog updates or reconnects.
Catalog changes publish atomically without stopping requests.
A client that only loads configuration at startup still needs relaunch; saving
policy never automatically closes/restarts an application.

ChatGPT, Claude Desktop, and OpenCode have separate profile ownership and recovery.
Claude uses a Relay-owned UUID profile in its `Claude-3p` library, preserves foreign
profiles/selections, and maps stable client-only Claude IDs back to real models.
There is one entry per enabled Messages-routable model, not one per effort.
Desktop's native effort menu is capped at the highest pool-allowed level on a
Messages route, not the longest contiguous prefix. A sparse set does not hide
higher supported choices. `maxEffort` cannot hide intermediate holes. Reserved
Desktop reasoning aliases on Messages explicitly map the five native picker
levels to the nearest executable, pool-allowed lower level, or the lowest
allowed level when none is lower. The choice uses the configured route union,
not pool order or cooldown; an exact choice is not downgraded to fit another
member. Admission and preparation use the same captured effective effort,
while usage retains the original requested effort. Disabled thinking, explicit
budgets, unknown levels, Ultra, ordinary model IDs and other client protocols
are not remapped. Exact route and pool checks still apply to the effective
choice; an empty supported set does not grant reasoning capabilities.
Aliases grant no upstream capabilities, pricing, or context. Catalog overflow fails before
closing/writing rather than truncating. Client-owned MCP discovery is enabled
only in the owned profile.

OpenCode profiles explicitly disable standard SDK reasoning variants that the
model route does not advertise; omission would let OpenCode restore its built-in
defaults. Models without evidenced effort levels retain the provider's default
mode. Refresh can re-enable a newly supported effort, removes stale generated
variants, and preserves genuinely custom user variants.
OpenCode's built-in `Default` is the provider-default mode, not an advertised
effort, and cannot be removed by the model variant configuration. Clients that
cache the profile at startup must reload before its new effort list takes effect.

Connect validates routes, key, catalog, ownership, and launch target before process
changes. Only the requested launch preference starts the client after attachment.
Overview's application launcher only opens the selected installed application,
even with the local API stopped or an empty pool. It does not attach a profile,
refresh a catalog, change credentials, start the API, or stop a running client;
existing client configuration remains unchanged. Launch does not depend on a
Relay runtime refresh or the validity of a previous managed profile.
Launch preferences take precedence over `OPENCODE_BIN` / `CLAUDE_DESKTOP_BIN`,
then installation discovery. Explicit overrides are validated even when a client
is already running; invalid overrides never silently select another installation.
Autodetected files pass desktop identity and executable checks before a restart.
Discovery continues past CLI entries on PATH and compares Claude version numbers
numerically. Process control requires a recognized desktop path and excludes
Chromium helpers. OpenCode and Claude launch success requires a running desktop,
not merely a successful spawn. Blocking launch/restart commands run off the UI
thread; the legacy saved-key launch leaves Relay visible when launch fails.
Restore refuses modified owned settings, preserves unrelated changes, and does
not automatically relaunch. Source edits do not upgrade an already running binary.

Remote management is capability- and revision-checked. Presets transfer pool,
model, order, and price settings; credentials require a separate explicit transfer.
They are not hidden two-way synchronization. Server SQLite affinity fingerprints
do not persist native replay/bridge payloads, which remain bounded memory state.

## Not provided by the current implementation

Hosted tenants, customer identities/keys, wallets, payments, and billing are not
current single-owner server features. PostgreSQL checkpoints, encrypted payload
retention, SDK idempotency, and settlement are planned in
[HOSTED_RELAY_ARCHITECTURE.md](HOSTED_RELAY_ARCHITECTURE.md).

Distributed scheduling, account resale, fingerprint spoofing, sharing concealment,
and automatic transfer of opaque conversations between accounts are outside the
first hosted release. Open local acceptance and measured improvements belong in
[ROADMAP.md](ROADMAP.md), not in claims of completed behavior here.
