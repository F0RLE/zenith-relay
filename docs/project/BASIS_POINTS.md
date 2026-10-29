# Excel / Basis Points

Current executor contract. The [architecture map](PLANNING.md) owns shared
runtime boundaries; [open acceptance](ROADMAP.md#excel--basis-points-acceptance)
remains in the Desktop roadmap.

BPS is a separate account executor, not the public Responses API. Its behavior
is observed and regression-tested, not guaranteed by public OpenAI documentation.

## HTTP request encoding

Relay leaves BPS Responses JSON uncompressed, including when client
ingress uses WebSocket. Native Codex compression and response-compression
negotiation (`Accept-Encoding`) do not establish BPS request support.
Neither an HTTP 401/422 status nor telemetry compression proves inference
request-decoder support. Do not enable it without endpoint-specific evidence
or resend a generation after an encoding failure. The local
[compression probe](../../CONTRIBUTING.md#request-compression-probe) measures
transfer costs, not BPS compatibility, model tokens or prompt-cache savings.

## Access and request context

- Permission discovery is token-incarnation-scoped, coalesced, bounded, and
  cached briefly. A changed token or failed access read cannot reuse old access.
  Restricted models and account policy are enforced at execution.
- Before rejecting an unknown WebSocket model, refresh scoped BPS access and
  resolve it again. This applies to initial frames and later turns, including
  HTTP fallback; known routes and malformed frames need no discovery I/O.
  BPS-only models use their HTTP executor without a native Responses upgrade.
- Compact messages, attribution, tool history, and inline attachments are
  normalized at this boundary. Collaboration context is lower-authority user
  context; remote image fetching is unsupported.
- Direct function string properties annotated `encrypted:true` are validated as
  plaintext, then sealed in a Relay-owned authenticated carrier before release.
  Nullable strings and omitted optional values retain their meaning. Nested,
  composite, and reference-based encryption schemas fail before dispatch rather
  than leaking plaintext. Each encrypted string is limited to 1 MiB of UTF-8.
  HTTP and WebSocket ingress restore these carriers in `agent_message` content
  and matching function-call history before admission; agent attribution stays
  context, never developer authority. Existing volatile replay snapshots retain
  canonical arguments so recovery does not resend client-only carriers.
  JSON and SSE use the same sealed argument,
  including terminal snapshots, with no premature executable output.
  Carriers remain bound to the local gateway key ID. Desktop loads their
  independent host key from a typed record in its existing encrypted secret
  vault before constructing the runtime. The same protected data directory can
  therefore resume saved carriers after graph replacement or process restart;
  another local key or independently initialized profile cannot open them.
  Key creation is atomic under the configured vault's writer mutex and must
  persist before use. Desktop's single-instance boundary owns the single writer;
  the mutex is not a multiprocess lock. A legacy vault without the record creates
  it once; a present null, malformed or unsupported record, unavailable vault or
  failed persistence stops startup rather than silently replacing the key.
  Carrier keys are not account credentials or generic secret references, and
  account cleanup cannot delete them. Messages and conversation replay stores
  remain volatile; there is no persistent message cache. Standalone Core hosts
  must explicitly supply their protected carrier key to survive restarts.
  Previously minted v1 carriers whose old volatile key was already lost cannot
  be decrypted by this change; neither a replacement agent nor a new key recovers
  that missing key. Authentication failures return a specific unavailable-history
  error with the original-profile/key or new-chat recovery options, not a generic
  source-adapter failure; they never dispatch to a provider or retry another route.
  Foreign provider ciphertext stays opaque and retains native-route behavior;
  BPS still rejects it explicitly. The public
  [Responses multi-agent guide](https://developers.openai.com/api/docs/guides/responses-multi-agent)
  describes encrypted `agent_message` items, not BPS decryption support.
- The observed Codex `codex_app.create_thread` handoff is lowered to labelled
  user context only when `call_id` is absent and its nonempty string output has
  complete `<codex_delegation>` delimiters. Original text and source ID remain
  visible; extra fields are rejected rather than hiding media or ciphertext.
  Ordinary results, including present/null/empty call IDs, keep their existing
  identity/pairing rules. Metadata is computed before this generated user message,
  so the handoff does not create a new logical turn or reset iteration.
- Metadata sent upstream contains only Relay's task/turn/iteration fields.
  Client metadata stays local and is restored in the translated response.
  Turn fingerprints borrow the relevant history prefix; JSON hashing streams
  through a bounded buffer shared with cross-protocol continuation keys. Their
  serialized meaning and identifiers stay unchanged, without extra full-history
  clones or serialized copies.
- After copying the client history for preparation, compact messages, generated
  text prologues and visible reasoning move their owned text buffers instead of
  cloning them again. Wire field order, extensions, metadata and provider-owned
  ciphertext behavior remain unchanged; the caller's request is not mutated.

## Executable calls and stream validation

- Ordinary text streams incrementally. Executable tools are withheld until the
  authoritative terminal validates identity, arguments, and original schemas.
  Failed/incomplete/truncated output releases no tool subset and triggers no
  regeneration. Synthetic client events are not timing or usage evidence.
  Invalid function schemas fail during preparation with `tools.parameters`;
  completed arguments that violate a valid schema retain
  `output.run_officejs.args.schema`. Safe diagnostics distinguish compilation
  from argument validation and include only an allowlisted constraint category
  and structural schema/instance depths, never property names, reference URLs,
  schema excerpts, or argument values.
  JSON and SSE retain the same tool-validation category and safe cause: malformed
  JSON, a non-object argument, and a schema mismatch remain distinguishable from
  a broken stream. Argument contents are never included in these diagnostics.
  Inner JSON syntax errors and premature EOF have separate safe parameter
  suffixes and static causes; both survive JSON and streamed error reporting.
  Their diagnostics retain only an allowlisted parser reason, payload-shape
  category, byte length, and failing decoder line/column (after any supported
  whitespace normalization). No key, value, source excerpt, or raw parser error
  enters diagnostics. BPS executable-tool failures additionally name a static
  validation stage and `repair_skipped=executable_tool`;
  these labels never authorize another generation or include tool names/values.
  Tool-identity failures additionally distinguish missing/invalid names, unknown
  tools, ambiguous names, conflicting legacy names, and reference/name mismatch
  with a static `tool_identity_reason`; no requested or declared name is logged.
  Buffered and streaming discovery preserve these fields
  in both the client error and request usage record, without regeneration.
  Completed client calls cannot reuse a call ID already present in supplied
  history, in compact/full mode as well as local discovery.
- Complex function string arguments may use Relay's explicit private frame:
  `relay-function-string-v1 {"field":"cmd","args":{}}`, one newline, then
  the unchanged string value. Other arguments live in the JSON header. The
  generated header stays compact on one line; its remaining string values still
  need JSON escaping. Only the selected body is raw. The decoder also accepts
  pretty-printed headers and the supported literal newlines inside header
  strings: the body starts at the first newline outside JSON containers and
  strings, without trimming or decoding its bytes. Malformed quotes and missing
  separators remain failures; Relay never guesses or regenerates executable code.
  CR-only and literal `\n`, `\r\n`, or `\r` delimiters are also accepted outside
  the header for transport compatibility. Only that delimiter is consumed;
  subsequent body bytes, including escapes and leading whitespace, stay exact.
  The reconstructed object still passes the original schema and tool-choice checks;
  duplicate fields and malformed headers fail. Ordinary JSON calls remain
  supported and custom tools stay opaque, even if their text resembles a frame.
  Replayed client function calls and generated examples share this encoder:
  the top-level string needing the most JSON escapes becomes the raw frame body;
  other arguments retain their types in its header. Plain arguments stay JSON.
  Shell examples include a declared working directory when its schema accepts
  the example, keeping the command raw and the directory JSON-escaped in the
  header. Undeclared arguments and incompatible examples are never added.
  This removes one JSON-escaping layer, not context or model reasoning.
  For compatibility, the inner function JSON decoder also escapes literal LF,
  CR, and tab characters inside quoted strings without changing their decoded
  bytes. Unknown printable string escapes such as `\s`, `\q`, or `\'` retain
  their literal backslash instead of invalidating the call. Standard JSON
  escapes keep their existing meaning; malformed Unicode, escaped control
  characters, quotes, incomplete objects, and schema violations still fail.
  Custom input and explicit frame bodies remain opaque. This compatibility
  grammar is private to BPS; client JSON and the public API remain unchanged.
- The legacy single-call `name(JSON)` compatibility parser also handles
  referenced function calls. The parsed name must exactly match the one
  reference; normal schemas and tool choice still apply. No code is evaluated,
  no tool identity is inferred, and custom input is never parsed this way.
  Referenced function payloads may contain one legacy `tool`/`name` and
  `args`/`arguments` envelope. Already-valid arguments are preserved first;
  recovery requires exactly those two fields, the same fully qualified tool,
  and an explicit closed object schema whose properties cannot describe the
  envelope (no pattern properties). The recovered object must pass the complete
  original schema. No extra fields are discarded, nested wrappers recursively
  removed, or calls regenerated; custom input remains opaque.
- Local BPS stream failures use the Responses `error` event's top-level
  `code`, `message`, `param` and ordered `sequence_number`. They do not fabricate
  a successful completion or reveal tool arguments. Text still streams before
  final tool validation. After the first native tool item, later indexed output
  waits for the authoritative terminal so batch expansion cannot renumber text
  already sent. Terminal-only or out-of-order tools that would shift emitted
  indexes fail closed.
  Discovery and direct streaming share native tool identity/index validation;
  completed terminals with changed or missing calls fail before catalog
  continuation or client release.
  Discovery preserves canonical upstream failure messages. A normalized
  `upstream_rate_limited` stays a
  rate-limit failure through SSE conversion and usage recording, never a generic
  `upstream_terminal`; output already sent does not authorize replay.
  Untyped SSE error envelopes retain their category. Provider retry delays and
  scope survive client-error normalization in request-local settlement state;
  they are not shortened to the default cooldown or exposed as raw error bodies.
  Both consume coalesced SSE frames without shifting the remaining buffer for
  each event; frame/response limits and the first-terminal boundary still apply.

## Diagnostics and timings

- Dispatch moves the prepared BPS body instead of retaining a duplicate history;
  catalog diagnostics share the session's parsed declarations. Wire bytes,
  discovery authority and cache-prefix ordering stay unchanged.
- Paired context sizes describe the client request and the final prepared body,
  after catalog preloads and inline-image replacement. Optional `conversation`
  counters partition conversation bytes into messages, tool calls, tool results,
  reasoning, attachments and other items. Nested typed blocks own their bytes;
  wrappers stay in the parent category, and outer history delimiters remain
  outside conversation bytes. Counts retain no content or identifiers; legacy
  records have no invented breakdown. Classification is bounded and does not
  inspect arbitrary tool arguments or decode provider ciphertext.
- Catalog `selectionReason` distinguishes explicit full/compact/discovery,
  automatic discovery, a below-threshold catalog, no tools, a lookup-tool
  collision, and dispatch-budget fallback. Missing legacy reasons stay unknown.
  `relayToolInstructionBytes` measures the known generated tool-instruction
  text as JSON bytes: it is a subset of upstream instruction bytes, can include
  the catalog, and does not claim to measure all Relay-generated history text.
- `httpVersion` records the actual upstream HTTP response version when observed,
  independently of client WebSocket ingress. It does not prove connection reuse;
  failures before response headers leave it absent.

- Safe request diagnostics retain the client's explicit parallel-call boolean
  separately from the native BPS final-generation call count and validated
  client call count. Native counts include local catalog lookups before they
  are consumed; intermediate discovery generations are not added to the final
  count. Missing observations stay absent, not false or zero. No names,
  arguments, instructions, or response text are retained by these counters.
  First HTTP response-header arrival and first validated client-call readiness
  use the executor's dispatch clock, independently of upstream TTFT. Readiness
  does not measure client receipt or tool execution; discovery time is included.
  These observations neither release tools early nor change parallel policy,
  effort, routing, retries, or cache behavior.
- Optional transport timings separate selected-attempt preparation duration
  (`requestPreparationMs`, including catalog preparation and attachment uploads)
  from dispatch-relative offsets: raw BPS collection (`upstreamResponseMs`),
  first meaningful normalized SSE output (`clientFirstOutputMs`), and successful
  normalized response readiness (`clientResponseReadyMs`). Existing upstream
  TTFT/generation accounting remains unchanged. Preparation excludes admission
  and route selection; offsets are not additive stage durations. Readiness is
  observed inside Relay before delivery, not client receipt, tool execution,
  or the pause before the next request. Buffered replies outside the observed
  SSE stream do not invent a client-first-output observation. Unobserved stages,
  older records, and
  buffered discovery without raw timing evidence leave fields absent; measured
  zero stays zero. Failed/incomplete/cancelled responses do not claim successful
  readiness. These counters persist in existing routing JSON, without content
  retention or a new database schema.
- `requestPreparation` optionally breaks that duration into request building
  (`bodyMs`), initial schema validation (`schemaValidationMs`, a subset of
  `bodyMs`), catalog preload/fallback (`catalogMs`), final JSON encoding
  (`serializationMs`), and
  attachment scanning/cache/upload (`attachmentsMs`). These are durations,
  not dispatch-relative offsets. They describe successful preparation in the
  HTTP request/account executors; unobserved stages stay absent. Older
  observations without `serializationMs` retain final encoding inside
  `catalogMs`; no separate encoding time is inferred for them. Schema timing
  includes selecting/parsing the tool catalog and structured-output schemas,
  not just validator compilation. Compact fallback revalidation belongs to
  catalog time. Rounding and remaining dispatch setup can leave a gap to the
  total. The recorder retains only counters and does not change validation,
  caching, wire bodies, or upload/retry policy.
  Optional `contextAccountingMs` measures prepared-context accounting after
  attachments, separately from those phases; it is not all diagnostic overhead.
  Support-export phase accounting includes it when observed.
  Schema-3 support exports expose this gap as
  `transport.preparationAccounting.unattributedMs`, computed from observed
  non-overlapping phases only. Initial schema validation is never added twice.

## Parallel tool calls and batch replay

- Tool guidance prefers grouping related read-only shell inspections into one
  declared shell-tool invocation, with the configured shell and each command's
  output/failure preserved. Concurrency is limited to independent reads and must
  respect task/tool permissions, approval boundaries, dependencies, and shared
  state. This is guidance, not automatic command rewriting. Actual scheduling
  and approvals belong to the client; concurrent execution is not guaranteed.
- Broad inspections request compact results while retaining required instructions,
  errors and edit context. Related missing contracts share a catalog lookup;
  asynchronous work uses job handles and bounded waits. Screenshots remain
  available for visual decisions and required fresh observations; this does not
  remove images/history or lower reasoning effort, validation, or safety.
- Core shell/edit/wait contracts receive eager-loading priority over optional
  declarations, avoiding discovery-only turns when an alphabetically earlier
  catalog would fill the budget. The eight-tool/16 KiB limits, explicit deferral
  and current tool-choice restrictions still apply; other tools remain discoverable.
- For independent generated scripts, patches and function-string frames, guidance
  prefers separate native calls in one response, preserving raw bodies without
  another nested batch-JSON layer. This does not disable parallel calls or remove
  existing batch formats; simple payloads may still use a compact batch.
- BPS also accepts an explicit Relay-local batch: ordered `references` names
  and `code` containing `relay-tool-batch-v1`, a newline, and a JSON payload
  array. There must be 2–8 calls and one payload per reference, with at most
  1 MiB of code and 1024 output items after expansion. Repeated names and
  namespaces are preserved. Function payloads are argument objects or encoded
  function strings; custom payloads must be strings and stay opaque.
  Batch headers accept LF or CRLF without modifying payload strings. Their JSON
  array must be one complete JSON value: trailing text, extra closing delimiters
  and concatenated values reject the whole batch without releasing a valid
  prefix. The array and function-string frame headers use the same strict-first,
  lossless escape/control handling as single function payloads; invalid Unicode,
  incomplete JSON and schema violations still
  fail the entire batch. Safe `.batch.*` and `.args.frame.*` parameters identify
  framing, size, count or shape failures; JSON diagnostics never include values.
  Catalog lookups and recursive transport calls are forbidden inside batches. Ordinary
  single-call envelopes stay supported. All children pass the existing current
  schema/tool-choice checks before any call is released; disabled parallel
  calls, invalid batches and incomplete terminals never expose a valid subset
  or cause regeneration.
  In a completed response, an explicit native tool-item status must be
  `completed` before batch expansion or local catalog lookup; missing item
  status remains accepted. Invalid status rejects the whole response.
  Child `call_id`s reversibly encode the native parent, count and position under
  the reserved `call_rb1.` prefix. Native parents are nonblank, non-control IDs
  of at most 64 bytes outside that prefix; duplicate/replayed IDs are rejected.
  Client aliases can exceed 64 characters and collapse before upstream ID
  limiting; they are opaque associations, never authorization. Full-history
  replay requires ordered contiguous child definitions and complete result
  groups. Results from independent batches and single calls may interleave
  within one uninterrupted tool-result block; groups retain first-seen order
  and children are matched by their encoded identity, not completion order.
  Missing, duplicate or mismatched results fail closed. Grouping never crosses
  an intervening message, instruction, reasoning item or new call. When a complete
  batch's results straddle such a boundary, history replays its children as
  individual native transport calls with matching results, without moving the
  boundary or executing tools again. Ordered contiguous definitions, complete
  unique results, original tool kinds/status and payload limits remain required.
  Upstream item IDs use the existing stable hash when aliases exceed the length or
  ASCII letter/digit/underscore/dash limits; call/result associations stay intact.
  Uninterrupted batches retain compact parent replay. Each result
  retains its child identity and metadata as labeled
  tool-result text, with string outputs left literal and typed content kept in
  order. Existing attachment handling processes typed images; file parts remain
  typed. Opaque reasoning and route/credential ownership stay unchanged. No new
  persistent state is introduced. This is a private transport contract, not a
  public provider feature or a guarantee that a model will choose batching.

## Catalogs, discovery and cache boundaries

- With explicit `additional_tools`/`tool_search_output` history, the transport
  prologue is catalog-independent and each declared batch renders at its original
  position. Later batches cannot rewrite earlier declarations; compact/full
  schemas or discovery indexes follow the requested presentation policy.
  Root declarations overriding that history are supplied after history, with
  current schemas/permissions authoritative for execution. Root-only catalogs
  retain their original prefix placement so an unchanged catalog does not move
  on every turn. A changed root-only catalog or presentation mode can still
  invalidate that prefix: Relay does not invent a hidden historical snapshot.
  `tool_choice: none`, named function/custom choices, and `allowed_tools`
  restrict the current turn after history; the prologue remains unchanged.
  A visible contract is not permission to call it: schema lookup, preload and
  completed-call validation remain scoped to the permitted tools. With no
  permitted tools, a final text-only guard forbids transport and catalog calls.
  An actually empty catalog still uses the ordinary no-tools preparation.
  Explicit named/allowed selections preload their small permitted contracts
  under the existing eight-tool/16 KiB eager budget, respecting explicit
  deferral, so stable discovery presentation does not require a lookup just
  to recover a small selected schema. Checkpoints borrow the exact prepared prologue.
- Parsed tool definitions, serialized declarations, and lazily rendered full/
  compact descriptions share immutable allocations in a content-addressed cache.
  Namespace, declared name, kind and exact definition bytes determine identity;
  selection and executable-call validation still use the current request.
  Limits: 1,024 entries, 512 KiB charged per entry, 16 MiB total charged retained
  allocation, ten minutes idle reuse. Charges include parsed objects, serialized
  declarations and reserved lazy rendering space; they are not process RSS.
  Oversized definitions work without retention. No arguments, history, provider
  state, credentials or per-request permissions enter this cache.
  Current-catalog collection borrows declarations until historical/root overrides
  are resolved, then prepares only the surviving definitions. First-position
  ordering, current root precedence and malformed-declaration rejection remain
  unchanged; historical catalog messages keep their original timeline.
- Catalogs use compact schema presentation without changing the original
  validator. Complex/unknown JSON Schema constructs remain complete JSON.
  Compact property names and annotation keywords have deterministic ordering,
  so reordering those schema keys does not rewrite the rendered catalog prefix.
  Annotation rendering borrows schema values;
  descriptions, constraints, defaults and examples remain intact.
  Closed objects retain explicit `additionalProperties:false` at every schema
  depth; a missing ellipsis alone is not their model-facing constraint. Tool
  arguments use the current declaration, not historical or transport fields.
  Successful offline validators are reused by an exact schema-content digest
  in a volatile cache: at most 512 entries, 128 KiB per schema, 4 MiB of schema
  input in total, and ten minutes idle reuse. These byte limits measure schema
  input, not compiled-validator memory. Oversized schemas still compile and
  validate normally without retention; changed schemas compile independently.
  Compilation stays outside the cache lock. No tool arguments or provider
  continuation state enter this cache, and no remote references are retrieved.
  Immutable prepared declarations also remember successful schema preparation,
  avoiding repeated schema serialization and digest lookup during preflight.
  This retains only a success marker, not another compiled validator. Changed
  definitions compile independently, failures are never marked successful, and
  every actual call still validates arguments and current-turn permissions.
  Automatic Relay-local discovery applies from 24 tools and 24 KiB of complete
  declarations (including descriptions, schemas, and custom formats):
  the initial JSON index groups names by their declared kind and namespace;
  qualified names reconstruct exactly, without aliases or inferred namespaces.
  Lookup searches the original names, descriptions, and schema text and returns
  the complete trusted definitions, including usage rules; descriptions and
  argument names are not repeated for every unloaded tool. Previously
  called tools start loaded with lossless compact schema presentation after
  history, without rewriting the stable index prefix. A bounded eager set also
  loads declared `exec_command`, `apply_patch`, and `write_stdin`, plus explicitly
  non-deferred tools, up to eight tools and 16 KiB; explicit deferral is respected.
  Discovery remembers at most 64 names per completed tool-call checkpoint in a
  volatile 256-entry, one-hour cache scoped to the local key, model, and history
  call ID. Lookup uses validated prepared history's upstream call IDs: compact
  batch parents or standalone children, retaining the original client-model
  scope.
  A completed local lookup also updates a checkpoint already present in the
  prepared history. Its hints remain reusable after a later answer fails or the
  dispatch budget is exhausted, including SSE. An explicit client retry can
  preload current trusted contracts without another lookup round. Budget
  exhaustion records the last generation once, as a terminal failure, not also
  as an intermediate usage event. Only bounded names and the existing scoped
  prologue are retained;
  no answer, history, execution result or provider continuation is cached.
  On reuse, names
  are intersected with the current selected catalog and current definitions are
  rendered within a separate 16 KiB preload budget; removed/disallowed tools
  never return through this cache. Missing or expired hints fall back to ordinary
  discovery. The notation legend is
  sent only with schema presentation, not with an index-only request. Lookup
  returns each original full definition once per discovery session; repeated
  selections refer to its earlier lookup result. Client instructions, history,
  reasoning effort, and original validators are not pruned or reduced.
- Those checkpoints may also retain the Relay-generated transport prologue, at
  most 32 KiB per entry, shared across a response's call IDs under the same
  volatile count/expiry limits. Reuse requires the same local key, model,
  historical call ID, identified client context, and digest of top-level and
  leading system/developer instructions. Client instructions, conversation,
  actual tool arguments, and provider state are not retained by this cache.
  For a continuation with no current tool declarations, the historical prologue
  keeps its original position and an explicit current no-tools guard follows
  history. Current declarations remain empty and output validation rejects tool
  calls; historical context never restores tool permissions. Missing, expired,
  oversized, or mismatched hints keep the ordinary text-only preparation.
  Reasoning effort and client compaction thresholds are unchanged. This includes
  tool-less compaction sent through ordinary
  Responses, not a change to `/responses/compact`. Prefix continuity does not
  guarantee provider cache hits or lower latency on the undocumented transport.
- Debug context comparison fingerprints the actual BPS body after preloads and
  attachment resolution, including subsequent catalog-discovery sends. It
  reports account changes, prefix/history changes and effective reasoning effort
  without storing prompts, tool arguments or fingerprints. The successful final
  send establishes the next client-request baseline; lookup usage stays separate.
  These local observations are not provider cache-miss verdicts. Debug-off makes
  no fingerprints, and diagnostics never alter cache keys, retention or requests.
- Discovery is a request-local read-only operation, not hosted `tool_search`.
  It allows at most eight lookup rounds, eight tools per lookup, 4 MiB retained
  input added by discovery, and 16 MiB per response. Admitted conversation
  history is not charged again against the discovery continuation limit.
  Every generation consumes the original dispatch budget and retains its
  OAuth incarnation/route/lease; no
  background task, owner change, paid probe, or post-uncertainty replay exists.
  A budget with fewer than two remaining sends selects compact mode before
  the first generation. Repeated or overlapping selections return only new full
  definitions in `tools`, plus `reused` entries containing each already supplied
  name and its original lookup `call_id`. An all-reused selection has `cached:true`.
  The referenced result remains unchanged in the same request's history; these
  references neither restore provider state nor grant tool permissions. A new
  generation is still needed to consume the lookup result, under the original
  budget; a repeated query alone does not authorize executing a client tool.
  Invalid arguments to a recognized local catalog lookup return a bounded
  tool error for correction under the same budget and owner; they do not
  authorize replay or repair of executable client calls.
  Legacy `tool`/`name` and `args`/`arguments` envelope fields are normalized at
  the shared transport boundary, so local lookup recognition, result removal,
  and client translation use the same identity. Conflicting name fields fail;
  referenced payloads and custom input stay opaque at this boundary.
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
- Parsed declarations belong to the immutable prepared client request and are
  reused for discovery and final JSON/SSE validation. This is not a cross-request
  permission cache: selection, tool choice and argument validation still use the
  current request. Account-only BPS preparation borrows unchanged history until
  a model or timezone rewrite needs ownership, then builds the wire body once.
- Discovery preserves upstream SSE for streaming inference clients. Ordinary
  text streams before terminal validation; executable calls remain withheld.
  After a withheld tool, later output whose index may shift waits for the
  terminal response. Local lookups continue inside the same response body,
  preserving one lifecycle and output order across generations. Explicit local
  output validation and upstream JSON fallbacks remain buffered; synthetic SSE
  is not first/last-token timing evidence. Lookup attempts record actual usage
  separately. Final response usage remains the last generation's counters;
  catalog diagnostics report additional lookup input/output without counting it
  twice, and cumulative time through the last lookup (already included in total
  request duration), not a sum of cumulative attempt durations. Schema bytes
  count compact preloads and unique full definitions introduced by lookups;
  reuse references do not count the same full schema again. Intermediate visible
  text, reasoning, and completed hosted web-search items are preserved in their
  original order, so the final output retains already-streamed item indices;
  native internal items replay only inside that request, not in a cross-request
  discovery store. Dropping the response drops discovery and its upstream body;
  no detached generation survives client cancellation.

## Attachments, capability limits and authorization failures

- Both request and account dispatch keep the prepared JSON value through catalog
  preparation and image resolution, then encode it once for HTTP. Discovery
  retains that same resolved value, without reparsing the wire body or retaining
  a redundant encoded copy of inline images. Text-only and existing-file-reference
  requests skip attachment credential lookup; references still pass validation and
  normalization. Inline-image cache reuse always checks the current credential.
  Final encoding is measured in `serializationMs`, not upload time;
  image validation, account/credential cache boundaries and cancellation are
  unchanged. Tools still wait for complete response validation before release.
- Inline-image uploads share in-flight success or failure for the same endpoint,
  account, credential fingerprint, media type and image bytes. Only file IDs
  enter the existing 512-entry cache; upload failures are not retained there.
  Up to 512 concurrent keys share request-owned initialization; excess new keys
  keep ordinary upload behavior. Weak entries do not retain requests or image
  data. Cancellation drops the active upload, without detached work; an existing
  independent waiter may perform its own upload. Credential and account scopes
  remain separate, and discovery continuations reuse uploaded file references.
  A second, versioned digest of the exact inline representation reuses a validated
  successful file ID before base64 decoding. It shares the same account/credential
  scope and 512-entry budget with byte-content keys; neither raw inputs nor image
  bytes are retained. Invalid images and failed uploads never seed this shortcut.
  Within one request, at most four image jobs resolve concurrently after all
  images pass validation. Bytes, input order, and input-ordered failures stay
  unchanged. Each completion frees a slot without waiting for earlier images;
  only completed IDs/errors wait for their input position. An observed failure
  stops admitting new jobs while earlier uploads resolve the input-ordered error.
  Cancellation/failure drops pending work before generation.
- BPS does not accept provider-side `previous_response_id`. Responses clients
  may continue through Relay's bounded, volatile native replay store: validate
  the exact predecessor owner, materialize its complete history before route
  preparation, and send BPS a self-contained request without the predecessor ID.
  Missing replay state fails locally; over WebSocket, `previous_response_not_found`
  lets a compatible client resend full history on a new connection. Relay never
  drops context or rotates merely because BPS cannot resolve an opaque ID.
  Native replay uses the existing 16 MiB total budget without a separate 2 MiB
  conversation cutoff. Full supplied history also reconstructs
  the tool catalog. The catalog preload hint cache is separate and cannot restore
  a conversation. Non-Standard tiers, native strict output, native web search/image
  generation and Agent Identity remain unsupported.
- A JSON schema with explicit `strict: false` buffers output and validates the
  unchanged self-contained schema locally without a Relay-private request flag.
  `relay_bps_output_validation: false` opts out; the explicit `true` option also
  enables the existing local JSON-object mode. Neither is constrained decoding;
  `strict: true`, invalid/incomplete output, and external references fail.
  Omitted strictness still requires explicit opt-in. JSON and streaming catalog
  discovery withhold the complete answer until terminal validation succeeds.
  Admission checks native and converted output formats before reserving a BPS
  member; catalog/access discovery stays independent. Incompatible BPS routes
  cannot consume selection capacity ahead of a compatible native route. A BPS-only
  pool still rejects native strict output; no schema or approval check is dropped.
- Generation 401 invalidates only its rejected token incarnation and returns
  without another generation. The next request prepares fresh authorization.
  Account-wide 429 and model-access blocks retain separate scopes/deadlines.
  Without explicit route-recovery waiting, the first proven not-sent BPS
  rate-limit rejection starts a maximum 30-second recovery window, or the
  shorter configured window. Account changes and transport handoffs do not
  restart it. With route-recovery waiting enabled, this time cap is suspended;
  provider cooldowns, client cancellation, dispatch/wire limits and the ban on
  replaying accepted or uncertain generations still apply. Turning waiting off
  restores the original deadline, not a new window. An already accepted
  generation is not interrupted by deadline expiry. Expiry during preparation
  preserves the last provider rejection and retry evidence rather than
  replacing a 429 with a synthetic 503 or recording an unsent generation.
  A BPS 422 invalid-body rejection is not a retryable server failure.
