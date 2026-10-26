# Changelog

All notable Zenith Relay changes are recorded here. The `Unreleased` section
tracks merged or review-ready work that has not been published as a release;
release entries are kept concise and link to the corresponding tag.

## [Unreleased]

<!-- relay-notes:en -->

This update adds Claude Desktop pool profiles, shared protocol conversion, and
recoverable account deletion. Changes below are grouped by area.

### Changed

- BPS preparation reuses owned text buffers for message normalization, generated
  instructions and visible reasoning, removing redundant copies without changing
  request bytes, conversation identity or validation.
- Coding-tool output keeps its exact text when sent to Gemini, including
  JSON-looking file contents and command errors. Responses/Chat conversion also
  preserves distinct developer/system instructions and their history positions.
- Added native Messages and Gemini token-count endpoints for API sources,
  respecting local-key permissions without generating an answer or inventing
  token estimates. Unsupported sources return an explicit error.
- Responses web search can now run through Claude Messages, retaining search
  sources, citations and follow-up history in JSON and streaming responses.
  Domain allowlists and approximate location are supported; unsupported search
  controls and unfinished server-tool loops return explicit errors.

- Rust uses toolchain 1.99.1, Edition 2024 and the 2024 formatter style across
  both packages. CI uses the same compiler and Bun 1.4.4; compatible dependency
  updates include nanoid 3.3.20 and openssl-probe 0.2.2.

- Enabled experimental HTTP/3 for direct HTTPS generation routes after a bounded,
  credential-free background probe. Cold or unsupported origins keep TCP without
  delaying the request; proxies and native WebSockets retain their transport.
  Failed submitted generations are not replayed. `ZENITH_RELAY_HTTP3=0` disables
  the experiment; diagnostics report the actual upstream HTTP version.
- Updated Desktop, web preview, and Rust dependencies to current stable releases,
  including Tauri, React, Tokio, HTTP/TLS libraries, schema validation, and SQLite.
  Production JavaScript uses two-pass Terser minification to stay within the
  existing bundle budgets, preserving lazy loading and the ES2020 WebView target.
- Repeated optional WebSocket handshake timeouts now trigger a short,
  route-scoped HTTP preference instead of repeating the two-second delay on
  every request. A single timeout still permits immediate native recovery;
  continuation ownership, account health, and generation retry rules are unchanged.
  A synthetic five-request loopback run fell from 10.19 s to 4.16 s;
  this measures stalled handshakes, not provider generation speed.

- BPS preparation moves the request history and reuses parsed tool declarations
  instead of copying them again. Diagnostics separate JSON serialization from
  catalog preparation and record the observed upstream HTTP version.

- Weekly remaining values now use one persisted, workload-matched calibration
  for comparable subscriptions instead of extrapolating each account's usage.
  Linked connections count once in the pool total; missing evidence shows `—`.
  Mixed workloads use the observed median, not a range or dominant profile. Cards
  retain the 80% price-coverage threshold and show coverage; Overview remains
  fully priced. Tooltips expose sample counts. Usage filters no longer change lifetime
  account payback; actual request usage and routing stay unchanged.

- Codex connections retain their catalog format during temporary BPS access
  failures. Unavailable models no longer appear as an invalid catalog or an
  unreachable API address; a fresh successful access check restores discovery
  without restarting Relay.

- HTTP, compaction and WebSocket default to full Responses without inferring Lite
  from native model catalogs. Live catalogs and managed Codex profiles also select
  full Responses so clients do not enable Lite themselves. Explicit client Lite
  requests remain supported; tool schemas, reasoning and parallel-call policies
  are preserved. WebSocket continuations reconnect when the explicitly selected
  Lite/full mode changes.

- Production builds group the eager workspace graph while keeping the native
  loader and deferred pages separate. Version lookup excludes development package
  metadata; obsolete translations are removed. Bundle-size limits stay unchanged.
  Request diagnostics reuse field markup; unused frontend command wrappers and
  duplicate reload explanations are removed. Usage displays Gemini Interactions
  consistently in protocol and endpoint fields.

- Desktop, web preview and Help use concise function and action descriptions.
  Documentation and code comments remove repetitive comparisons with rejected
  alternatives while preserving operational and security constraints.
  The Codex text-access warning icon is vertically centered with its dialog title.

- Overview adds weekly-quota API-equivalent history by subscription/workload,
  defaulting to all models within one subscription without duplicate ledger votes,
  with medians, spread and reset markers. Passive measurements deduplicate linked
  account routes, expire stale forecasts and avoid extra provider requests;
  subscription cost, routing and billing remain unchanged.

- Native Responses replay shares immutable history on cache lookup and compares
  string prefixes without temporary JSON copies. Memory limits, expiry, ownership
  checks and outgoing conversation history remain unchanged.
- Shared SSE parsing uses vectorized CR/LF search for native and converted
  streams. Event bytes, ordering, terminal checks and per-event limits stay intact.
- Large HTTP JSON requests and compressed bodies are parsed in bounded workers.
  On a synthetic 1.72 MB debug probe, median longest async-worker occupation fell
  from 58–59 ms to below 0.05 ms. Small uncompressed requests stay inline.
- Native WebSocket model checks and forwarding reuse decoded frames. Original
  text/binary bytes, usage, terminal errors and continuation ownership are preserved.
  A synthetic debug probe reduced text/tool delta decoding time by about 49%.
- Converted streams validate growing text and tool arguments without cloning
  their previous emitted snapshots. Text, reasoning, refusal and function-argument
  deltas extend retained buffers. Prefix, numeric-normalization, tool-identity
  and completion checks remain unchanged.
- Function-argument normalization skips quoted text with vectorized delimiter
  search. Escaped strings, numeric precision and incomplete-number handling
  retain their existing behavior.
- Pool and Connections reuse linked-account indexes and timestamp formatters
  when rendering cards. Pool availability shares visible-model normalization
  across members within each check. Quota freshness and routing decisions remain live.
- Account, sign-in and request-cache countdowns pause in hidden windows and catch up on return.
  Date formatters are reused across cards and ticks, including after a language change.
- Usage shares two covering read indexes across request pages, filters and
  reports. Summary queries separate logical timings from send consumption;
  database upgrades preserve history.
- Weekly card forecasts read indexed persisted measurements instead of
  aggregating request history on each account-quota refresh.
- Usage charts aggregate send consumption and pricing together. Six alternating
  comparisons on the same synthetic 100,000-send database reduced histogram
  calculation medians from 863 to 609 ms, or from 284 to 178 ms with a member
  filter. Counts, retry attribution and per-bucket price rounding are preserved;
  the separate bucket-pricing pass and its unused helper are removed.
- Request timezone now follows the assigned proxy automatically, without account
  pickers, global switches, manual bindings or a default New York fallback.
  Shared background jobs discover exits independently of the UI, refresh before
  the 15-minute expiry and pace failed lookups. Requests never wait for location;
  unknown/stale evidence leaves client fields intact. Legacy controls are removed
  on startup; newer observations update proxy cards through coalesced events.

- ChatGPT account settings briefly explain Codex sign-in for phone control and
  clarify that requests still use the pool.

- The Images only badge stays in Pool and its member picker, not on Connection
  account cards. Image-only admission and actions remain unchanged.

- Connection account cards now match Pool card typography, header spacing, and
  quota-bar thickness, without changing account data or actions.

- API-source and proxy bulk actions now replace search and sorting inline, like
  account selection, instead of adding a row that pushes cards down. Clearing
  selection restores the previous search and sorting controls.

- Proxy speed tests now queue in click order instead of rejecting concurrent
  requests. Cards distinguish waiting from measuring; waiting does not consume
  the measurement timeout or compete for download bandwidth.

- Local account cards can explicitly group Codex, SIWC and Excel/BPS authorizations
  and select one route from Accounts or Rotation. Credentials and quota remain
  independent; switching preserves pool weight/concurrency and waits for idle routes.
  Association checks the whole group; SIWC does not prove a shared workspace or allowance.
  Account-targeted automations follow route changes atomically with model validation.

- Codex, ChatGPT/SIWC and Excel / Basis Points remain separate
  sign-in methods. Zenith export/import preserves the issuing client and SIWC
  registration; imported SIWC requires a fresh sign-in before joining the pool.
  SIWC uses verified registrations, protected rotating tokens and public Responses
  HTTP/SSE. Unsupported capabilities fail explicitly; live acceptance is pending.
- SIWC opens in Relay's Tauri sign-in window with an isolated profile and the
  existing proxy controls. OAuth callback checks and cancellation remain in Rust;
  copy-link browser recovery stays available. Live embedded sign-in is unqualified.

#### Protocol adapters and Excel / Basis Points

- BPS prepares images in the existing request object and serializes once before
  sending, removing repeated full-history JSON copies in both dispatch paths.
  Wire output, discovery continuation, cache isolation and tool validation stay
  unchanged; a synthetic before/after probe measures the removed CPU overhead.
- BPS discovery avoids duplicate ordinary text/reasoning SSE capture while keeping
  terminal items, usage, errors, model/tier evidence and the total-response limit.
  Replay preparation moves the original request instead of cloning its history.
- Codex request compression shares the input buffer instead of copying it onto
  the async thread. Debug diagnostics distinguish encoding wall time from HTTP
  execute-to-headers; first output remains a separate milestone. Reproducible
  local compression/transfer probes and their limits are documented in
  [Contributing](../../CONTRIBUTING.md#request-compression-probe). Provider support,
  payload contents and retry policy are unchanged.
- Protocol regressions cover interleaved calls, reversed completion order,
  fragmented UTF-8 and escaped arguments. Gemini schema conversion can consume
  an owned schema without cloning it again; canonical type names stay unchanged.
- API-key sources can explicitly use native Gemini Interactions JSON/SSE.
  Opaque interaction IDs remain caller/model/route-owned; interrupted streams
  retain known usage and report native errors. Generate Content does not imply
  Interactions support. Agents, background execution, retrieval and conversion
  are not included, and unsupported OpenCode SDKs do not advertise these models.
- Soft affinity can keep anonymous conversation branches on the opening
  instruction/user prefix and observed Claude child sessions with their parent.
  Explicit keys stay stronger; hints never authorize opaque continuation.
- Desktop capabilities expose stable connector-family/sign-in descriptors for
  existing OpenAI connections only. Other provider logins and their UI remain
  paused. Policy reloads match unique source IDs independently of order, and
  publishing the same runtime no longer retires its own listener/candidates.
- BPS preserves the observed Codex new-chat handoff as labelled context without
  inventing a tool call or changing turn metadata; extra hidden content is rejected.
- Full-history tool continuations no longer lose their native state when a
  client serializes function-argument object fields in another order, including
  nested file-tool options, writes an exact integral exponent as an integer,
  or splits adjacent same-role text into messages. Only comparison copies are
  normalized; arrays, exact values, negative zero, native arguments and reasoning
  stay intact. Regressions fail before the fixes and pass afterward; JSON/SSE
  file-tool chains recover their scoped state after client reserialization,
  while changed history still fails before dispatch.
- BPS accepts explicitly non-strict JSON-schema output without private client
  flags. It validates the unchanged schema before releasing JSON or streaming
  replies; invalid or incomplete output fails without repeating generation.
  Native strict output still requires a compatible route.
- Managed Codex picker aliases omit the placeholder `none` effort for catalogued
  thinking models without published effort controls. Native Responses can replay
  owned tool history after an explicit rejection by a stateless service.
- Request details separate the model declared by the upstream response from
  requested and routed models without treating it as verified identity.
  BPS tool failures identify the validation stage and why executable calls are
  not regenerated. Ambiguous encrypted reasoning is no longer removed in bulk.
- Native HTTP has an optional response-header deadline shared across attempts,
  disabled by default. It does not limit response bodies, BPS or native WS;
  expiry never automatically repeats possibly accepted work or disables accounts.
- BPS keeps turn-local tool choices and serial-call constraints outside its
  stable catalog/history prefix, including `none`, named function/custom and
  `allowed_tools` choices. Synthetic before/after probes retain all four leading
  input items instead of zero for restricted turns (38,407 compact / 34,177
  discovery JSON bytes, not tokens or observed provider cache hits).
  Small selected schemas preload within existing limits; discovery checkpoints
  reuse the prepared prologue instead of rendering it again.
  Account Responses requests without a client
  cache key derive an opaque context-scoped key consistently over HTTP and
  WebSocket; explicit keys, API-source bodies and routing limits stay unchanged.
- Debug cache-context fingerprinting reuses one bounded serialization buffer
  without changing digests or byte/item limits. Reproducible local before/after
  measurements and preparation-time tradeoffs are recorded in
  [Contributing](../../CONTRIBUTING.md#protocol-optimization-probes); provider
  cache-hit, first-token and billing improvements still need live acceptance.
- BPS supports explicitly framed multiline function strings without nested
  JSON escaping, retaining original argument-schema checks and opaque custom
  input. Local stream failures use Responses error fields and ordered sequence
  numbers instead of a nested HTTP error envelope.
- BPS preserves literal line breaks and tabs inside nested function JSON strings
  instead of rejecting the whole response. JSON and streaming share this decoder;
  malformed syntax and schema violations still fail without retrying generation.
- BPS accepts its existing single-call compatibility form with an explicit tool
  reference too, requiring an exact name match and original schema validation.
  Mixed old/new envelopes recover only one same-tool wrapper without dropping
  fields or changing valid arguments; custom input remains opaque.
  Invalid JSON diagnostics distinguish syntax errors from premature EOF without
  retaining arguments or automatically regenerating a response.
- BPS JSON failures now include an allowlisted parser reason, input-shape
  category, byte count and parse coordinates without argument contents.
  Buffered and streaming catalog discovery preserve those details in request
  diagnostics as well as the client error; invalid tools still never execute.
- BPS retains literal backslashes in unknown printable JSON-string escapes
  from tool arguments, avoiding failures for regex and command strings.
  Valid JSON escapes keep their meaning; malformed Unicode, structural errors,
  and schema violations still fail. Custom tools and explicit frames stay opaque.
- Buffered local adapter failures send `x-should-retry: false` so clients that
  honor it do not automatically regenerate after a failed conversion. HTTP
  status and usage stay unchanged; explicit client resends remain possible.
- Removing a deferred tool from client-search results no longer reactivates it
  through historical calls. Converted Chat/Messages replies reject contradictory
  tool stop reasons in JSON/SSE instead of publishing a successful terminal.
  Required/named tool choices also reject a normal text-only success while
  preserving refusal and token-limit outcomes.
- Responses client-executed tool search works across converted JSON/SSE routes,
  including deferred schemas, namespaces, paired results and the next answer.
  Search results do not duplicate complete schemas in foreign model input.
- Base64 PDF inputs retain bytes and labels across all twelve conversions;
  Responses/Messages also support PDF URLs and PDF tool results. Provider file
  IDs and unrepresentable rendering/citation controls remain native-only.
- Native Gemini continuations retain candidate tokens and late usage, bind to
  their original owner/key/model/contents and never automatically replay a
  generation. Native cache references and provider-hosted tools stay native.
- BPS preserves argument-validation causes in streamed errors instead of
  reporting every failure as a broken stream. Transport examples cover quoted
  multiline commands; all catalog modes reject call IDs reused from history.
- SSE parsing borrows single-line payloads and avoids repeated buffer shifts in
  protocol conversion, upstream usage capture, and HTTP-to-WebSocket fallback.
  Native Responses forwarding reuses validated delta
  parsing, including escaped text/tool arguments, without building a full JSON
  tree; usage and terminal checks remain.
  WebSocket forwarding avoids ordinary-frame copies and inapplicable retry
  parsing; HTTP fallback consumes parsed events without cloning JSON or opaque
  payloads. Converted streams avoid copying
  unchanged text/reasoning snapshots without weakening closed-block checks.
  Messages native text/thinking snapshots finalize at block closure instead of
  copying on every delta; signature fragments append in place.
  Responses appends custom-input fragments and missing final text suffixes in
  place instead of copying the accumulated prefix.
  Continuation hashing appends adjacent text fragments in place as well.
  Stream completion transfers existing history and native continuation state
  without cloning the whole prepared request; Gemini Part boundaries stay intact.
- BPS starts large tool catalogs with names grouped by kind and namespace, loading complete
  descriptions and schemas through bounded Relay-local lookups without weakening
  argument validation. Long descriptions and custom formats also trigger automatic
  discovery. Previously used schemas keep compact presentation after history.
  Context details show loaded schemas and lookup usage. Discovery streams text
  before pending tool calls and validates calls before release. Native tool-search
  semantics are unchanged.
- Large conversations use less scratch memory when Relay fingerprints their
  history for BPS and protocol continuations, without changing session identity,
  client instructions, history, or reasoning effort.
- Responses, Chat Completions, Messages, and Gemini use one conversion pipeline.
  Supported tool identities, namespaces, text/image results, schemas, citations,
  and parallel-call rules survive conversion; unsupported features fail explicitly.
  Native forwarding preserves provider extensions, precise numbers, object order,
  and permitted headers without cross-protocol conversion.
  ([1f64175](https://github.com/F0RLE/zenith-relay/commit/1f64175))
- Initial system attachments after a user message no longer block converted
  Claude requests before the first reply. Instructions retain their order in
  the target system field; instructions after earlier assistant turns still fail
  explicitly when the target cannot preserve their position.
- Reasoning settings and reports share the `xhigh` identity and actual effort or
  budget. Messages clients receive available public summaries and accept Claude's
  empty initial thinking signature while retaining native continuation state;
  foreign provider signatures are not fabricated.
- Excel/BPS supports tool-search results, function/custom tools, tool-result images,
  and collaboration attribution. It streams text but releases tools only after
  terminal validation against the current catalog and original schemas. Optional
  buffered JSON/schema validation is local, not provider-native strict decoding.
  ([#84](https://github.com/F0RLE/zenith-relay/pull/84))
- ChatGPT and Excel / Basis Points have separate sign-ins and issuing-client
  refresh. Native ChatGPT accounts can join the pool or launch ChatGPT directly;
  BPS is pool-only, rejects known Free plans, and supports Zenith/Sub2API exports.
  The old transport switch and manual callback entry are removed.
- Excel model access refreshes before catalog publication and execution. Disabled,
  restricted, and policy-blocked models are omitted; provider order is preserved
  and cached access belongs to the current token, with bounded reads.
  ([#84](https://github.com/F0RLE/zenith-relay/pull/84))

#### Pool, routing, and models

- Automatic rotation keeps a warm route eligible across a one-request normalized
  load difference instead of immediately sending the conversation to a cold member.
  Capacity, material quota/credit leads, health and opaque ownership remain enforced;
  unrelated work still spreads by load, with no new wait or provider cache-policy change.
  [Synthetic comparison](../../CONTRIBUTING.md#cache-affinity-admission-comparison)
  records route reuse and local overhead, not a live cache-hit or billing claim.
- Automatic rotation can move self-contained tool conversations to a better
  eligible member. Opaque continuations and unpaired results retain their owner
  unless validated local replay restores complete history. Both quota windows
  constrain selection; equal remaining percentages are not guaranteed.
  Same-model handoff restores saved tool/reasoning history behind
  `previous_response_id`, instead of using the plaintext model-switch repair.
  HTTP and WebSocket keep the cache key and current controls; missing history
  remains owner-bound. Original opaque reasoning is forwarded unchanged, not
  discarded or used as a blanket rotation ban. Local replay tests do not establish
  provider acceptance across accounts; provider cache hits are not fabricated.
- Credit refresh is independent of response headers. Busy traffic cannot keep
  stale credits fresh, postpone polling indefinitely, or replace newer limits;
  shared ChatGPT credit totals count the newest provider ledger once.
- API routes use each known model's native upstream independently of the client.
  OpenCode SDK selection follows the same reference policy; unsupported endpoints
  stay excluded. Model switches preserve completed history without transferring
  another model's active provider state.
- Context and compaction have independent controls. Published Short/Long windows
  and native account defaults are preserved; the 25% fallback applies only without
  a published default. API limits stay authoritative and long context is opt-in.
  Saves are serialized and show reload warnings without restarting applications.
  ([#84](https://github.com/F0RLE/zenith-relay/pull/84))
- Catalogs share a default developer/family/version order with manual overrides
  and a public policy export. Discovery retains provider priorities and valid
  cached data after failures. ChatGPT/Codex catalog reads accept up to 4 MiB
  and retry one transient transport failure with pacing.
  ([#86](https://github.com/F0RLE/zenith-relay/pull/86))
- Speed stays stable through retries. Prices retain separate Standard, Flex,
  Fast, Ultrafast, and published long-context bands, including explicit zero
  prices. Estimates use actual request input; missing prices remain unknown.
- API setup groups provider presets and console links, keeping Kimi Code separate
  from Moonshot's API wallet. Moonshot balances retain regional USD/CNY units;
  known services without balance readers no longer receive generic billing probes.

#### Claude Desktop and profiles

- Claude Desktop's fixed effort picker maps missing intermediate levels to
  the nearest supported, pool-allowed lower level, or the lowest allowed level
  when none is lower. Usage keeps requested and effective efforts separate;
  off, explicit budgets, direct API requests and other clients stay unchanged.

- Claude Desktop connects through a third-party inference profile. Relay validates
  before closing the client, saves recovery data, writes the catalog, and launches
  only when requested. Restore preserves foreign profiles and newer selections.
  Direct ChatGPT uses native discovery; pool profiles use Relay's catalog.
- Each Claude Desktop model appears once, with stable client-only route IDs and
  separate built-in Effort selection bounded by the route and pool policy. Real
  labels, upstream model identities, context, and prices stay unchanged;
  oversized catalogs are rejected before editing the profile.
- MCP tools load on demand. Messages bridges accept validated loaded definitions,
  discovery references, metadata, and advisory cache hints without emulating hosted
  search or Anthropic cache prices on other providers. Reconnect and start a new
  session to apply the profile setting.

#### Desktop UI and Access

- Slow runtime snapshots save their stage timings in one durable SQLite
  transaction instead of committing each stage separately. Validation, retention
  and the single-metric path remain unchanged; failed batches leave no partial data.
- Pool and Connections index runtime candidates once per snapshot rather than
  scanning every route for each card. Saved model ordering resolves IDs/groups
  once, compares numeric positions, and moves the original strings into the
  result; manual anchors, provider grouping and routing semantics are unchanged.
- Usage first entry sorts only send keys and decodes the selected page instead
  of buffering full-history diagnostics. Narrow totals/filter reads and indexes
  reduce database work; report loading no longer occupies the window event loop.
- Usage loads only the selected page and reuses unchanged visited pages. Period
  totals appear without a separate button; historical filter choices load on demand.
  Hidden or inactive views invalidate cached history without background report reads.
- Native Usage pagination reuses a bounded summary without retaining diagnostic
  pages. History and pricing changes invalidate it; catalog freshness still updates.
- Unchanged metadata refreshes share immutable model/index trees instead of
  rebuilding them. Source caches omit the derived merged JSON; compact internal
  field provenance preserves the same public evidence and conflicts.
- Restoring Usage reads only the selected saved preview instead of loading all
  eight reports into renderer memory. BPS resolves catalog overrides before
  preparing definitions and reuses successful immutable schema preparation;
  actual arguments and current tool permissions remain fully validated.
- Request details close without reconciling the Usage workspace. An active
  tooltip no longer consumes the dialog's first Escape, and backdrop dismissal
  preserves focus on the opener without a click-through focus change. This dialog
  keeps its dim backdrop without full-window blur and yields a paint opportunity
  before restoring focus, without measuring passive hints or overriding newer focus.
  Opening reads only the selected chat/candidate/model cache history on a worker,
  not the desktop event loop; closed technical details render only on expansion.
- Entering Usage no longer blocks the window while its page-open timing waits
  for the history database. Renderer timing writes run on a worker with the same
  validation and persistence rules.
- Local network enumeration, configuration and backup/trash reads, history
  clearing, and diagnostic writes no longer run on the window thread. Switching
  between Pool and Connections keeps the existing routing read and fallback
  timer instead of starting duplicate work.
- Overview displays completed analytics before saving its derived cache, coalesces
  writes and flushes pending data when hidden or leaving the page. Pool activity
  no longer repeatedly serializes unchanged source-statistics inventory; actual
  snapshot updates and explicit refreshes retain their existing behavior.
- Usage coalesces request-ID typing into a settled read and cancels unfinished
  searches on clear or tab change. Cached reports share unchanged filter-choice
  arrays without reusing stale totals, send records or missing projections.
- Existing setups reveal navigation before the first runtime snapshot. Settings
  and Help remain usable while it loads; data pages show a retryable failure
  instead of empty inventory, without recording failed startup as data-ready.
- Hidden windows coalesce activity until visible instead of rendering on a 16 ms
  timer. Activity-only updates reuse account-label projections, and startup no
  longer fetches and retains an unused all-time Usage report.
- Closed action menus mount their contents only when opened, reducing hidden UI
  work while preserving keyboard and dismissal controls.
- Removes the retired personal-server runtime and Desktop management client,
  including deployment, CI and compatibility contracts. Computer/LAN access and
  the separate Hosted development remain. Saved account ownership is retained
  privately for explicit local recovery, never automatic reactivation.
- Startup uses a themed, localized loader with real loading states and a reload
  action for slow or failed starts. The window opens before React loads;
  secondary pages load on navigation intent instead of competing with startup.

- Pool rotation separates presets into a side panel and groups participants
  into compact cards. Selected buttons use fill instead of colored borders;
  narrow windows keep every control available in a single scrolling area.
  The header close button replaces the redundant footer action.
- Access combines Local API, Applications, and Accounts settings. Application
  setup saves an executable path without launching or changing the pool; protected
  Claude restore lives in Recovery. Shared account settings retain per-account
  overrides and existing launch opt-outs; timezone follows the proxy automatically.
- Pages, pickers, dialogs, source cards, and saved proxies use compact layouts.
  Source cards show readiness and balances, not account rotation; Launch sits
  beside Edit. Proxy checks retain exit IP, country, latency, and check time,
  automatically binding the exit IP's validated timezone without manual setup.
  macOS uses native window controls and the system move/resize menu.
  ([4cb1d2d](https://github.com/F0RLE/zenith-relay/commit/4cb1d2d),
  [#85](https://github.com/F0RLE/zenith-relay/pull/85))
- Sign-in shows one saving status and closes on success; failed saves remain
  explicit. Initial model/quota checks run concurrently and pool membership stays
  opt-in. Import/export supports Cockpit Tools JSON and account metadata.
- Deleted accounts enter persistent trash. Restore leaves them disabled and out
  of the pool; permanent deletion needs confirmation, and older servers without
  trash support cannot silently turn deletion into a permanent action.
- Usage reports group timing, tokens/cache, attempts, provider IDs, and errors,
  with period/member/model filters and no empty sections. Debug context comparison
  keeps conversation text out of storage. BPS cards show expiry/recovery hints;
  English and Russian Help cover chart legends, BPS sign-in requirements,
  setup, recovery, and error codes.
- Passwords and 2FA secrets have individual reveal controls and stable save
  feedback. Changing a secret clears its previous generated code.
- Request context details compare client and prepared instruction/history/tool
  JSON sizes without storing content or inventing token estimates. BPS catalog
  preparation avoids repeated shared schema guidance and redundant sorting copies;
  ordinary text replies no longer warn about unused tools in automatic mode.

### Fixed

- Windows client restarts no longer print garbled localized `taskkill` output.
  Restart timeouts retain exit or OS error codes in diagnostics; process targets
  and the existing force-stop fallback are unchanged.

- Reused tool-call IDs from another conversation no longer reject an exact owned
  Responses continuation just because full materialized replay is unavailable.
  Bounded hashed ownership keeps caller/model/route checks and expiry intact;
  reused response IDs replace, rather than accumulate, call permissions.

- SIWC now applies the shared scoped cache-key fallback to requests without a
  client key, including the WebSocket-to-HTTP bridge. Explicit cache keys, modes,
  TTL and reasoning settings remain unchanged; this does not guarantee cache hits.

- BPS guidance uses separate native calls for scripts, patches and framed
  arguments instead of encouraging nested batch JSON. Parallel calls and simple
  batches remain supported; malformed executable payloads still fail closed.

- Responses continuations no longer reject a valid tool result when another
  conversation reuses its call ID. The exact predecessor and retained history
  disambiguate the call; key, model and route isolation remain enforced.

- Quota countdown automations verify current quota when saved or enabled and
  recover already-full windows without repeating a completed cycle. A new reset
  is recognized even when quota stays rounded to 100%; canceling an unstarted
  reservation no longer blocks an edited or re-enabled rule.

- Quota reset checks no longer disappear when the countdown reaches zero during
  a read. Overdue checks use the shared refresh job; verified windows do not
  trigger a refresh loop, and provider retry limits remain in force.

- Account indicators now distinguish quota/model refresh failures from blocked
  execution. Connections and Pool show temporary connection waits consistently,
  retain terminal errors, and identify quota timeouts in their tooltips.

- Usage no longer reloads its report for unrelated account/quota state events.
  History and pricing updates refresh in place without a recurring spinner or
  pagination lock; retention and hidden-view invalidation remain event-driven.
- Usage waits for the pending native report and runs only the latest filter or
  history intent; Usage and Overview discard queued work on navigation, and
  Usage re-entry shares pending reads.
  Unchanged request/error rows skip rendering; normal manual refresh is removed,
  with retry retained for errors. Late filter replies cannot replace newer choices.
- Folder, export, preset-preview and key-copy actions no longer reload the full
  runtime. Routing fallback no longer duplicates pending or just-completed reads;
  hidden windows defer queued refreshes. Settings reuse known storage paths, and
  network observation no longer restarts for unrelated interface actions.
- Long native/BPS conversations can use the existing 16 MiB replay budget
  instead of losing continuation above 2 MiB. If replay is unavailable,
  WebSocket clients receive the standard full-history recovery code; the
  internal HTTP bridge preserves the original Relay error category.
- Switching from the Relay pool to a native Codex account also updates chat
  metadata in differently named root-level history databases and Desktop
  catalogs. Old chats no longer retain a removed Relay provider solely because
  the database filename changed; unrelated rows and databases remain untouched.
- BPS tool-result continuations restore owned history before preparing the route,
  avoiding false model/source mismatches and unnecessary account changes. Native
  replay no longer retains unused prior tool catalogs or request options that
  could crowd valid history out of its existing memory budget.
- WebSocket continuation ownership failures immediately before sending a turn
  now persist in Desktop SQLite as local Relay errors, without recording a
  provider attempt or tokens. Zero-send support exports no longer mark their
  empty attempt history as missing. Ownership checks remain unchanged.
- Cloudflare 1010 no longer changes credential health; explicit internal WS 403
  refusals are distinct terminal transport errors. Valid JSON-before-SSE errors
  retain their first classification. New tests cover partial usage, peer cancellation
  and route-health isolation without changing rotation policy.
- Explicit unknown provider tiers remain unpriced instead of borrowing Standard
  rates. Raw tier names survive aggregation; retained history is reclassified
  once without rewriting archived evidence or older migrations. Context-price
  overlays preserve missing fields and explicit zeroes consistently.

- Source price editors use their own resolved quote with provenance in tooltips
  instead of borrowing a pool member's price. Only evidenced price schedules
  and schedule-specific input thresholds appear; mixed cache-write components
  stay visible, and USD-per-million units precede the rows.

- Cache-write estimates retain explicitly published generic creation prices
  instead of treating them as an unrelated cache window. Request totals and
  history use the same catalog rates; the price editor no longer assumes a
  retention duration for a generic write price.
- Compatible catalogs no longer inherit Anthropic cache windows from their
  protocol or an unrelated provider. Ambiguous cache-read window prices stay
  unknown, and DeepSeek cache-hit usage survives native and converted responses.

- SIWC tool requests include the required developer role on generated
  `additional_tools` input items, preventing upstream `input[0].role` rejections
  without changing tool names, call history or retry behavior.

- Missing SIWC plan/quota data uses linked Codex/BPS values in the existing badge
  and meters, without an extra panel. Compact indicators retain source, time and
  staleness details; stored metadata, restrictions and routing remain separate.
  Manual quota refresh adds no background polling.
- Selecting an unconnected Codex or Excel/BPS route normally can use the card's
  saved notes for automatic sign-in on Windows, macOS and Linux. The route menu
  no longer uses right-click for sign-in. Missing notes fall back to manual
  sign-in; SIWC stays manual and client credentials stay separate.
- Every sign-in method exposes copy-link recovery. Redundant purpose and
  automatic-login descriptions are removed from the sign-in dialog.
- Usage's last-24-hours filter now ends at the current time instead of selecting
  the local calendar day, including across midnight and daylight-saving changes.
- Adding another sign-in method to an account shows its saved notes instead of
  an empty draft, without reusing a different client's grant. The redundant
  account-connections menu and dialog have been removed.
- Runtime HTTP clients no longer resend protocol-level refusals invisibly inside
  one Relay attempt. Existing retry eligibility and candidate budgets remain authoritative.
- BPS resolves up to four image uploads concurrently, preserving bytes, input
  order, cancellation and account/credential-scoped coalescing. Failures are not cached.
- Usage rows keep short status labels; hover or keyboard focus reveals the reason.
- Repeated BPS catalog selections reference earlier lookup results instead of
  appending duplicate full schemas. History, current validators and generation budgets remain intact.
- Completed BPS catalog lookups remain reusable after a later answer fails
  or the dispatch budget is exhausted, including streaming replies.
  An explicit retry with the same validated history can preload the current
  tool contracts rather than repeating discovery over the full conversation.
  Budget exhaustion no longer emits a duplicate usage event for the last lookup.
  History, validators, permissions and no-replay protections stay unchanged.
- BPS failures during local tool discovery retain redacted provider error details
  instead of replacing them with a generic rate-limit message in request history.
  Buffered and streamed responses keep their existing no-replay safety checks.
- Structured-output requests skip incompatible Excel/BPS routes before account
  reservation, preserving the JSON schema on a compatible
  native route. Unsupported BPS-only requests still fail without weakening
  command approval checks.
- Pool activity retains each member's latest event when events from different
  members arrive out of order, so completed requests no longer stay marked active.
- Proxy storage can clear a configured common proxy after confirmation without
  deleting stored entries or explicit account bindings. Direct-access restrictions
  and protected-proxy deletion checks remain in force.
- Late expiry or rejection of an access-only token no longer marks a newer login
  as expired or replaces an existing sign-in error.
- Stopped or finished local API listeners release references to published routes
  and storage even while idle connection subscribers remain. Already-dispatched
  requests retain their own state during shutdown.
- **Launch application** on Overview now opens ChatGPT, Claude Desktop or OpenCode
  without connecting it to Relay, changing its configuration or restarting a
  running client. Launch is available with the API stopped and an empty pool.
- External account/profile changes no longer send ordinary launches through
  Relay's profile checks or runtime refresh. New manual ChatGPT context values
  remain untouched instead of blocking catalog updates and reconnection.
- Launcher process checks no longer accept `OpenAI.Codex.exe` by name alone or
  treat Claude's Chromium helpers as running desktop clients or stop targets.
- The legacy saved-key launch reports errors without hiding Relay. OpenCode
  restarts, including direct-source launches, no longer block the UI thread.
- Invalid ChatGPT launch overrides are checked even if the client is already
  running. Invalid `OPENCODE_BIN` / `CLAUDE_DESKTOP_BIN` overrides fail explicitly
  instead of silently launching a different installation.
- OpenCode confirms a running desktop before reporting launch success. Automatic
  discovery skips CLI entries throughout PATH, selects Claude versions by numeric
  order, and rejects non-executable desktop files before closing a working client.
- Native BPS keeps named Responses function outputs without a call ID, fixing
  thread startup and handoffs without inventing a matching tool call. Invalid
  supplied IDs and custom outputs without IDs remain rejected.
- Claude Desktop no longer hides supported `high` or `max` effort when an
  intermediate level is absent. The native menu cap and exact route validation
  stay separate; unsupported choices are never silently remapped.

- OpenCode profiles explicitly hide unsupported built-in reasoning variants so
  the SDK cannot offer Medium to a route that only accepts other effort levels.
- Claude Desktop requests routed to Gemini retain opaque user attribution through
  Gemini safety labels instead of failing on Messages metadata before replying.
- Converted Responses requests accept a function result's optional name when it
  matches the recorded call. Standalone outputs without `call_id` report that
  unsupported mapping instead of a generic input failure; no call is invented.
- Chat conversion preserves refusal history through the next user turn and
  pairs historical function results by name rather than call ID. Unrepresentable
  participant names and foreign Messages reply roles no longer disappear silently.
- Responses WebSocket requests discover BPS-only models when access is cold or
  expired, including later turns. Excluded native accounts are not contacted;
  expired access is revalidated and revoked model access remains blocked.
- Messages `pause_turn` no longer becomes a fabricated token-limit result on a
  converted route. JSON conversion rejects foreign Responses output roles and
  invalid Chat choice indexes instead of silently changing response identity.
- Converted output formats reject misplaced schema controls, competing schema
  envelopes, invalid names, and wrong-type Gemini MIME values instead of
  silently weakening the request. Messages/Gemini preparation avoids an unused
  copy of the complete response schema.
- Image continuation fingerprints retain non-default detail, preventing an
  edited history from restoring older native state. Invalid or misplaced
  detail controls fail instead of silently becoming automatic detail.
- Chat/Messages conversions reject foreign tool-choice shapes and unexpected
  fields instead of silently dropping controls. Messages `tool_choice: none`
  no longer carries an unsupported parallel-use modifier.
- Converted Chat Completions streams validate and respect `include_usage`
  without disabling internal accounting. Native usage capture ignores data after
  its terminal marker while retaining Gemini's separate trailing usage.
- Usage separates per-request cache reads/writes from retention estimates,
  preserves reported zeroes, and isolates cache activity by chat, connection,
  and upstream model. Misses do not extend the estimate; unknown and mixed
  windows no longer inherit a guessed or maximum lifetime.
- Desktop windows can be moved by dragging the existing title bar region.
  ([#88](https://github.com/F0RLE/zenith-relay/pull/88))
- Interrupted Responses streams report an explicit failure after complete SSE
  frames and retain event sequencing. They never insert another event into a
  forwarded partial frame or treat disconnection as successful completion.
- Conversion preserves explicit token limits, typed empty custom-tool results,
  and instruction order. Conflicting Gemini schemas/Parts, malformed tool values,
  and unsupported caller/namespace changes fail instead of silently losing data.
- BPS 401 renews access before the next request without repeating the failed
  generation. Late or temporary refresh failures preserve newer credentials,
  operator disable/drain state, and pool membership.
- BPS 429 pauses the affected connection and respects Retry-After without
  extending cached pauses on reads. Model-access refusals pause only that model
  and its aliases for at least 30 minutes; temporary access failures no longer
  masquerade as unknown models.
- BPS normalizes compact histories and keeps foreign client metadata local.
  Invalid-body 422 and completed-response conversion failures do not rotate or
  mark healthy accounts unhealthy. Direct single-tool replies are supported;
  invalid terminal tool output cannot trigger another generation.
- Excel requests have a default 120-second upstream-progress timeout; heartbeats
  do not extend it and valid terminal events do not wait for connection closure.
  Partial failures retain reported usage and real first/total timings, while
  unknown timing stays explicit. BPS access and encoding share reasoning aliases.
  ([#84](https://github.com/F0RLE/zenith-relay/pull/84))
- Generic rejections and turn limits no longer cause account-wide rotation or
  tool-history repair. Rejected encrypted compaction checkpoints remain intact.
  Native ChatGPT turn/WS state stays bound to its endpoint and authorization;
  transport retries share the original request budget.
- Delayed import, proxy, and timezone work cannot overwrite newer settings or
  remove credentials adopted by a completed import. Re-import preserves the
  selected proxy; replacing a sign-in blocks its previous authorization link.
- BPS quota countdown requests work with the local API stopped and wait for quota
  verification after successful generation. Unavailable saved models require a
  new selection instead of silently running another model.
- Monitoring distinguishes discovery/quota timeouts, redacts account identity,
  and retains safe provider IDs in client-visible errors. Usage filter resets,
  narrow badges/tooltips, keyboard focus, and temporary browser-storage failures
  no longer hide controls or erase saved preferences.

<!-- relay-notes:ru -->

Обновление добавляет профили пула для Claude Desktop, общий механизм
преобразования протоколов и корзину аккаунтов. Изменения сгруппированы по темам.

### Изменения

- Подготовка BPS повторно использует буферы текста при нормализации сообщений,
  добавлении инструкций и обработке видимого reasoning: без лишних копий,
  изменения байтов запроса, идентичности диалога или проверок.
- Вывод coding-инструментов сохраняется дословно при отправке в Gemini, включая
  JSON из файлов и ошибки команд. Преобразование Responses/Chat также сохраняет
  отдельные роли developer/system и положение инструкций в истории.
- Добавлены нативные endpoints подсчёта токенов Messages и Gemini для API-источников:
  с ограничениями локального ключа, без генерации ответа и выдуманных оценок.
  Неподдерживаемые источники возвращают явную ошибку.
- Веб-поиск Responses теперь работает через Claude Messages с сохранением
  источников, цитат и истории продолжения в JSON и потоке. Поддерживаются списки
  разрешённых доменов и примерное местоположение; неподдерживаемые настройки
  поиска и незавершённые циклы серверных инструментов возвращают явную ошибку.

- Оба Rust-пакета переведены на компилятор 1.99.1, Edition 2024 и стиль
  форматирования 2024. CI использует тот же компилятор и Bun 1.4.4;
  совместимые обновления зависимостей включают nanoid 3.3.20 и openssl-probe 0.2.2.

- Включён экспериментальный HTTP/3 для прямых HTTPS-запросов после ограниченной
  фоновой проверки без учётных данных. Первый запрос и неподдерживаемые адреса
  используют TCP без ожидания проверки; прокси и WebSocket сохраняют транспорт.
  Начатая генерация после ошибки не повторяется. `ZENITH_RELAY_HTTP3=0` отключает
  эксперимент; в диагностике видна фактическая версия HTTP ответа провайдера.
- Зависимости Desktop, веб-превью и Rust обновлены до актуальных стабильных
  версий, включая Tauri, React, Tokio, HTTP/TLS, проверку схем и SQLite.
  Двухпроходная минификация Terser сохраняет прежние лимиты размера JavaScript,
  отложенную загрузку страниц и совместимость WebView с ES2020.
- После повторных таймаутов необязательного WebSocket-подключения маршрут
  временно использует HTTP без повторной двухсекундной задержки на каждом
  запросе. Одиночный таймаут не мешает следующей попытке WebSocket;
  владение продолжениями, состояние аккаунта и правила повторов генерации сохранены.
  Пять запросов в синтетическом loopback-тесте ускорились с 10,19 до 4,16 с:
  измеряется устранение зависших подключений, а не скорость генерации провайдера.

- Подготовка BPS использует историю запроса и разобранный каталог без повторного
  копирования. Диагностика отдельно измеряет сериализацию JSON и подготовку
  каталога, а также сохраняет фактическую версию HTTP ответа провайдера.

- Остаток недели считается по общей сохраняемой калибровке сопоставимых подписок
  и нагрузки, а не по отдельной истории каждого аккаунта. Связанные подключения
  входят в итог пула один раз; без данных показано `—`. В подсказке видны профиль
  и число замеров. Разные нагрузки используют общую медиану, без диапазона. В карточках
  сохранён порог покрытия ценами 80%; показаны покрытие и число подписок. Обзор
  по-прежнему требует полных цен. Фильтр периода не меняет общую окупаемость;
  фактическое использование и маршрутизация не изменены.

- При временном отказе доступа BPS подключение Codex сохраняет формат каталога.
  Недоступные модели больше не показываются как повреждённый каталог или
  недоступный адрес API; после успешной проверки доступа каталог восстанавливается
  без перезапуска Relay.

- HTTP, сжатие контекста и WebSocket используют полный Responses по умолчанию,
  без автоматического выбора Lite из каталога моделей. Каталог и сохраняемый
  профиль Codex также выбирают полный Responses, чтобы клиент сам не включал Lite.
  Явные запросы клиента на Lite поддерживаются; схемы инструментов, рассуждение
  и политика параллельных вызовов сохранены. При явно выбранной смене режима
  Lite/полный WebSocket
  переподключается до отправки продолжения.

- Сборка объединяет сразу нужные модули интерфейса, сохраняя отдельный нативный
  загрузчик и отложенные страницы. Ради номера версии больше не включается весь
  пакет разработки; удалены неиспользуемые переводы. Лимиты размера сохранены.
  Диагностика запросов использует общую разметку полей; убраны неиспользуемые
  обёртки команд и повторные пояснения кнопки перезагрузки. В Использовании
  Gemini Interactions одинаково подписан в протоколе и конечной точке.

- Тексты Desktop, веб-демо и справки описывают функцию и действие короче.
  Из документации и комментариев убраны повторы и сравнения с отвергнутыми
  вариантами; рабочие ограничения и предупреждения сохранены.
  Значок предупреждения о текстовом доступе Codex выровнен по центру заголовка окна.

- В Обзоре появилась история недельной квоты в API-эквиваленте по подпискам и
  профилям нагрузки: медиана, разброс и отметки сбросов. Фоновый сбор не создаёт
  запросов к провайдеру, не дублирует связанные аккаунты и убирает устаревшие
  оценки. Стоимость подписки, маршрутизация и биллинг не изменены.

- Кэш продолжений Responses использует общую неизменяемую историю вместо
  копирования при каждом чтении; строковые префиксы сравниваются без временного JSON.
  Лимиты памяти, срок хранения, проверка владельца и отправляемая история сохранены.
- Общий SSE-парсер использует векторный поиск CR/LF для нативных и преобразованных
  потоков. Байты и порядок событий, проверки завершения и лимиты сохранены.
- Крупные HTTP-запросы JSON и сжатые тела разбираются в ограниченном пуле рабочих
  потоков. В синтетическом debug-замере на 1,72 МБ медиана самой долгой блокировки
  асинхронного потока снизилась с 58–59 мс до менее 0,05 мс.
  Небольшие несжатые запросы обрабатываются без переключения потока.
- Проверка модели и пересылка в нативном WebSocket используют общий результат разбора.
  Исходные текстовые и бинарные кадры, расход, ошибки и владелец продолжения сохранены.
  В синтетическом debug-замере время разбора текстовых и инструментальных дельт
  сократилось примерно на 49%.
- Преобразованные потоки проверяют растущий текст и аргументы инструментов без
  копирования предыдущего отправленного состояния. Текст, рассуждения, отказы
  и аргументы функций дополняют существующие буферы. Проверки префикса,
  нормализация чисел, идентичность инструментов и завершение сохранены.
- Нормализация аргументов функций пропускает текст в кавычках с векторным поиском
  разделителей. Обработка экранирования, точных чисел и незавершённых числовых
  значений сохранена.
- Пул и Подключения повторно используют индексы связанных аккаунтов и форматтеры
  времени при отрисовке карточек. Проверка доступности пула нормализует каталог
  моделей один раз для всех участников. Актуальность квот и маршруты проверяются заново.
- Отсчёты квот, подписок, входа и кэша запроса приостанавливаются в скрытом окне и обновляются
  при возвращении. Форматтеры дат общие для карточек и повторных тиков;
  смена языка учитывается сразу.
- В Использовании список, фильтры и статистика используют два общих индекса.
  Расчёт отдельно обрабатывает тайминги запросов и расход отправок.
  Обновление локальной базы сохраняет историю.
- Прогноз недели в карточках читает сохранённые замеры по индексам вместо
  повторного подсчёта истории запросов при обновлении квот аккаунтов.
- Графики использования считают расход и стоимость отправок за один проход.
  В шести чередующихся сравнениях на одной синтетической базе из 100 000 отправок
  медиана расчёта графика снизилась с 863 до 609 мс, с фильтром участника — с 284
  до 178 мс. Счётчики, учёт повторов и округление стоимости сохранены;
  отдельный проход расчёта цены и неиспользуемая вспомогательная функция удалены.
- Часовой пояс автоматически следует назначенному прокси: удалены выбор города,
  глобальные переключатели, ручные привязки и запасной Нью-Йорк. Общие фоновые
  задачи определяют выход без открытия UI, обновляют пояс до истечения 15 минут
  и повторяют сбои с паузой. Запросы не ждут геолокацию; без актуальных данных
  поля клиента сохраняются. Старые настройки очищаются при запуске, карточки
  получают новые результаты по объединяемым событиям.

- В настройке аккаунта ChatGPT кратко поясняются вход в Codex для управления
  с телефона и выполнение запросов через пул.

- Бейдж «Только изображения» остаётся в Пуле и выборе его участников, но не на
  карточках Подключений. Условия добавления и действия аккаунтов не меняются.

- Карточки аккаунтов в Подключениях используют те же шрифты, отступы шапки и
  толщину полос квоты, что и в Пуле; данные и действия аккаунтов не меняются.

- Массовые действия API-источников и прокси заменяют поиск и сортировку в той же
  строке, как в аккаунтах, а не сдвигают карточки вниз. После сброса выделения
  возвращаются прежние поиск и сортировка.

- Тесты скорости прокси выполняются по очереди вместо ошибки занятости.
  Карточки различают ожидание и измерение; очередь не расходует таймаут
  измерения и не создаёт конкуренцию за полосу скачивания.

- На одной локальной карточке можно явно объединить авторизации Codex, SIWC и
  Excel/BPS и выбрать маршрут в Аккаунтах или Ротации. Токены и квоты не смешиваются;
  переключение сохраняет вес/параллельность и доступно после завершения запросов.
  Проверяется вся группа; SIWC не подтверждает общую рабочую область или равные лимиты.
  Автоматизации следуют за маршрутом атомарно с проверкой доступности модели.

- Codex, ChatGPT/SIWC и Excel / Basis Points остаются отдельными
  способами входа. Экспорт/импорт Zenith сохраняет клиента и регистрацию SIWC;
  импорт SIWC требует повторного входа перед добавлением в пул. SIWC использует
  проверенные регистрации, защищённые ротируемые токены и публичный Responses
  HTTP/SSE. Неподдерживаемые возможности отклоняются; реальная проверка — отдельно.
- SIWC открывается в окне Tauri внутри Relay с отдельным профилем и прежними
  настройками прокси. Проверки OAuth и отмена остаются в Rust; ссылка для входа
  через браузер сохранена. Реальный вход во встроенном окне требует проверки.

#### Адаптеры протоколов и Excel / Basis Points

- BPS обрабатывает изображения в уже подготовленном запросе и сериализует его
  один раз перед отправкой, без повторных JSON-копий всей истории в обоих путях
  исполнения. Содержимое запроса, продолжение диалога, изоляция кеша и проверки
  инструментов сохранены; добавлен синтетический замер накладных расходов.
- BPS discovery не хранит вторую копию обычных текстовых/reasoning-дельт SSE,
  сохраняя завершённые элементы, usage, ошибки, модель/тариф и общий лимит ответа.
  Подготовка продолжения переносит исходный запрос вместо копирования истории.
- Сжатие запросов Codex использует общий буфер без полного копирования в
  асинхронном потоке. Диагностика разделяет подготовку сжатия и ожидание
  HTTP-заголовков; первый вывод остаётся отдельной отметкой. Воспроизводимые
  локальные замеры сжатия/передачи и их ограничения описаны в
  [Contributing](../../CONTRIBUTING.md#request-compression-probe). Поддержка
  провайдеров, содержимое запросов и правила повторов не меняются.
- Регрессионные тесты покрывают перемежающиеся вызовы, обратный порядок
  завершения, фрагменты UTF-8 и экранированные аргументы. Конвертация схем Gemini
  принимает уже принадлежащую ей схему без повторного клонирования; канонические
  названия типов остаются неизменными.
- API-источники с ключом могут явно использовать нативный Gemini Interactions
  через JSON/SSE. ID продолжений привязаны к клиентскому ключу, модели и маршруту;
  обрыв сохраняет известный usage и выдаёт нативную ошибку. Generate Content не
  означает поддержку Interactions. Агенты, фоновые задачи, получение сохранённых
  ресурсов и конвертация не входят; неподдерживаемые SDK OpenCode не получают
  такие модели в конфигурации.
- Мягкая привязка объединяет ветки по начальным инструкциям и сообщению, а
  наблюдаемые дочерние сессии Claude — по родителю. Явные ключи важнее; подсказка
  не даёт права использовать чужое непрозрачное продолжение.
- Desktop capabilities возвращает стабильные описания семейства и способа
  входа только для существующих OpenAI-подключений. Новые входы и их интерфейс
  на паузе. Обновление политик сравнивает уникальные ID источников независимо от
  порядка; повторная публикация runtime не отключает его listener и кандидатов.
- BPS сохраняет наблюдаемый контекст нового чата Codex без выдуманного вызова
  инструмента или смены метаданных хода; лишнее скрытое содержимое отклоняется.
- Продолжения после инструментов больше не теряют исходное состояние, если
  клиент переставил поля JSON-аргументов, в том числе вложенные параметры
  файлового инструмента, заменил экспоненциальную запись целого числа обычной или
  разбил соседний текст одной роли на сообщения. Меняются только копии для
  сравнения; массивы, точные значения, отрицательный ноль, нативные аргументы
  и reasoning сохраняются. Регрессионные тесты падают до правок и проходят после;
  цепочки файловых инструментов JSON/SSE восстанавливают состояние после
  пересериализации клиентом, а изменённая история отклоняется до отправки.
- BPS принимает JSON-схему с явно нестрогим режимом без специальных флагов клиента.
  Исходная схема проверяется до выдачи JSON или потока; неверный или незавершённый
  ответ завершается ошибкой без повторной генерации. Для нативного строгого
  режима по-прежнему нужен совместимый маршрут.
- Управляемые алиасы Codex убирают подставленное `none` у thinking-моделей без
  опубликованных режимов усилия. Native Responses восстанавливает продолжение
  по сохранённой истории после явного отказа сервиса без хранения ответов.
- В деталях запроса отдельно показана модель из ответа источника — без выдачи
  её за подтверждённую личность модели. Ошибки BPS-инструментов поясняют этап
  проверки и запрет перегенерации исполняемых вызовов. При неоднозначном отказе
  несколько зашифрованных reasoning-элементов больше не удаляются массово.
- Для native HTTP добавлен общий для попыток срок ожидания заголовков,
  выключенный по умолчанию. Он не ограничивает тело ответа, BPS и native WS;
  истечение срока не повторяет потенциально принятый запрос и не отключает аккаунт.
- BPS больше не меняет стабильный префикс каталога и истории при смене настроек
  вызова инструментов, включая `none`, выбор функции/custom и `allowed_tools`.
  В синтетическом сравнении до/после сохраняются все четыре начальных элемента
  вместо нуля: 38 407 байт JSON в compact и 34 177 в discovery — это не токены
  и не измеренный кеш-хит провайдера. Небольшие выбранные схемы загружаются сразу
  в прежних лимитах; checkpoint использует готовый пролог без повторного рендера.
  Для аккаунтных Responses без клиентского ключа кэша
  используется непрозрачный ключ контекста, одинаковый в HTTP и WebSocket;
  явные ключи, тела запросов к API-источникам и ограничения ротации сохранены.
- Диагностические отпечатки контекста используют один ограниченный буфер
  сериализации; дайджесты и лимиты байт/элементов не меняются. Локальные замеры
  до/после и компромиссы времени подготовки описаны в
  [Contributing](../../CONTRIBUTING.md#protocol-optimization-probes). Рост кеш-хита,
  ускорение первого токена и снижение стоимости требуют отдельной live-проверки.
- BPS передаёт сложные многострочные аргументы функций в явном обрамлении без
  вложенного JSON-экранирования; исходные схемы и сырые входы custom-инструментов
  сохраняются. Локальные ошибки стрима используют поля Responses и правильную
  нумерацию событий вместо вложенной HTTP-обёртки.
- BPS сохраняет буквальные переносы строк и табуляции внутри вложенного JSON
  аргументов функций вместо отказа всего ответа. JSON и поток используют один
  декодер; повреждённый синтаксис и нарушения схемы по-прежнему отклоняются
  без повторной генерации.
- BPS распознаёт уже поддерживаемую совместимую форму одного вызова и с явной
  ссылкой на инструмент, проверяя точное совпадение имени и исходную схему.
  При смешении старого и нового формата снимается только одна обёртка того же
  инструмента: поля не отбрасываются, корректные аргументы и custom-входы не меняются.
  Диагностика отличает синтаксическую ошибку JSON от преждевременного обрыва,
  не сохраняя аргументы и не запуская автоматическую перегенерацию.
- Ошибки JSON в BPS содержат безопасный код причины парсера, категорию формы
  входа, размер и координаты ошибки без содержимого аргументов. Эти сведения
  сохраняются и при поиске инструментов — в ответе клиенту и диагностике
  запроса. Некорректные вызовы по-прежнему не выполняются.
- BPS сохраняет обратный слеш в неизвестных печатных escape-последовательностях
  строк аргументов, не отклоняя весь ответ из-за регулярного выражения или
  команды. Обычные JSON-экранирования не меняют смысл; повреждённый Unicode,
  структура и нарушения схемы по-прежнему отклоняются. Custom-входы и явное
  обрамление передаются без изменений.
- Локальные ошибки адаптера в обычных HTTP-ответах содержат
  `x-should-retry: false`: учитывающие его клиенты не повторяют генерацию
  автоматически. Статус HTTP и учёт использования не меняются; явная повторная
  отправка клиентом остаётся возможной.
- Удалённый из результатов клиентского поиска отложенный инструмент больше
  не включается снова из-за старых вызовов. Преобразуемые ответы Chat/Messages
  отклоняют противоречивую причину остановки в JSON/SSE вместо успешного завершения.
  Обязательный или конкретный выбор инструмента также исключает обычный успешный
  ответ без вызова, сохраняя смысл отказов и остановки по лимиту токенов.
- Клиентский tool search Responses работает на преобразуемых JSON/SSE-маршрутах:
  отложенные схемы, пространства имён, парные результаты и следующий ответ.
  Результаты поиска не дублируют полные схемы во входе чужой модели.
- PDF в base64 сохраняет байты и подписи во всех двенадцати преобразованиях;
  Responses/Messages также передают URL PDF и PDF-результаты инструментов.
  ID файлов провайдера и непереносимые настройки рендеринга/цитат остаются нативными.
- Нативные продолжения Gemini сохраняют токены кандидатов и завершающий usage,
  привязаны к исходному владельцу/ключу/модели/contents и не перезапускают генерацию
  автоматически. Ссылки на кэш и серверные инструменты остаются нативными.
- BPS сохраняет причину ошибки аргументов в потоке вместо общей ошибки
  стриминга. Примеры упаковки включают многострочные команды с кавычками;
  все режимы каталога отклоняют повторное использование ID вызова из истории.
- Разбор SSE не копирует однострочные данные событий и не сдвигает буфер после
  каждого события при преобразовании протоколов, учёте исходного расхода
  и HTTP→WebSocket-fallback. Нативная передача дельт
  Responses использует проверенный быстрый разбор, в том числе для текста и
  аргументов с экранированием, без полного JSON-дерева;
  проверки использования и завершения ответа сохраняются.
  WebSocket не копирует обычные кадры и не разбирает их для уже запрещённого
  повтора; HTTP-fallback использует готовые события без копирования JSON
  и непрозрачных данных. Преобразователи не копируют
  неизменившиеся блоки текста и размышлений, сохраняя проверку закрытых блоков.
  Нативные снимки текста и размышлений Messages собираются при закрытии блока,
  а не копируются на каждой дельте; фрагменты подписи дописываются на месте.
  Responses дописывает фрагменты custom-input и недостающие суффиксы итогового
  текста без копирования накопленного префикса.
  Ключ истории также вычисляется без пересборки накопленного текста
  на каждом фрагменте.
  При завершении потока история и нативное состояние передаются в продолжение
  без копирования всего подготовленного запроса; границы Parts Gemini сохраняются.
- BPS группирует имена инструментов по типу и пространству имён; полные
  описания и схемы загружаются ограниченными локальными запросами Relay без
  ослабления проверки аргументов. Длинные описания и форматы custom-инструментов
  тоже учитываются при автоматическом выборе поиска. Схемы использованных
  инструментов сохраняют компактное представление после истории. В деталях видны
  загруженные схемы и расход поиска. Текст до ожидающих вызовов передаётся потоком,
  а сами вызовы выдаются только после проверки. Нативный поиск инструментов
  не подменяется.
- При подготовке больших диалогов BPS и продолжений между протоколами Relay
  использует меньше временной памяти для отпечатков истории. Идентификаторы сессии,
  инструкции клиента, история и уровень размышлений не меняются.
- Responses, Chat Completions, Messages и Gemini используют общий механизм
  преобразования. Сохраняются поддерживаемые идентификаторы и пространства имён
  инструментов, текстовые результаты и изображения, схемы, цитаты и правила
  параллельных вызовов. Неподдерживаемые возможности отклоняются явно; нативная
  передача сохраняет расширения провайдера, точные числа, порядок полей и заголовки.
  ([1f64175](https://github.com/F0RLE/zenith-relay/commit/1f64175))
- Начальные служебные инструкции после сообщения пользователя больше не блокируют
  преобразованные запросы Claude до первого ответа. Их порядок сохраняется в
  системном поле целевого протокола; инструкции после предыдущих ответов ассистента
  по-прежнему отклоняются явно, если целевой формат не сохраняет их положение.
- Настройки и отчёты размышлений используют единый `xhigh` и фактический уровень
  или бюджет. Клиенты Messages получают доступные публичные сводки и принимают
  пустую начальную подпись Claude с сохранением нативного состояния продолжения.
  Подписи чужого провайдера не подделываются.
- Excel/BPS поддерживает результаты поиска инструментов, function/custom,
  изображения в результатах и атрибуцию совместной работы. Текст поступает
  постепенно, инструменты — только после проверки завершения по текущему каталогу
  и исходным схемам. Необязательная буферизованная проверка JSON/схем выполняется
  локально и не является нативным строгим выводом провайдера.
  ([#84](https://github.com/F0RLE/zenith-relay/pull/84))
- ChatGPT и Excel / Basis Points имеют отдельный вход и обновление через выдавший
  токен OAuth-клиент. Нативные аккаунты ChatGPT работают напрямую или через пул.
  BPS работает только через пул, отклоняет известные Free-тарифы и поддерживает
  экспорт Zenith/Sub2API. Старый переключатель транспорта и ручной callback убраны.
- Доступ к моделям Excel обновляется до публикации каталога и выполнения запроса.
  Отключённые, ограниченные и заблокированные правилами модели исключаются;
  порядок провайдера сохраняется. Кэш доступа привязан к текущему токену,
  чтение ограничено таймаутом и размером ответа.
  ([#84](https://github.com/F0RLE/zenith-relay/pull/84))

#### Пул, маршрутизация и модели

- Автоматическая ротация сохраняет прогретый маршрут среди кандидатов при разнице
  нормализованной нагрузки в один запрос, не отправляя диалог сразу на холодного участника.
  Ёмкость, существенная разница квоты/кредитов, здоровье и непрозрачное владение сохраняются;
  независимые запросы распределяются по нагрузке без нового ожидания или смены политики кеша провайдера.
  [Синтетическое сравнение](../../CONTRIBUTING.md#cache-affinity-admission-comparison)
  фиксирует повторное использование маршрута и локальные расходы, а не реальный cache hit или экономию.
- Автоматическая ротация может передать диалог с полной историей инструментов
  более подходящему участнику. Непрозрачные продолжения и результаты без вызова
  сохраняют владельца, пока проверенное локальное восстановление не даст полную
  историю. Оба окна квоты ограничивают выбор; одинаковые остатки не гарантируются.
  Передача той же модели восстанавливает сохранённые вызовы инструментов
  и reasoning по `previous_response_id`, а не использует текстовую ветку смены
  модели. HTTP и WebSocket сохраняют ключ кеша и текущие настройки; без истории
  владелец не меняется. Исходный ciphertext передаётся без изменений: он не
  удаляется и не служит общим запретом ротации. Локальные тесты не доказывают приём
  между аккаунтами провайдера; попадания в кеш не подменяются.
- Обновление кредитов отделено от заголовков ответов. Нагрузка не делает старые
  кредиты свежими, не откладывает опрос бесконечно и не отменяет новые лимиты.
  Общий баланс ChatGPT учитывает последнее состояние одного счёта провайдера один раз.
- API-маршруты выбирают нативный протокол известной модели независимо от клиента.
  Выбор SDK OpenCode использует ту же справочную политику; неподдерживаемые
  эндпоинты исключаются. Смена модели сохраняет завершённую историю, но не переносит
  активное провайдерское состояние другой модели.
- Контекст и сжатие настраиваются независимо. Опубликованные Short/Long и нативные
  значения аккаунта сохраняются; резервные 25% применяются только без известного
  значения по умолчанию. Лимиты API остаются обязательными, длинный контекст включается
  явно. Сохранения последовательны, предупреждения не перезапускают приложения.
  ([#84](https://github.com/F0RLE/zenith-relay/pull/84))
- Каталоги используют общий порядок по разработчику, семейству и версии, ручные
  исключения и публичный экспорт политики. Получение моделей сохраняет приоритеты
  провайдера и корректный кэш при ошибках. Каталоги ChatGPT/Codex принимают до 4 МиБ
  и один раз повторяют временный сетевой сбой с задержкой.
  ([#86](https://github.com/F0RLE/zenith-relay/pull/86))
- Скорость сохраняется при повторах. Цены разделяют Standard, Flex, Fast, Ultrafast
  и опубликованные диапазоны длинного контекста, включая нулевые цены. Оценка
  использует фактический вход запроса; отсутствующие цены остаются неизвестными.
- Настройка API объединяет пресеты провайдеров и ссылки на консоли, отделяя
  Kimi Code от API-кошелька Moonshot. Балансы Moonshot сохраняют региональную валюту
  USD/CNY; известные сервисы без чтения баланса не получают общие биллинг-пробы.

#### Claude Desktop и профили

- Фиксированная шкала размышлений Claude Desktop сопоставляет пропущенные
  уровни с ближайшим поддерживаемым и разрешённым уровнем снизу либо минимальным
  разрешённым, если ниже ничего нет. В использовании остаются запрошенный и
  фактический уровни; выключение, явные бюджеты, прямой API и другие клиенты не меняются.

- Claude Desktop подключается через профиль стороннего провайдера. Relay проверяет
  настройки до закрытия клиента, сохраняет восстановление, записывает каталог
  и запускает приложение только по выбранному действию. Восстановление сохраняет
  чужие профили и новые ручные изменения. Прямой ChatGPT получает нативный каталог,
  профили пула — каталог Relay.
- Каждая модель Claude Desktop показана один раз со стабильным клиентским ID.
  Встроенный Effort выбирается отдельно в пределах возможностей маршрута и правил
  пула. Названия, реальные модели провайдера, контекст и цены не меняются;
  слишком большой каталог отклоняется до изменения профиля.
- Инструменты MCP загружаются по требованию. Мост Messages принимает проверенные
  загруженные определения, ссылки поиска, метаданные и рекомендательные указания
  кэша, но не имитирует серверный поиск или цены кэша Anthropic у другого провайдера.
  Для применения настройки профиля переподключитесь и начните новый диалог.

#### Интерфейс и Доступ

- Этапы медленных снимков состояния записываются одной надёжной транзакцией SQLite
  вместо отдельного сохранения каждого этапа. Проверки, срок хранения и одиночные
  метрики сохранены; при ошибке пакет откатывается целиком.
- Пул и подключения индексируют кандидатов один раз на снимок вместо полного
  обхода маршрутов для каждой карточки. Сортировка моделей определяет ID/группы
  один раз, сравнивает числовые позиции и переносит исходные строки в результат;
  ручной порядок, группы провайдеров и правила маршрутизации сохранены.
- Первый вход в «Использование» сортирует ключи отправок и загружает подробности
  только выбранной страницы вместо всей истории. Облегчённые запросы итогов/фильтров
  и индексы сокращают работу базы; чтение отчёта вынесено с потока окна.
- «Использование» загружает только выбранную страницу и повторно использует
  просмотренные, пока данные не изменились. Итоги за период появляются без отдельной
  кнопки, а списки фильтров из истории — по действию пользователя. Скрытые
  и неактивные страницы сбрасывают устаревший кэш без фонового чтения отчёта.
- Нативная пагинация «Использования» повторно использует ограниченный кэш итогов
  без подробностей соседних страниц. История и цены сбрасывают кэш; свежесть каталога
  обновляется и без изменения цен.
- Неизменившийся каталог повторно использует общие деревья моделей и индексы.
  Производный объединённый JSON больше не сохраняется; компактные внутренние
  сведения об источниках полей сохраняют публичные доказательства и конфликты.
- При восстановлении «Использование» читает только выбранный сохранённый отчёт,
  а не все восемь в память интерфейса. BPS сначала разрешает замены объявлений
  каталога и повторно использует успешную подготовку неизменных схем;
  аргументы вызовов и текущие разрешения по-прежнему проверяются полностью.
- Закрытие сведений о запросе не перерисовывает журнал использования. Подсказка
  больше не перехватывает первое Escape окна, а закрытие по фону сохраняет фокус
  на открывшей его кнопке без смены фокуса сквозь исчезнувшее окно. Только это окно
  сохраняет затемнение без размытия всего фона; фокус возвращается после возможности
  отрисовать закрытие, без измерения пассивной подсказки и перехвата нового фокуса.
  При открытии история кэша читается только для выбранных чата, участника и модели
  в рабочем потоке, без блокировки окна; технические детали строятся при раскрытии.
- Вход в «Использование» больше не блокирует окно, когда запись замера открытия
  ждёт базу истории. Замеры интерфейса записываются в рабочем потоке с прежними
  правилами проверки и сохранения.
- Чтение сетевых интерфейсов, настроек, резервных копий и корзины, очистка истории
  и запись диагностики больше не выполняются в потоке окна. Переход между пулом
  и подключениями сохраняет текущее чтение ротации и резервный таймер вместо
  запуска повторной работы.
- Обзор показывает готовую аналитику до сохранения её кеша, объединяет записи
  и сохраняет ожидающие данные при скрытии или закрытии страницы. Активность пула
  больше не сериализует повторно неизменный список статистики API; новые снимки
  состояния и явные обновления обрабатываются по прежним правилам.
- Usage объединяет ввод ID запроса в одно чтение после паузы и отменяет незавершённый
  поиск при очистке или смене вкладки. Отчёты используют общие неизменные списки
  фильтров без подмены свежих итогов, отправлений или отсутствующих проекций.
- После первоначальной настройки навигация появляется до первого снимка состояния.
  Настройки и помощь доступны во время загрузки; страницы данных предлагают повтор
  при ошибке вместо пустого списка. Ошибка запуска не отмечается как готовность данных.
- Скрытое окно объединяет активность до возвращения вместо перерисовки по таймеру
  16 мс. Обновления нагрузки используют готовые подписи аккаунтов, а запуск больше
  не загружает и не удерживает неиспользуемый отчёт Usage за всё время.
- Закрытые меню действий создают пункты только при открытии, уменьшая скрытую
  работу интерфейса; управление с клавиатуры и закрытие сохранены.
- Удалён старый личный сервер и его клиент управления, включая развёртывание,
  CI и контракты совместимости. Режим «На устройстве», локальная сеть и отдельная
  разработка Hosted сохранены. Прежнее владение аккаунтами сохраняется локально
  для явного восстановления, без автоматического включения.
- Новый экран запуска учитывает тему и язык, показывает этап загрузки и
  позволяет перезагрузить окно при задержке или ошибке. Окно открывается до
  загрузки React; остальные страницы подгружаются при переходе, а не все сразу.

- В окне ротации пресеты вынесены на боковую панель, а участники собраны
  в компактные карточки. Выбор выделяется заливкой, а не цветной обводкой;
  в узком окне все настройки доступны в одной области прокрутки.
  Закрытие оставлено в заголовке без дублирующей нижней кнопки.
- Доступ объединяет Локальный API, Приложения и Аккаунты. Настройка приложения
  сохраняет путь без запуска и изменения пула; защищённое восстановление Claude
  находится в Восстановлении. Общие настройки аккаунтов сохраняют индивидуальные
  исключения и прежние отказы от автозапуска; часовой пояс автоматически следует прокси.
- Страницы, выбор приложений, диалоги, карточки источников и прокси стали компактнее.
  Источники показывают готовность и баланс, а не ротацию аккаунтов; Запустить стоит
  рядом с Изменить. Проверки прокси сохраняют IP, страну, задержку и время проверки,
  автоматически привязывая проверенный пояс выходного IP без ручной настройки.
  macOS использует нативные кнопки окна и системное меню перемещения и размера.
  ([4cb1d2d](https://github.com/F0RLE/zenith-relay/commit/4cb1d2d),
  [#85](https://github.com/F0RLE/zenith-relay/pull/85))
- Вход показывает одно состояние сохранения и закрывается при успехе; ошибка
  остаётся явной. Первые проверки моделей и квоты параллельны, участие в пуле
  включается отдельно. Импорт и экспорт поддерживают JSON Cockpit Tools и метаданные.
- Удалённые аккаунты попадают в постоянную корзину. Восстановленные остаются
  отключёнными и вне пула; окончательное удаление требует подтверждения. Старый
  сервер без корзины не может молча превратить удаление в безвозвратное.
- Отчёты объединяют время, токены/кэш, попытки, ID провайдера и ошибки без пустых
  разделов, с фильтрами периода, участника и модели. Сравнение контекста в отладке
  не сохраняет текст диалога. Карточки BPS показывают срок входа и восстановление;
  справка EN/RU объясняет графики, требования входа BPS, подключение,
  восстановление и коды ошибок.
- Пароли и секреты 2FA раскрываются отдельно, сохранение не сдвигает элементы.
  Смена секрета очищает ранее рассчитанный код.
- Сведения о контексте сравнивают размеры JSON инструкций, истории и инструментов
  клиента и подготовленного запроса, не сохраняя текст и не выдумывая токены.
  Подготовка каталога BPS не повторяет общие указания схем и не копирует их для
  сортировки; обычный ответ без инструментов в автоматическом режиме не вызывает предупреждение.

### Исправления

- При перезапуске клиентов в Windows больше не выводится искажённый текст
  `taskkill`. При таймауте коды завершения или ошибки ОС сохраняются в диагностике;
  выбор процессов и переход к принудительному завершению не изменены.

- Повторный ID вызова из другого чата больше не блокирует подтверждённое
  продолжение Responses только из-за отсутствия полной материализованной истории.
  Ограниченные хешированные данные владения сохраняют проверки ключа, модели,
  маршрута и срока; повторный ID ответа не накапливает права старых вызовов.

- SIWC теперь применяет общий резервный ключ кэша в рамках клиентского контекста,
  если ключ не задан, в том числе в мосте WebSocket → HTTP. Явные ключи, режимы,
  TTL и настройки reasoning сохраняются; это не гарантирует попаданий в кэш.

- BPS рекомендует отдельные нативные вызовы для скриптов, патчей и аргументов
  с raw-frame вместо вложенного batch JSON. Параллельность и простые пакеты
  сохранены; повреждённые исполняемые аргументы по-прежнему отклоняются целиком.

- Продолжение Responses больше не отклоняет корректный результат инструмента,
  если другой чат использовал такой же ID вызова. Вызов проверяется по владельцу
  предыдущего ответа и сохранённой истории; изоляция ключа, модели и маршрута сохранена.

- Запуск отсчёта в автоматизациях проверяет текущую квоту после сохранения или
  включения и подхватывает уже полное окно без повтора завершённого цикла.
  Новый сброс распознаётся и при округлённых 100%; отмена ещё не начатой задачи
  больше не блокирует изменённое или повторно включённое правило.

- Проверка сброса квоты больше не теряется, если таймер дошёл до нуля во время
  чтения. Просроченную проверку выполняет общая задача обновления; уже проверенные
  окна не вызывают цикл запросов, ограничения повторов провайдера сохраняются.

- Индикаторы аккаунтов различают сбой обновления квоты или моделей и блокировку
  запросов. «Подключения» и «Пул» одинаково показывают временное ожидание связи,
  сохраняют реальные ошибки и уточняют таймаут проверки квоты в подсказке.

- «Использование» больше не перечитывает отчёт при посторонних изменениях
  аккаунтов и квот. История и цены обновляются без постоянного индикатора загрузки
  и блокировки страниц; очистка старых записей и скрытые вкладки учитываются событиями.
- «Использование» дожидается текущего нативного чтения и выполняет только последний
  выбранный фильтр или обновление истории. Уход из «Использования» и «Обзора» отменяет
  отложенную работу; возврат в «Использование» использует незавершённое чтение.
  Неизменившиеся строки запросов и ошибок
  не перерисовываются. Обычное ручное обновление убрано; повтор остаётся при ошибке.
  Поздние ответы фильтров не заменяют свежие варианты.
- Открытие папок, экспорт, просмотр пресета и копирование ключа больше не
  перечитывают весь runtime. Таймер порядка пула не дублирует текущее или только
  завершённое чтение; скрытое окно откладывает обновления. Настройки сразу показывают
  известные пути, а посторонние действия не перезапускают наблюдение за сетью.
- Длинные нативные/BPS-диалоги используют существующий лимит истории 16 МиБ,
  без потери продолжения после 2 МиБ. Если история недоступна, WebSocket-клиент
  получает стандартный код для повторной отправки полного контекста;
  внутренний HTTP-мост сохраняет исходную категорию ошибки Relay.
- При переходе с пула Relay на нативный аккаунт Codex обновляются и метаданные
  чатов в базах истории с другими именами и каталогах Desktop в корне профиля.
  Смена имени базы больше не оставляет старые чаты с удалённым провайдером Relay;
  посторонние записи и базы не меняются.
- Продолжения BPS с результатом инструмента восстанавливают историю владельца
  до подготовки маршрута, без ложной ошибки модели/источника и лишней смены
  аккаунта. В памяти больше не хранятся неиспользуемые каталоги инструментов
  и параметры предыдущего запроса, вытеснявшие историю из существующего лимита.
- Ошибки владельца контекста WebSocket непосредственно перед отправкой запроса
  теперь сохраняются в SQLite Desktop как локальные ошибки Relay, без попытки
  провайдера и токенов. Диагностика не считает пустую историю отправок таких
  запросов потерянными данными. Проверки владельца не меняются.
- Cloudflare 1010 больше не меняет состояние авторизации; явный внутренний WS 403
  выделен в терминальную ошибку транспорта. JSON-ошибка перед SSE сохраняет исходную
  классификацию. Добавлены проверки частичного usage, отмены соседнего потока и
  изоляции здоровья маршрутов без смены политики ротации.
- Неизвестный тариф провайдера больше не считается по Standard. Названия новых
  тарифов сохраняются в агрегатах; доступная история переклассифицируется один
  раз без подмены архивных данных и старых миграций. Пороги цен одинаково
  обрабатывают пропущенные поля и явно нулевые ставки в редакторе и расчёте.

- Редактор цен использует расчёт выбранного источника; происхождение доступно
  в подсказке, без подстановки тарифа другого участника пула. Меню содержит только схемы
  с данными и их пороги входных токенов; общий тариф записи и тарифы отдельных
  окон кэша не скрывают друг друга, единицы USD за миллион видны до строк.

- Оценка записи в кэш учитывает явно опубликованный общий тариф, не подменяя
  его чужим окном хранения. Суммы запросов и история используют одинаковые
  цены каталога; редактор больше не приписывает общему тарифу срок хранения.
- Совместимые каталоги больше не наследуют окна кэша Anthropic по протоколу
  или чужому провайдеру. Неоднозначная цена чтения остаётся неизвестной,
  а cache-hit DeepSeek сохраняется в нативных и преобразованных ответах.

- SIWC передаёт обязательную роль developer в создаваемых блоках
  `additional_tools`: запросы с инструментами больше не теряют `input[0].role`.
  Имена инструментов, история вызовов и правила повторов не меняются.

- Если SIWC не передал тариф или квоту, данные Codex/BPS показаны на обычных местах,
  без отдельного блока. Компактные значки сохраняют источник, время и устаревание
  в подсказках; сохранённые данные, ограничения и маршруты не смешиваются.
  Ручное обновление квоты не добавляет фоновых опросов.
- Обычное нажатие на неподключённый Codex или Excel/BPS запускает автовход из
  заметок карточки на Windows, macOS и Linux. ПКМ в меню маршрутов больше не
  запускает вход. Без почты или пароля остаётся ручной вход; SIWC входит вручную,
  авторизации разных клиентов не смешиваются.
- Копирование ссылки доступно для всех способов входа. Лишние описания назначения
  Codex и автоматического заполнения убраны из окна входа.
- Фильтр использования «Последние 24 часа» отсчитывается от текущего момента,
  а не от полуночи, в том числе при смене даты и переходе на летнее время.
- При добавлении способа входа к аккаунту показываются сохранённые заметки,
  а не пустой черновик; авторизация другого клиента не переиспользуется.
  Лишний пункт «Подключения аккаунта» и его окно удалены.
- HTTP-клиенты больше не повторяют протокольные отказы незаметно внутри одной
  попытки Relay. Правила безопасного повтора и лимиты кандидатов сохраняются.
- BPS загружает до четырёх изображений одновременно, сохраняя байты, порядок,
  отмену и объединение одинаковых загрузок в рамках аккаунта/авторизации. Ошибки не кэшируются.
- В строках Использования остаётся короткий статус; причина видна при наведении или фокусе.
- Повторный выбор инструментов BPS ссылается на прежние результаты поиска вместо
  добавления полных копий схем. История, актуальная валидация и лимиты генераций сохраняются.
- Завершённый поиск каталога BPS остаётся пригодным для повторного использования,
  даже если последующий ответ завершился ошибкой или исчерпан лимит попыток,
  в том числе в потоке. При явном повторе с той же
  проверенной историей контракты инструментов загружаются заранее, без лишнего
  поиска на всей переписке. При исчерпании лимита последняя загрузка не создаёт
  дубликат события usage. История, валидация, права и защита от повторной генерации сохраняются.
- Ошибки BPS при локальном поиске инструментов сохраняют обезличенные подробности
  отказа провайдера в истории запросов вместо общего сообщения о лимите частоты.
  Защита от повторной генерации сохраняется для обычных и потоковых ответов.
- Запросы со структурированным ответом исключают несовместимые маршруты Excel/BPS
  до резервирования аккаунта. JSON-схема сохраняется на
  совместимом родном маршруте; неподдерживаемый запрос в пуле только из BPS
  завершается отказом, без ослабления проверки команд.
- Активность пула сохраняет последнее событие каждого участника, даже если события
  разных участников пришли не по порядку: завершённые запросы не остаются активными.
- В хранилище прокси можно явно сбросить общую настройку после подтверждения,
  не удаляя записи и явные привязки аккаунтов. Запрет прямого доступа и защита
  удаления используемых прокси сохраняются.
- Запоздалая ошибка или истечение токена без refresh token больше не помечает
  новый вход как истёкший и не подменяет уже установленную ошибку авторизации.
- Остановленный или завершившийся локальный API освобождает ссылки на маршруты
  и хранилище даже при оставшихся подписчиках простаивающих соединений.
  Уже отправленные запросы сохраняют своё состояние во время остановки.
- **Запустить приложение** на Обзоре открывает ChatGPT, Claude Desktop или OpenCode
  без подключения к Relay, изменения настроек и перезапуска работающего клиента.
  Запуск доступен при остановленном API и пустом пуле.
- После смены аккаунта или профиля вне Relay обычный запуск не зависит от
  проверок профиля и обновления состояния Relay. Новые ручные настройки контекста
  ChatGPT сохраняются и не блокируют обновление каталога и повторное подключение.
- Проверка процессов больше не принимает `OpenAI.Codex.exe` только по имени
  и не считает служебные процессы Chromium в Claude запущенным приложением
  или целью остановки.
- Старый запуск с сохранённым ключом сообщает об ошибке, не скрывая Relay.
  Перезапуск OpenCode, в том числе для прямого источника, не блокирует UI-поток.
- Неверный путь ChatGPT проверяется и при уже работающем клиенте. Ошибочные
  `OPENCODE_BIN` / `CLAUDE_DESKTOP_BIN` больше не подменяются другой установкой.
- OpenCode подтверждает появление процесса приложения перед сообщением об успехе.
  Автопоиск проходит мимо CLI по всему PATH, сравнивает версии Claude численно
  и отклоняет неисполняемые файлы до закрытия работающего клиента.
- Нативный BPS сохраняет именованные результаты функций Responses без ID вызова:
  запуск чатов и передача задач больше не требуют выдуманного вызова инструмента.
  Некорректные переданные ID и custom-результаты без ID по-прежнему отклоняются.
- Claude Desktop больше не скрывает доступные `high` или `max`, если отсутствует
  промежуточный уровень. Ограничение нативного меню не заменяет точную проверку
  маршрута; неподдерживаемый выбор не подменяется другим уровнем.

- Профили OpenCode явно скрывают неподдерживаемые встроенные режимы размышления,
  чтобы SDK не предлагал Medium для маршрута, разрешающего другие уровни.
- Запросы Claude Desktop к Gemini передают непрозрачный идентификатор пользователя
  через safety labels Gemini вместо ошибки поля metadata до первого ответа.
- Преобразование Responses принимает необязательное имя результата функции,
  если оно совпадает с вызовом. Результат без `call_id` явно сообщает об
  отсутствии точного отображения вместо общей ошибки `input`; вызов не выдумывается.
- Преобразование Chat сохраняет историю отказов на следующем ходе и связывает
  старые результаты функций по имени, а не ID вызова. Имена участников без
  точного отображения и чужая роль ответа Messages больше не игнорируются молча.
- WebSocket-запросы Responses находят модели, доступные только в BPS, при
  отсутствующем или устаревшем доступе, в том числе на следующих ходах.
  Запрещённые нативные аккаунты не вызываются; отозванный доступ не обходится.
- `pause_turn` Messages больше не превращается в вымышленное исчерпание токенов
  на преобразуемом маршруте. Преобразование JSON отклоняет чужую роль выходного
  сообщения Responses и неверный индекс варианта Chat вместо подмены их смысла.
- Форматы ответа при преобразовании отклоняют неуместные ограничения схемы,
  конфликтующие оболочки, неверные имена и тип MIME Gemini вместо молчаливого
  ослабления запроса. Подготовка Messages/Gemini больше не создаёт лишнюю
  полную копию схемы ответа.
- Ключ продолжения учитывает изменённую детализацию изображения и не позволяет
  восстановить прежнее нативное состояние для отредактированной истории.
  Неверный тип и расположение `detail` больше не превращаются молча
  в автоматический режим.
- Преобразование Chat/Messages отклоняет чужие формы `tool_choice` и лишние поля
  вместо молчаливого удаления ограничений. В Messages `tool_choice: none`
  больше не получает неподдерживаемый параметр параллельных вызовов.
- Преобразованные потоки Chat Completions проверяют и соблюдают `include_usage`,
  не отключая внутренний учёт. Учёт исходного потока игнорирует данные после
  терминального события, сохраняя отдельный завершающий usage Gemini.
- Использование разделяет чтение/запись кэша за запрос и оценку хранения,
  сохраняет сообщённые нули и разделяет активность по чату, подключению
  и модели провайдера. Промахи не продлевают оценку; неизвестным и смешанным
  срокам больше не назначается угаданный или максимальный срок жизни.
- Окно приложения можно перемещать за существующую область заголовка.
  ([#88](https://github.com/F0RLE/zenith-relay/pull/88))
- При обрыве Responses после полных SSE-событий клиент получает явную ошибку
  с сохранением последовательности. Новое событие не вставляется в уже
  переданный незавершённый фрейм; разрыв не считается успешным завершением.
- Преобразование сохраняет явные лимиты токенов, типизированные пустые результаты
  custom-инструментов и порядок инструкций. Конфликтующие схемы/Parts Gemini,
  неверные значения инструментов и неподдерживаемая смена исполнителя или
  пространства имён отклоняются вместо тихой потери данных.
- После BPS 401 доступ обновляется перед следующим запросом без повтора неудачной
  генерации. Поздние и временные ошибки обновления сохраняют новые учётные данные,
  отключение/завершение работы участника и его членство в пуле.
- BPS 429 приостанавливает подключение и соблюдает Retry-After; чтение кэша не
  продлевает паузу. Отказ доступа к модели затрагивает только её и псевдонимы
  минимум на 30 минут. Временная ошибка доступа больше не выглядит неизвестной моделью.
- BPS нормализует сокращённую историю и оставляет чужие метаданные клиента локально.
  Ошибка тела 422 и ошибка преобразования завершённого ответа не вызывают ротацию
  или ухудшение состояния исправного аккаунта. Поддержан одиночный прямой вызов
  инструмента; невалидный итоговый вывод не запускает другую генерацию.
- Для Excel по умолчанию действует таймаут 120 секунд без прогресса провайдера;
  heartbeat его не продлевает, корректное завершение не ждёт закрытия соединения.
  При частичных сбоях сохраняются сообщённые токены и реальные времена первого
  вывода/всего ответа, неизвестное время помечается явно. Доступ и кодирование BPS
  используют одинаковые псевдонимы размышлений.
  ([#84](https://github.com/F0RLE/zenith-relay/pull/84))
- Общие отказы запроса и лимиты хода больше не вызывают ротацию всего аккаунта
  или восстановление истории инструментов. Отклонённые зашифрованные контрольные
  точки сжатия сохраняются. Состояние хода/WS ChatGPT привязано к эндпоинту и входу;
  транспортные повторы используют исходный бюджет запроса.
- Поздние операции импорта, прокси и пояса не перезаписывают новые настройки
  и не удаляют учётные данные завершённого импорта. Повторный импорт сохраняет
  выбранный прокси, смена входа блокирует прежнюю ссылку авторизации.
- BPS-запросы обратного отсчёта квоты работают при выключенном локальном API
  и после успешной генерации ждут проверки квоты. Недоступная сохранённая модель
  требует нового выбора вместо незаметного запуска другой модели.
- Мониторинг различает таймауты каталога/квоты, скрывает личность аккаунта
  и сохраняет безопасные ID провайдера в клиентских ошибках. Сброс фильтров,
  узкие индикаторы/подсказки, фокус клавиатуры и временные сбои хранилища браузера
  больше не скрывают элементы и не стирают сохранённые настройки.

## [1.1.5] - 2026-10-05

<!-- relay-notes:en -->

Zenith Relay 1.1.5 changes pool rotation, keeps ChatGPT account continuity, labels API errors, and expands request usage details.

### Changed

- Automatic pool rotation spreads concurrent requests across the least-loaded eligible members, then ranks by fresh quota and provider credits. Session affinity stays until another equally loaded member leads by 15 credits; pool cards show active request counts.
- Pool rotation has Automatic and Manual modes. Manual cycles through the saved member order, skips unavailable members and concurrency limits, and wraps to the start. Automatic keeps request weights and per-member limits.
- In Automatic mode, pool cards with no positive quota window sort by fresh provider-reported credits. API source wallet balances do not affect the order.
- Quick Setup places the Relay mark between connection sources and compatible applications, with window controls in a frameless, draggable top area.
- Excel / Basis Points imports preserve allowlisted provider client headers through reauthentication and token refresh, keep native upstream identity on routed requests, isolate attachment cache entries by credential session, and return provider credential failures without rotating to another account.
- The pool rotation dialog no longer repeats mode explanations. The behavior is in Help.
- Account login notes save in order, keep their fields through reauthentication, and do not show a success message for a failed or closed save. Reauthentication replaces the stored token, keeps local notes, and does not open new-account pool setup.
- Direct ChatGPT launch is disabled for accounts that need sign-in or have a terminal credential, proxy, or health failure. Relay validates credentials before stopping an existing ChatGPT session and restores the session after a failed profile change.
- Newly signed-in OAuth accounts stay outside the pool until explicitly selected. The pool option has no green selection border.
- If a ChatGPT account rejects encrypted reasoning or compaction context, Relay removes the rejected ciphertext and bound id on that same account, keeps the visible summary and remaining history, and retries once. If that account cannot accept the retry, Relay drops the temporary binding and continues with another eligible member.
- Basis Points honors `parallel_tool_calls: false`. If the provider returns multiple client tool calls, Relay asks it to regenerate once and returns an error if the next response still violates the serial setting.
- Chat API errors begin with an English `Account:`, `Provider:`, or `Relay:` prefix in JSON, Messages, Relay Server responses, SSE, WebSocket, route and method failures, and request details.
- Usage details show whether a request used HTTP or WebSocket and provide an informational API-equivalent estimate from published model rates, observed service tier, and input-context band. Missing prices remain unknown. Account plan labels recognize Pro 100, Pro 200, and Pro 500.
- Relay no longer applies fixed byte caps to JSON generation, image upload or edit, provider responses, SSE events, translated streams, or alpha search results. Providers may still reject large requests or responses.
- A stale credit reading no longer overrides a proven rate limit. Fresh provider credits can still keep an account with an empty quota window schedulable.
- Usage price refresh replaces an older row when sample or cache-write counters increase, not only when token counters do.

<!-- relay-notes:ru -->

Zenith Relay 1.1.5 меняет ротацию пула, сохраняет продолжение чатов ChatGPT, помечает источник ошибки API и добавляет сведения об использовании запросов.

### Изменения

- Автоматическая ротация распределяет параллельные запросы по наименее загруженным участникам, затем учитывает свежую квоту и кредиты провайдера. Привязка сессии сохраняется, пока другой участник с такой же загрузкой не опередит на 15 кредитов; карточки пула показывают число активных запросов.
- В ротации пула два режима: «Автоматически» и «Вручную». Ручной режим проходит сохранённый порядок по кругу, пропускает недоступных участников и достигших лимита запросов. Автоматический режим сохраняет долю запросов и индивидуальные лимиты.
- В автоматическом режиме карточки без положительной квоты сортируются по актуальным кредитам провайдера. Баланс API-источника на порядок не влияет.
- В быстрой настройке логотип Relay стоит между подключениями и совместимыми приложениями. Кнопки окна остаются в перетаскиваемой верхней области без полосы.
- Импорт Excel / Basis Points сохраняет разрешённые заголовки клиента провайдера после повторной авторизации и обновления токена, передаёт нативную идентичность провайдера, разделяет кэш вложений по сессии учётных данных и не переключает аккаунт при ошибках учётных данных провайдера.
- В окне ротации нет пояснений режимов. Поведение описано в справке.
- Заметки входа сохраняются по порядку, не пропадают после повторного входа и не показывают успешное сохранение при ошибке или закрытии окна. Повторный вход заменяет сохранённый токен, сохраняет локальные заметки и не открывает настройку пула для нового аккаунта.
- Прямой запуск ChatGPT отключён для аккаунтов, которым нужен повторный вход или у которых есть окончательная ошибка учётных данных, прокси или состояния. Relay проверяет учётные данные до остановки уже запущенного ChatGPT и восстанавливает сессию после неудачной смены профиля.
- Новые OAuth-аккаунты остаются вне пула, пока их явно не выберут. У варианта пула нет зелёной рамки выбора.
- Если аккаунт ChatGPT отклоняет зашифрованный контекст рассуждений или сжатия, Relay удаляет отклонённый шифротекст и связанный id на этом же аккаунте, сохраняет видимое краткое содержание и оставшуюся историю и один раз повторяет запрос. Если этот аккаунт не может принять повтор, временная привязка снимается и запрос продолжается другим подходящим участником.
- Basis Points соблюдает `parallel_tool_calls: false`. Если провайдер возвращает несколько клиентских вызовов инструментов, Relay один раз просит сформировать ответ заново и возвращает ошибку, если следующее сообщение всё ещё нарушает последовательный режим.
- Ошибки Chat API начинаются с английской метки `Account:`, `Provider:` или `Relay:` в JSON, Messages, ответах Relay Server, SSE, WebSocket, ошибках маршрута и метода, а также сведениях о запросе.
- В сведениях об использовании указан транспорт запроса, HTTP или WebSocket, и приведена информационная оценка эквивалентной стоимости API по опубликованным ценам модели, сообщённому режиму обслуживания и размеру контекста на входе. Неизвестные цены остаются неизвестными. Метки планов аккаунта распознают Pro 100, Pro 200 и Pro 500.
- Relay больше не задаёт фиксированные ограничения размера для JSON-генерации, загрузки и редактирования изображений, ответов провайдера, событий SSE, преобразованных потоков и результатов alpha search. Провайдеры по-прежнему могут отклонить большой запрос или ответ.
- Устаревшие кредиты больше не отменяют подтверждённый лимит. Свежие кредиты провайдера по-прежнему оставляют аккаунт без окна квоты доступным для выбора.
- Обновление цен использования заменяет старую строку, когда растут счётчики семплов или записи кэша, а не только токенов.

## [1.1.4] - 2026-10-03

<!-- relay-notes:en -->

Zenith Relay 1.1.4 improves quota-based rotation, checks the model reported by ChatGPT account routes, adds images to Basis Points, and makes launcher controls more responsive.

### Changed

- Automatic rotation picks the member with the largest fresh quota remainder. One point is enough to switch. Load is compared only when the remainder is equal. A chat stays on its member when the saved history cannot be resent to a member with more quota left.
- OpenAI models stay in the order Astra, Sol, Terra, then Luna. Anthropic models stay in the order Fable, Opus, Sonnet, then Haiku. A newer version of the same family keeps that place.
- An API price uses the provider price first. If the provider does not send one, Relay uses the official family price: GPT, ChatGPT, and Codex use OpenAI; Claude uses Anthropic; Gemini uses Google; Grok uses xAI. A manual price is used only when neither exists.
- The API tab can block a ChatGPT account route when it reports an internal degrade id or a different model. Relay checks response.created, subsequent SSE events, buffered responses, and WebSocket turns, including reused connections. A rejection before generation can select another member; a mismatch after generation stops the response without replaying it. This checks reported identity, not the model's intelligence.
- The tool-optimization switch is removed. Tool lists leave Relay exactly as the client sent them. A previously saved optimized mode no longer changes new requests.
- Switching an account keeps the official ChatGPT catalog and its official models. A direct API connection uses the models reported by that provider. Known reasoning levels come from the catalog parser and are connected directly, without the pool rewriting them.
- The pool picker filters working, cooldown, unavailable, disabled, and server connections. The status color matches the account card. The automatic queue is the left-to-right order of the cards, not a text list.
- Model speed is shown as icons. A reasoning control appears only for a text model that has reasoning levels, and it sits after the speed icons.
- On macOS, close, minimize, and full screen are on the left and do what those buttons normally do. The name and logo stay on the right.
- A model switch saves without waiting for a full snapshot. Other switches keep their new value while a background refresh runs, so the rest of the window stays usable.
- ChatGPT account requests use current Chrome headers. Quota rejection keeps the account recoverable and applies to the open primary window instead of marking the account broken.
- Excel / Basis Points accepts an image inside a user message. A data URL is uploaded with the account and replaced by file_id. A remote image URL is rejected, and detail is not forwarded. One encrypted-content failure drops the foreign ciphertext and retries the same account.
- Excel / Basis Points keeps long history identifiers within the upstream limit, so a tool call still matches its output. A visible reasoning summary stays even without ciphertext. Maximum reasoning is sent as the extra-high level this route supports.
- A blank ChatGPT model-catalog response no longer removes models the account already reported. A failed model-list refresh says that the list could not be refreshed, and a later local failure does not hide the provider error already recorded for that account.
- Basis Points model-access and usage-policy refusals keep their original 403 status. A model-access refusal pauses that model on the affected member; it does not ban the whole account. A usage-policy refusal ends the request without marking quota exhausted or trying other accounts to bypass it.
- Identity checks run before protocol conversion, so an adapter cannot hide a different upstream model by writing the requested name into its response. Error codes and safe explanations survive conversion to the client's protocol.
- Buffered account streams finish as soon as the terminal response arrives, even when the provider keeps the connection open. Split SSE events are inspected together, and oversized uninspectable events stop the stream.
- A failed terminal response that already contains generated output cannot trigger history repair or another model execution. Unknown disconnects also stay failures instead of being silently replayed.
- The SSE text-delta path recognizes ordinary deltas without building full JSON trees. Terminal usage, reasoning tokens, cache counters, and provider-reported service tier still pass through the normal accounting path.
- Pool card order reflects eligibility for the selected model before quota ranking. An account that cannot serve that model does not appear ahead of a usable API source just because its general account status is healthy.
- Failed setting changes roll the switch back and release its busy state, including when saving throws before returning a promise. Tabs and other controls remain usable during independent background operations.
- Launcher keyboard focus no longer adds the unwanted outlines or inset shadows. Change notifications keep their compact box without an extra border or shadow.
- Runtime snapshots reuse account facts and shared projections instead of repeatedly copying credentials and rebuilding the same state. Desktop and Relay Server use the same core definitions for account status and source summaries.
- Both Help languages explain route mismatch, model-access refusals, usage-policy blocks, continuation failures, and the corresponding recovery steps. These release notes are included in the in-app updater in English or Russian according to the selected language.

<!-- relay-notes:ru -->

Zenith Relay 1.1.4 улучшает ротацию по квоте, проверяет модель в ответах аккаунтов ChatGPT, добавляет изображения в Basis Points и ускоряет реакцию элементов лаунчера.

### Изменения

- Автоматическая ротация выбирает участника с наибольшим свежим остатком квоты. Одного пункта достаточно для переключения. Нагрузка сравнивается только при равном остатке. Чат остаётся на своём участнике, если сохранённую историю нельзя переслать участнику с большим остатком.
- Модели OpenAI остаются в порядке Astra, Sol, Terra, затем Luna. Модели Anthropic остаются в порядке Fable, Opus, Sonnet, затем Haiku. Новая версия того же семейства занимает это же место.
- Цена API сначала берётся у провайдера. Если провайдер её не прислал, Relay использует официальную цену семейства: GPT, ChatGPT и Codex относятся к OpenAI, Claude к Anthropic, Gemini к Google, Grok к xAI. Ручная цена нужна только когда обеих нет.
- На вкладке API можно блокировать маршрут аккаунта ChatGPT, если он сообщает внутренний degrade-id или другую модель. Проверяются response.created, последующие события SSE, собранные ответы и ходы WebSocket, в том числе на повторно используемом соединении. Отказ до генерации позволяет выбрать другого участника; подмена после генерации останавливает ответ без повторного выполнения. Проверяется заявленная модель, а не её интеллект.
- Переключатель оптимизации инструментов убран. Список инструментов уходит так, как его прислал клиент. Сохранённый оптимизированный режим больше не меняет новые запросы.
- Переключение аккаунта сохраняет официальный каталог ChatGPT и его официальные модели. Прямое API-подключение берёт модели, которые сообщил этот провайдер. Известные уровни размышления берутся из разбора каталога и подключаются напрямую, без переписывания пулом.
- При добавлении в пул можно отфильтровать рабочие, в кулдауне, недоступные, отключённые и серверные подключения. Цвет состояния совпадает с карточкой аккаунта. Автоматическая очередь видна порядком карточек слева направо, а не текстовым списком.
- Скорость модели показана иконками. Выбор размышления появляется только у текстовой модели, у которой есть уровни, и стоит после иконок скорости.
- На macOS закрытие, сворачивание и полный экран стоят слева и выполняют свои обычные действия. Название и логотип остаются справа.
- Переключение модели сохраняется без ожидания полного снимка. Остальные переключатели сохраняют новое значение, пока идёт фоновое обновление, поэтому остальным окном можно пользоваться.
- Запросы аккаунта ChatGPT отправляются с актуальными заголовками Chrome. Отказ по квоте оставляет аккаунт восстановимым и применяется к открытому основному окну, а не помечает аккаунт сломанным.
- Excel / Basis Points принимает изображение в сообщении пользователя. Data URL загружается от имени аккаунта и заменяется на file_id. Внешняя ссылка отклоняется, поле detail не пересылается. Одна ошибка зашифрованного содержимого убирает чужой шифротекст и повторяет запрос тем же аккаунтом.
- Excel / Basis Points удерживает длинные идентификаторы истории в допустимом пределе, поэтому вызов инструмента по-прежнему совпадает со своим ответом. Видимое краткое содержание размышления сохраняется даже без шифротекста. Максимальное размышление отправляется как экстра-высокий уровень, который поддерживает этот маршрут.
- Пустой ответ каталога моделей ChatGPT больше не удаляет модели, которые аккаунт уже сообщил. Ошибка обновления списка моделей говорит, что список не удалось обновить, а поздняя локальная ошибка не скрывает уже записанную ошибку провайдера.
- Отказы Basis Points по доступу к модели и правилам использования сохраняют исходный статус 403. Отказ доступа временно приостанавливает эту модель у конкретного участника, а не блокирует весь аккаунт. Отказ по правилам использования завершает запрос, не обнуляет квоту и не перебирает другие аккаунты для обхода ограничения.
- Модель проверяется до преобразования протокола: адаптер не может скрыть другую модель провайдера, записав в ответ запрошенное имя. Код ошибки и безопасное объяснение сохраняются в протоколе клиента.
- Сборка ответа аккаунта заканчивается сразу после завершающего события, даже если провайдер держит соединение открытым. Разбитые на части события SSE проверяются целиком; слишком большое событие, которое нельзя проверить, останавливает поток.
- Ошибка завершения, в которой уже есть сгенерированный ответ, не запускает восстановление истории с повторной генерацией. Обрыв с неизвестным результатом также остаётся ошибкой, а не скрытым повтором запроса.
- Быстрый путь текстовых SSE-дельт распознаёт обычные события без построения полного JSON-дерева. Итоговое использование, токены размышления, счётчики кеша и сообщённый провайдером класс обслуживания по-прежнему учитываются обычным способом.
- Порядок карточек сначала учитывает доступность выбранной модели, затем квоту. Аккаунт без этой модели не оказывается впереди подходящего API-провайдера только потому, что в целом он исправен.
- Если настройку не удалось сохранить, переключатель возвращается назад и перестаёт висеть в загрузке — в том числе при мгновенной ошибке команды. Вкладки и остальные элементы остаются доступны во время независимых фоновых операций.
- Фокус с клавиатуры больше не добавляет нежелательные обводки и внутренние тени в лаунчере. Уведомления об изменениях сохраняют компактный блок без дополнительной рамки и тени.
- Снимки состояния повторно используют сведения об аккаунтах и общие представления вместо лишнего копирования учётных данных и одинаковых вычислений. Desktop и Relay Server используют общие определения статусов аккаунтов и сводок провайдеров.
- В справке на обоих языках описаны несовпадение модели, отказы доступа, блокировки по правилам использования, ошибки продолжения и способы восстановления. Описание этой версии попадает во встроенное обновление на русском или английском согласно выбранному языку.

## [1.1.3] - 2026-09-30

<!-- relay-notes:en -->

Zenith Relay 1.1.3 keeps a ChatGPT account available after a generic provider
403, restores Excel tool history, shows an approximate cache lifetime beside usage tokens, and
uses the same compact controls across the launcher.

### Changed

- ChatGPT sign-in opens in a Relay window instead of the system browser. The
  account proxy is used for that window and for the token exchange. A live
  ChatGPT session finishes sign-in by itself; otherwise it is finished in the
  window. Saved login notes are not filled in. The lock inside the new-sign-in
  button chooses a saved HTTP proxy for that first login; it does not pick one
  at random.

- Excel / Basis Points tool instructions follow adapter v0.2.8. Examples use
  only tools allowed in the current request and match their declared type and
  schema, including the two JSON layers for function arguments. A separate
  developer reminder repeats the transport rule and keeps custom-tool input
  raw. The one-shot retry hint is appended after the prepared input, before a
  compaction trigger. Structured `text.format` is rejected instead of being
  dropped. Ordinary `service_tier` values `auto`, `default`, and `standard`
  stay on this route; Fast still does not. Image upload from the upstream
  plugin is not copied.

- An image stream that ends incomplete, failed, or without a real completion
  is reported as a failure instead of a finished image.

- A failed provider-balance refresh keeps the last amount. The selected API
  overview marks it **Not refreshed**. A pool card keeps the amount without
  that label; the reason stays in diagnostics.

- The API tool optimization switch now says that schemas open on demand for a
  normal Responses route, while Excel and other routes still send the full list.

- The API switch for ChatGPT accounts in the pool is now **Use Basis Points**.
  It sends those accounts through Excel instead of Responses and may help a
  degraded account generate, without guaranteeing the result.

- Closing the main window now releases its WebView and leaves the pool running
  in the tray. Opening Relay creates the window again instead of reusing the
  hidden page.

- A generic provider 403 no longer marks a ChatGPT account as permanently
  blocked. The account stays available, the dialog uses the upstream access
  error, and a previously stored false block is cleared when desktop storage
  opens or a quota refresh succeeds without a new denial. An explicit disabled
  workspace still stays blocked.

- Anthropic models now follow the provider's family order: Fable, Opus, Sonnet,
  then Haiku. Newest releases remain first within each family, and unknown
  families remain visible after the known lineup.

- Pool and Connections now share a compact header with clear primary actions
  and wrapping tabs. Pool presets live in the overflow menu; participant
  status, account filters and routing controls use one consistent panel,
  with current activity alongside the pool controls.

- Named Responses WebSocket requests now validate the documented `stream_id`
  characters and length, and HTTP/SSE fallback events and Relay-generated
  request errors include their named stream ID. Invalid IDs return
  `invalid_stream_id`. A bare native Responses SSE `[DONE]` without a
  `response.completed` is now an incomplete stream rather than a successful
  WebSocket turn with no terminal event. SSE terminal markers from another
  protocol no longer report a completed request for Responses, Chat
  Completions, Messages or Gemini; a claimed Responses completion with an
  explicitly failed or incomplete status is no longer recorded as success.
  Full parallel WebSocket multiplexing remains open work.

- Accounts with both a remaining provider quota window and credits now retain
  the reported window for pool admission, including the protected Codex
  account reserve. A temporarily unavailable route without an account error
  no longer appears as an invented account failure.

- Usage request and error lists now accept a page number for direct navigation;
  previous and next controls remain available.

- Route recovery now lives under **API → API** and applies to text requests in
  Responses, Chat Completions, Messages and Gemini, including clients other
  than ChatGPT. Existing saved switches are preserved; the wait is live and
  still shares the request's send and queue budgets. Empty Gemini prompt blocks
  converted to Chat Completions now keep a null assistant content field.

- Source pricing and pool member rules show models directly under their provider,
  without extra family headings. The model list keeps the catalog order and the
  dialogs use tighter, consistent rows. Source pricing also shows explicit
  five-minute and one-hour cache-write rates even when the model catalog was
  discovered through another endpoint; prices without a stated TTL remain
  unknown. An unknown cache-read price no longer appears as the input price.

- Quick Setup now starts with a shared local or server pool. The local
  connection step accepts both ChatGPT accounts and API sources, including
  multiple connections before choosing a client. A new API source joins the
  pool when saved; importing the current ChatGPT profile stays on the same
  step so another source can be added. Existing direct **Choose API** mode
  remains available from the application mode menu.

- Light and dark themes now use the warm neutral surfaces and deep green accents
  from the Zenith website, while keeping warning, error and information colors
  distinct.

- Model Rules now use consistent compact controls for reasoning, request speed
  and model visibility. Selected reasoning levels have check marks, and narrow
  windows keep speed labels visible without horizontal scrolling.

- Usage now has separate summary cards, compact filters and clearer request
  details. Narrow windows show labeled report cards, while summary settings
  use the same switches as the rest of Relay.

- Relay dialogs now share one compact visual style and close when the free
  space outside the window is clicked. Action menus, option lists, the mode
  picker, context menus and the mobile Help contents use the same compact
  surface and close on an outside click as well. Escape and the close button
  keep the same behavior. Long forms now use consistent field widths, action
  areas, selected states and scroll boundaries across connection, proxy,
  automation and export windows.

- Pool now separates participant counts, routing controls and current activity
  in an open layout. Connections and Pool share a compact summary with inline
  counts. Account controls have a full-width search field in narrow windows.
  Request details still identify the transport used.

- Excel / Basis Points tool failures now identify the invalid envelope field
  without revealing tool arguments. An ambiguous unqualified namespace tool or
  a non-object function argument is rejected instead of dispatching the wrong
  tool. Responses output is unwrapped before conversion to Chat Completions,
  Messages or Gemini, including their SSE event formats. Malformed completed
  output gets one bounded regeneration before Relay returns a terminal 502; it
  does not cool down the account or retry again after that regeneration.

- Account routes using Excel / Basis Points now use the v0.1.14 tool envelope:
  the client tool name is carried in `references`, while `code` contains the
  function arguments or custom-tool input directly. Historical client calls,
  including calls whose catalog is no longer in the request, are restored to
  that envelope. A declared tool excluded by `tool_choice` is rejected as a
  choice error. Namespaced tools retain their fully qualified name.
  Requests also carry stable turn metadata and the exact Excel client headers,
  including the corrected Office header names, so the upstream endpoint does
  not reject an otherwise valid request with a generic 422.
  The strict Basis Points body now matches the upstream adapter: client tool
  definitions stay in developer instructions, `tools`/`tool_choice` are not
  forwarded, and the original stream flag is preserved.
  A malformed client-tool relay now gets the same single bounded regeneration
  used by the official adapter before Relay records a terminal 502.

- Disconnecting a managed ChatGPT profile now restores only Relay-owned
  settings and login fields. External `config.toml` edits and newer manual
  OAuth sign-ins are kept instead of blocking the disconnect; reconnecting
  still cannot silently replace that new sign-in.

- A changed ChatGPT model catalog file no longer makes its profile backup look
  corrupt. Disconnect restores the previous settings and sign-in without
  deleting the externally edited catalog; newer sign-ins remain protected.

- Converted tool turns keep parallel Responses function calls together for
  Chat Completions providers. Messages and Chat Completions refusals reach
  clients with the proper terminal reason, including empty refusals in Codex;
  an incomplete Messages answer no longer appears as a successful completion.
  Request details distinguish the model requested by the client from the ID
  sent to the selected source when they differ.

- macOS builds now use ad-hoc signing without an Apple certificate. The DMG
  and updater archive are checked before publication, and release files include
  checksums. First launch still needs one-time approval in macOS settings.

- Codex catalogs now retain Ultra for exact models supported by the installed
  Codex or a matching native account card when the pool can route Max and the
  subagent effort. Generic API/adapter Max no longer creates a misleading
  Ultra option. This Codex orchestration mode is separate from request speed.

- API-source model and balance refreshes now share bounded work between manual
  and background requests. Changing an API address, key or catalog discards
  late results. Reopening a page uses the current session's cached statistics;
  an explicit refresh fetches again. If that fetch fails, the last successful
  amount is shown as stale with the failure reason, without disabling routing.
  Background observations appear in Pool and Choose API without extra provider
  reads. Protocol aliases now activate the cadence of their physical API source
  instead of being mistaken for accounts.

- Pool and API cards keep model, quota and balance refresh evidence out of the
  normal presentation. Values, actionable failures and stale-reading warnings
  remain visible where they help the user; detailed refresh state and check
  times stay in diagnostics and do not affect routing.

- Desktop and Relay Server share account quota/model refresh work between
  background jobs and manual requests. Quota and model lists update independently,
  repeated refreshes respect provider pauses, and closing a waiting request does
  not cancel shared work. Initial quota/model and reset-credit authorization
  preparation joins a
  separate on-demand job with reserved capacity; credentials never enter the
  observation cache. Late results and errors cannot overwrite a newer
  sign-in, proxy setting or replaced account; newer quota data from model
  requests takes precedence over an older read. Fresh quota headers from an
  inference request can postpone the next automatic quota poll when subscription
  metadata is current; manual refresh and reset verification remain scheduled.
  Relay Server also checks quota again shortly after a reported future reset.
  A delayed server 401 from an old bearer cannot invalidate a newer token or
  replaced login during quota or model recovery. Late OAuth and Agent-task
  writes cannot replace credentials installed by a subsequent import; an old
  runtime build cannot publish after that import or account deletion. Server
  re-import and deletion now close the old account route before changing its
  saved credential; a failed vault deletion restores the record without
  reopening pending work on the old runtime. Provider
  management requests also have a shared bounded HTTP queue with reserved
  login/recovery capacity; a changed account or source cannot send an outdated
  queued read. An older desktop refresh or pending token write cannot recreate
  credentials after an account is deleted and added again.

- The 1.1.3 update converts existing pool profiles to the current rotation automatically
  at startup, preserving saved member settings and gateway enabled state. No
  migration confirmation, notification, manual stop/start or rollback UI is
  required. Obsolete failure thresholds and ranking options are discarded;
  older saved profiles and presets still open. Old servers cannot receive
  unsupported rotation settings.

- Moving accounts to a user-managed server now closes pending local dispatch
  through import and verified cleanup. An interrupted move or a server-owned
  account cannot reappear in local routing after a restart; failed local
  activation keeps ownership in recovery rather than enabling two copies.

- If deleting a local account fails and its credentials or profile cannot be
  restored, the local gateway now stops instead of serving a stale route.

- Capacity and recovery waits now share runtime/per-key count and retained-byte
  limits, event-driven wakeups and one accumulated wait budget across retries
  and transport handoffs. Busy capacity is assigned fairly between keys;
  cancellation releases queue accounting without penalizing a provider. When
  every physical slot is busy, new requests no longer rescan the entire wait
  queue; unsupported routes still fail without joining it.

- Pool selection now uses normalized local load in automatic mode, with
  physical members sharing capacity across protocol aliases. Retries share one
  request budget through transports and repairs; an uncertain provider outcome
  is not silently replayed. Mandatory provider pauses are installed before a
  slot becomes available to another request. Server API sources no longer have
  a second, independently timed storm block outside pool rotation. Pending sends
  cannot use an old desktop account/source route while its membership, endpoint,
  proxy, login or permissions are being replaced; already started work may finish.

- Usage details put an approximate cache lifetime in parentheses beside the
  cache-read count, or beside the cache-write count when there is no read.
  A window reported by the provider is used as given. GPT-5.6 and later fall
  back to OpenAI's documented 30-minute minimum when the response omits one.
  Other missing windows stay marked unreported. The note is estimated from that
  request's latest cache write or read; it is not a live provider expiry.

- Catalog ordering now keeps model families from the same numbered generation
  together and uses stable family IDs for their order. A later release date
  alone no longer moves GPT-6 Sol above GPT-6 Astra.

- Background refresh can no longer replace saved settings with an older
  snapshot. Switching away from a connection and back also clears pending
  refresh work, so a slow earlier request cannot stall the current view.
  Visible remote pool pages now pick up server changes periodically and when
  returning to the window without requiring a local state-change event.

- API settings now offer a small standard/automatic tool optimization switch
  for local and compatible server pools. Standard mode forwards the complete
  catalog; automatic mode applies native Responses deferred tool search to
  every eligible request, with a compatibility retry when the selected
  endpoint does not support the standard fields. Relay no longer exposes
  thresholds or name-based allow/deny lists. The switch saves immediately; the
  full behavior is in Help.

- Pool catalog refresh now resolves each member's routes once for all its models,
  avoiding repeated discovery-policy work as model inventories grow. Offline
  models retain their editable metadata and cache-price fields. (`3e6e174`)
- Codex tool schemas preserve reference scope, validation constraints and literal
  data while limiting reference expansion, preventing excessive memory growth
  from deeply nested or branching definitions. (`3e6e174`)
- Converted Chat Completions responses retain reasoning text alongside tool
  calls in JSON, streams and follow-up history. Opaque reasoning state continues
  to require its native owner. (`e211560`)
- Retrying source setup after a failed snapshot or pool-membership update reuses
  the saved source instead of creating a duplicate. Removed unused controls and
  frontend helpers for the retired API-role ordering and manual route editor.
  (`54b0c75`)

- Long generations no longer end because Relay's HTTP, SSE or WebSocket timers
  expire. Relay waits for the provider, keeps streaming connections alive and
  preserves client cancellation and retries for real failures before output.
  (`3e6e174`)

- Reduced memory retained by reference catalogs between updates. Source data
  stays compact, cached derived data is skipped when loading, and saving a
  catalog no longer reads another full copy of the previous file into memory.
- Reduced startup and catalog refresh work by indexing model matches once and
  sharing immutable reference data. Refresh no longer merges the same catalog
  twice or duplicates its JSON tree in memory.
- Fixed a Windows crash when refreshing pool members. Bulk quota refresh keeps
  bounded concurrency and returns each account's result without overflowing
  the desktop command stack.
- Codex launch now applies pending model catalog updates before opening the
  client. Connecting a local pool checks its catalog before closing Codex, and
  starting the client no longer waits for an intermediate interface refresh.
  Profile preparation failures reliably reopen a previously running client.
  Account catalog requests run concurrently within a shared time limit, and
  history synchronization avoids rereading every file just to fingerprint it.
  A successful connection reuses its catalog when launching, and an OpenCode
  configuration error no longer prevents Codex catalog updates.
- Reconnecting Codex no longer fails because already-matching chat history
  exceeds the history-repair size limit. Only files requiring a provider change
  count toward the rewrite budget, so model and speed updates can complete
  without backing up unchanged conversations.

- Model details now come from a shared reference catalog for accounts and API
  providers, with consistent defaults when metadata is missing. Participant
  capability fields no longer hide reasoning or tools, and redundant metadata
  polling has been removed. Provider prices still take precedence when supplied.
- OpenAI models offer Standard, Fast and Ultrafast by Relay policy, including
  models whose account or provider returns no speed fields. Explicit speed
  choices survive rotation and override the pool default.
  Speed catalog entries include descriptions so Codex's Ultrafast option no
  longer has an empty explanation.

- API sources, stored proxies and quota automations share a more readable layout
  and compact editors. Proxy import can check new addresses immediately and show
  the actual exit IP, country and response time. Failed checks keep saved proxies
  and assignments; declared location is shown separately from observed results.
  Quota rules run automatically without a separate start button or mode selector.
  Existing manual rules are upgraded while disabled rules remain disabled.
  Default rule names now distinguish starting a quota countdown from resetting
  the weekly limit, including existing rules. Custom names remain unchanged.
  The editor starts with the automation type and its relevant fields; a custom
  rule name is optional. General automation labels no longer refer only to quotas.
  The Automations tab omits the shared search and refresh toolbar; rule changes
  update the list automatically.
  Frequent row actions use icons with tooltips. API source rows place launch
  before edit, with additional actions last. (`0b65f07`, `54b0c75`)

- Source creation keeps provider choices visible above a single column of key,
  address and name fields. Quick setup now groups progress, choices and navigation
  in a compact workspace without repeating the window's logo. Custom API setup
  exposes all required fields and can continue after they are filled; reselecting
  the active provider preserves edits. (`54b0c75`)

- The API tab brings status, pool counts, address and key into one connection
  panel with labelled copy actions. Port settings have their own row, and key
  reissue is available from the key's action menu.

- Adding connections to a pool now uses one searchable list, account/API sections,
  a selected-connections view and fixed add actions. Selection stays intact
  while filtering.

- Native requests no longer fail adapter compatibility checks because a source
  catalog omits a reasoning level or incorrectly marks a feature unsupported.
  Converted routes retain their feature and reasoning checks.

- Settings no longer repeat the enabled debug-mode notice and operations-folder
  button below the toggle; the folder action remains in Diagnostics.

- Streaming keep-alives wait for complete event boundaries, preventing a pause
  in a fragmented response from inserting a heartbeat into its JSON content.

- Invalid stream events now retain safe parser diagnostics in request details:
  JSON error category, position and frame sizes, without recording response
  content. These diagnostics identify Relay's parser separately from an
  upstream error message.

- Streams with mixed LF/CRLF/CR line endings no longer merge separate events and
  fail with `stream_invalid`. The shared parser preserves event order for
  native streams, protocol adapters and context compaction.

- Converted Codex requests accept client tracing, cache-affinity keys and the
  optional encrypted-reasoning output selector. Plain text and empty reasoning
  controls no longer exclude otherwise compatible routes. Unsupported options
  identify the offending field; encrypted input still requires a native route.

- Codex context compaction accepts completed output items delivered before the
  final stream event. Account routing retains only ChatGPT's infrastructure
  cookie in memory, isolated by account and authorization. WebSocket continuations
  reconnect after credential changes only with portable history, and delayed
  catalog refreshes cannot overwrite a different Codex profile binding.

- Relay accepts gzip and zstd JSON requests from current clients. Account
  compaction falls back to the Responses compaction trigger when the old
  compact endpoint is explicitly unavailable, preserving context and usage.
  Turn-state hints stay with the exact session, model and account credentials,
  including authentication refresh and concurrent responses.

- Model visibility, member model permissions, draining, launch preferences and automation enablement
  now use the same switch styling as application settings.

- Pool model rules can reset model and group order to the catalog default.
  Default order places OpenAI, Anthropic, Google and xAI first, then other
  companies alphabetically, and keeps each catalog family together,
  with newer versions first inside the family, consistently across providers.
  New families are grouped automatically from metadata. Failed reorder operations restore the displayed saved order,
  and another reorder cannot overlap a pending save.

- Catalog refresh retains prices and reference reasoning levels even when legacy
  route lists or endpoint declarations are incomplete. Pool client projections
  follow automatic routing, and converted requests retain their source prices
  across all four protocols. Server background refresh also updates protocol
  metadata without overwriting concurrent edits or restoring removed sources.
  Codex receives shared reference reasoning levels through eligible Responses
  routes, including converted models, with key scopes and adapter limits applied.

- Pool model rules now retain names, groups, prices, reasoning modes and saved
  order for all pooled account and API models, even while a member is disabled
  or unavailable. Shared model IDs appear once and count only pool members.

- Codex model labels use shared reference names, then compact labels from the
  model ID, consistently across pools and direct API
  connections. Shared reference names remain intact when participant metadata
  is malformed, and a valid owning-account card takes precedence over an
  incompatible one. Model IDs and pool order are preserved.

- Removed the technical force-refresh sign-in action from account cards and
  menus. Sign-in recovery now stays in the normal sign-in flow, while quota
  refresh remains the only visible refresh action.

- Pool recovery is automatic: all compatible members are considered, temporary
  failures pause for at least five seconds, and repeated failures increase the
  cooldown. Unavailable models, rejected credentials, and opaque gateway
  rejections no longer prevent fallback to another eligible API or account.
  Removed the manual retry-count and last-candidate cooldown controls.

- Rewrote English and Russian Help around the current connection workflow,
  operating modes, pool rotation, quotas, balances, and recovery. Added a linked
  contents list and corrected obsolete settings paths and behavior descriptions.
  Help now has a reading column, a sticky section index with the current section,
  numbered setup steps, and error rows that fit narrow windows. The index becomes
  a compact menu on small screens.
  The error reference now groups exact codes with causes and concrete recovery
  steps, with expandable categories and search by code or symptom.

- Weekly quota reset uses a compact action row with a separate credit count
  in account and pool cards.

- Pool member rules open with a compact model list and one switch per model.
  Expandable provider groups retain disabled models; long lists support search.
  API prices share aligned columns beside each model, including 5-minute and
  1-hour cache writes; narrow windows use labeled price rows. Secondary settings use aligned rows
  on their own tab. Account and API model order now follows metadata and saved manual order
  without moving disabled models or prioritizing price overrides.

- Accounts and API providers now share three pool rotation modes: Automatic,
  In order, and Round robin. Existing Smart profiles become Automatic on update.
  One reorderable list replaces separate API roles, with member weights and shared
  request limits. Settings apply without interrupting active requests; unavailable
  members do not block the rest. Concurrent edits are detected before saving, and
  presets retain the mixed order. The editor uses one scrollbar, short status
  labels and mode-specific controls; centered fields use Request share and
  Concurrent requests, with Unlimited shown for an unset concurrency cap.
  Detailed explanations are in Help. Automatic chooses the least occupied
  eligible members and uses request share only to break ties. It does not rank
  by manual order, balance, or quota percentage. In order preserves the manual
  queue. Modes support keyboard selection.
  Recent failures lose their scheduling penalty within a minute, so recovered
  members can return automatically. Finishing an older request cannot release
  another recovery probe; occupied probes allow a bounded wait. Server pool
  membership changes immediately apply saved order and request limits.
  The next-candidate hint follows the scheduler and is omitted when the choice
  depends on the model or format. Stale activity events cannot overwrite a
  newer runtime's state.

- Pool controls now use a compact toolbar, a separate current/next route line,
  and a framed panel with a shaded status strip and dividers between counters.
  Connections uses the same panel styling for search, filters, and account actions.
  Both summaries keep the total provider credits when available. Full route
  and model names wrap on narrow screens; icon actions retain their tooltips.
- Pool request speed is now a single draggable, three-position control for
  Standard, Fast, and Ultrafast instead of a menu and a separate switch.
  Its compact label shows only the selected mode, with smooth transitions
  that respect reduced-motion preferences. Keyboard and touch selection are
  supported; dragging saves on release.

### Added

- Pools serve Responses, Chat Completions, Messages and Gemini concurrently,
  with native paths and supported conversions for JSON and streaming requests.
- API source routing is now fully automatic. Relay keeps adapter routing
  internal, prefers each model's declared native endpoint and converts requests
  from Responses, Chat Completions, Messages or Gemini when needed.
  Chat Completions supports function tools and their result history.
- New API sources determine formats automatically from provider declarations
  and endpoint settings. Every catalog model remains routable through the
  source fallback when declarations are absent. Legacy manual-mode fields are
  accepted during import and ignored; old physical endpoint assignments remain
  fallback hints so mixed-protocol sources keep working after an upgrade.
- OpenCode uses protocol-specific SDK groups and retains working model IDs and
  user options during catalog refresh. Codex uses HTTP streaming when a model
  needs conversion, while native WebSocket connections remain supported.

- API cards recognize Sub2API, New API, compatible One API billing, DeepSeek
  and SiliconFlow balances. OpenRouter statistics work with ordinary inference keys. Cards
  distinguish wallets, key allowances, subscriptions, currencies and Relay's
  own usage estimate, omit internal adapter labels and missing request counts, and mark failed refreshes
  without discarding the last known balance. Refresh progress uses the button
  animation without adding a duplicate status line below the counters.

- Failed request details now show the provider's original error code, type,
  message, and HTTP status separately from Relay's category. Messages can be
  copied; sensitive content is hidden and long messages are bounded. Older
  records explicitly show when no provider message was saved.
- **Diagnostics** in Settings. Relay keeps separate, size-limited and
  redacted logs for errors, crashes, and important operation stages; each
  folder can be opened directly from the app. Detailed operation logging is
  off by default and can be enabled there when troubleshooting.
- Portable account imports preserve a safe account name and bounded,
  de-duplicated tags without exposing credentials; existing Relay tags remain
  authoritative on reimport.

### Fixed

- Gemini prompt blocks and empty filtered or token-limited candidates now
  reach clients as incomplete responses in JSON and streaming conversions,
  rather than appearing as malformed upstream output or successful completion.
- Account-card actions now use separate, consistent controls without the
  nested header frame; sign-in-required quota states have a clearer
  keyboard and pointer target.
- Pool rotation modes now save on first use and after members join or leave,
  without reverting to Smart because of an outdated stored member list.
  Concurrent edits still refresh and retry safely. ([#74](https://github.com/F0RLE/zenith-relay/pull/74))
- ChatGPT account selection and quota reserve share a compact panel; feature
  switches use flat rows with concise descriptions. Pool member settings no
  longer add a second frame inside the dialog. The OpenCode tab subtitle now
  says "Use the pool in OpenCode". ([#74](https://github.com/F0RLE/zenith-relay/pull/74))
- GPT model names in ChatGPT and Codex keep their original IDs even when the
  selected account has no matching catalog card. Explicit key prefixes and
  existing aliases remain supported; capabilities come from the matching model.
  ([#74](https://github.com/F0RLE/zenith-relay/pull/74))
- Responses tool-history recovery preserves call/result links when a client
  references an item ID or omits a call ID. Results are matched by kind and
  namespace without deleting history or guessing between parallel calls.
  HTTP, streaming and WebSocket use the same bounded recovery. ([#74](https://github.com/F0RLE/zenith-relay/pull/74))
- Pool rotation now saves mode, order and member settings immediately. Dragging
  works consistently, concurrent membership updates refresh the editor, and
  conflicting saves retry without restoring removed members. ([#74](https://github.com/F0RLE/zenith-relay/pull/74))
- Model rule actions share one aligned group with consistently sized format,
  reasoning, speed and enable controls. ([#74](https://github.com/F0RLE/zenith-relay/pull/74))
- Provider price fields use one rounded outline, a separated currency marker
  and numbers aligned to the right. Focus and invalid values highlight the
  whole field consistently in both themes. ([#74](https://github.com/F0RLE/zenith-relay/pull/74))
- Pool speed labels and connection summaries fit wider system fonts without
  clipping or unnecessary line breaks on desktop screens. ([#74](https://github.com/F0RLE/zenith-relay/pull/74))
- Multiple routes for one member share rotation weight and request capacity.
  An unsupported endpoint does not disable the member's other formats; quota
  and authentication failures remain shared. Continuation retries preserve
  complete history, tool results and opaque ownership.
- Converted requests reject unsupported parameters instead of dropping them.
  Schemas keep their constraints, per-turn instructions do not persist into
  later turns, and reasoning levels are offered only where they can be mapped.
  Streamed tools retain their IDs and order, and usage reflects the actual
  upstream format without inventing missing counters.

- Smart cache affinity no longer selects a member outside the best available
  group or bypasses its rotation accounting.
- Provider retry delays are respected for overloaded and unavailable models
  and other service errors, including when a shorter recovery delay is configured.

- Updated TLS handling to rustls 0.23.45 to address RUSTSEC-2026-0285.

- Pool no longer reports that every member is unavailable when routing telemetry
  is missing or stale but ready members remain. When none are ready, the warning
  separates quota waits, unavailable members, and disabled members, with readable
  error reasons. Pool and Connections group accounts by availability while
  preserving routing order within each group.
- Provider failures inside HTTP 200 JSON or streaming responses remain failures
  in usage history. Rate limiting and spend-limit errors stay distinct, and
  explicit request-validation errors do not put an account into cooldown.
- A provider's generic request rejection or disabled model no longer blocks
  fallback to another compatible member. Service errors on account routes keep
  their account attribution without marking the account unhealthy.
- Tool continuations validate the whole history, including calls beyond the
  first sixteen. Completed historic calls no longer select the owner of a new
  tool result, and outputs from different owners cannot be mixed implicitly.
  Examples in tool schemas and nested result data no longer affect routing.
- Account model refreshes no longer erase an independent block, sign-in, or
  verification failure, or downgrade a terminal catalog error after a temporary
  network failure. Quota-monitoring errors no longer hide the reason an account
  is unavailable in Connections and Pool.
- Pool activity matches the last-used account or API source by identity, so
  requests with identical timestamps cannot highlight the wrong member.
- Recovery preserves the original response binding for other chat branches
  and retries, and uses the current request options without reviving old
  instructions or transport settings. Provider-side
  conversation and item references, or tool results without their calls, no
  longer count as a complete local replay.
- Chats with a supplied compacted context can continue without a stale response
  reference over HTTP or WebSocket. The compacted window and retained tool
  results stay intact; unreadable compaction is no longer silently discarded.
- Chat recovery now requires a saved conversation chain before removing an
  earlier response reference unless a complete compacted window is supplied,
  preventing silent loss of prior context.
- Manual credential refresh and observed client logins restore account routing
  health. Remote account cards now reflect runtime cooldowns and unavailable routes.
- Recovered pool accounts remain available without removing another member.
  Delayed authentication updates no longer undo a newer account disable, and
  account cards reflect temporary runtime unavailability during cooldowns.
- Importing an account or API source without adding it to the pool no longer
  restarts the live local gateway. This prevents an unrelated listener restart
  from interrupting Relay during inventory-only imports.
- After an interrupted launch or import, Diagnostics now records the last
  redacted operation stage on the next start, even when detailed debug logging
  was disabled.
- A chat whose original account has run out of quota can now continue through
  the next compatible healthy account or API source. This works for both
  ordinary and WebSocket Responses requests.
- Chats can recover from an expired response reference using Relay's locally
  retained history, including on an already-open WebSocket. Complete tool
  history can rotate with the pool; incomplete tool state is no longer silently
  removed during recovery.
- Unknown response references are rejected before reaching an unrelated
  account. Clients can resend complete history without the old reference;
  otherwise Relay reports that the continuation context is unavailable.
- An account that needs sign-in, is cooling down, or has an error now disables
  only that candidate, including when its status changes while the local gateway
  is already running. The rest of the pool and its available models keep
  rotating normally.
- Account import and OAuth cleanup no longer leave stale temporary state that
  can close Relay or block the next import. A JSON account can be added to
  Relay without adding it to the pool.
- Refreshed account credits and quota limits are applied to the running pool,
  including when Relay is open only in the tray. Only real provider credits
  are shown as available balance.
- Background ChatGPT wake checks now use the live gateway runtime, including
  its token refresh, cooldown, usage, and diagnostics paths, while staying
  pinned to the account that the scheduler selected.
- A model can be returned to Standard speed while its route is temporarily
  unavailable. Fast and Ultrafast are recalculated for every retry candidate,
  so an unsupported speed is not carried to the next account or API source.
- Model Rules keeps every model from pooled members, including members that are
  disabled or temporarily unavailable. Request availability is checked when
  Relay selects a member, so a missing route does not delete the model row.
- Saving a partial Model Rules reorder now preserves unavailable and
  binding-only pool models instead of treating them as removed.
- Pool and Connections cards use consistent account information and
  reauthentication controls. Unavailable accounts remain visible with their
  status instead of making the whole pool look unavailable.
- The OAuth completion page no longer shows a close button that cannot work.

<!-- relay-notes:ru -->

Zenith Relay 1.1.3 оставляет аккаунт ChatGPT доступным после общего отказа провайдера 403, восстанавливает историю инструментов Excel, показывает примерный срок кэша рядом с токенами использования и применяет те же компактные переключатели по всему приложению.

### Изменения

- Вход в ChatGPT открывается в окне Relay, а не в системном браузере. Прокси
  аккаунта используется и для этого окна, и для обмена токена. Действующая
  сессия завершает вход сама; иначе вход заканчивается в окне. Сохранённые
  заметки входа сами не подставляются. Замок внутри кнопки нового входа
  выбирает сохранённый HTTP-прокси для первого входа и не подставляет
  случайный.

- Инструкции инструментов Excel / Basis Points следуют адаптеру v0.2.8.
  Примеры используют только инструменты текущего запроса и совпадают с их
  типом и схемой, включая два слоя JSON для аргументов функции. Отдельное
  напоминание разработчика повторяет правило транспорта и оставляет ввод
  custom-инструмента как есть. Подсказка одноразового повтора добавляется
  после подготовленного ввода, до триггера сжатия. Структурный `text.format`
  отклоняется, а не отбрасывается. Обычные значения `service_tier` `auto`,
  `default` и `standard` остаются на этом маршруте; Fast по-прежнему нет.
  Загрузка изображений из внешнего плагина не копируется.

- Поток изображения, который закончился неполным, с ошибкой или без реального
  завершения, записывается как ошибка, а не как готовое изображение.

- Неудачное обновление баланса провайдера сохраняет последнюю сумму. Обзор
  выбранного API помечает её **Не обновлено**. Карточка пула оставляет сумму
  без этой пометки; причина остаётся в диагностике.

- Переключатель оптимизации инструментов теперь поясняет, что схемы открываются по запросу на обычном маршруте Responses, а Excel и остальные маршруты по-прежнему отправляют полный список.

- Переключатель для аккаунтов ChatGPT в пуле теперь называется **Использовать Basis Points**. Такие аккаунты отправляются через Excel, а не через Responses. Это может помочь деградировавшему аккаунту с генерацией, но результат не гарантирован.

- Закрытие главного окна освобождает его страницу, а пул продолжает работать в трее. Следующее открытие создаёт окно заново, а не возвращает скрытую страницу.

- Общий отказ провайдера 403 больше не блокирует аккаунт ChatGPT навсегда. Аккаунт остаётся доступным, диалог показывает ошибку доступа от провайдера, а ранее сохранённая ложная блокировка снимается при открытии локального хранилища или при успешном обновлении квоты без нового отказа. Явно отключённое рабочее пространство по-прежнему остаётся заблокированным.

- Модели Anthropic идут в порядке семейств провайдера: Fable, Opus, Sonnet, затем Haiku. Внутри семейства новые выпуски остаются первыми, неизвестные семейства видны после известного ряда.

- Пул и Подключения используют общий компактный заголовок с понятными основными действиями и переносимыми вкладками. Пресеты пула находятся в дополнительном меню. Статус участников, фильтры аккаунтов и управление маршрутизацией собраны в одной панели, а текущая активность показана рядом с управлением пулом.

- Именованные запросы Responses по WebSocket проверяют допустимые символы и длину `stream_id`. События запасного HTTP/SSE и ошибки запроса, созданные Relay, содержат этот идентификатор. Неверный идентификатор возвращает `invalid_stream_id`. Одиночный нативный `[DONE]` у Responses без `response.completed` теперь считается незавершённым потоком, а не успешным ходом WebSocket без завершающего события. Завершающие маркеры SSE другого протокола больше не отмечают запрос Responses, Chat Completions, Messages или Gemini как завершённый. Заявленное завершение Responses с явно неуспешным или неполным статусом больше не записывается как успех. Полное параллельное мультиплексирование WebSocket остаётся незавершённой работой.

- Если у аккаунта одновременно есть оставшееся окно квоты провайдера и кредиты, для допуска в пул сохраняется сообщённое окно, включая защищённый резерв аккаунта Codex. Временно недоступный маршрут без ошибки аккаунта больше не показывается как выдуманный сбой аккаунта.

- Списки запросов и ошибок в использовании принимают номер страницы для прямого перехода. Кнопки назад и вперёд остаются.

- Ожидание восстановления маршрута находится в **API → API** и применяется к текстовым запросам Responses, Chat Completions, Messages и Gemini, в том числе не из ChatGPT. Сохранённые переключатели не сбрасываются. Ожидание остаётся живым и по-прежнему использует бюджеты отправки и очереди этого запроса. Пустые блоки запроса Gemini, преобразованные в Chat Completions, сохраняют пустое поле содержимого ответа ассистента.

- Цены источника и правила участника пула показывают модели прямо под провайдером, без дополнительных заголовков семейств. Порядок списка остаётся каталожным, строки в окнах стали плотнее и единообразнее. Цены источника показывают явную запись кэша на 5 минут и на 1 час, даже если каталог модели получен через другой адрес. Цена без указанного срока остаётся неизвестной. Неизвестная цена чтения кэша больше не подменяется ценой входа.

- Быстрая настройка начинается с общего локального или серверного пула. На компьютере в одном шаге можно добавить и аккаунты ChatGPT, и API-источники, в том числе несколько подключений до выбора клиента. Новый API-источник сразу входит в пул. Импорт текущего профиля ChatGPT остаётся на том же шаге, чтобы можно было добавить ещё один источник. Прямой режим **Выбрать API** по-прежнему доступен в меню режима приложения.

- Светлая и тёмная темы используют тёплые нейтральные поверхности и глубокий зелёный акцент с сайта Zenith. Цвета предупреждения, ошибки и информации остаются различимыми.

- В правилах моделей компактные переключатели размышления, скорости запроса и видимости модели выглядят одинаково. Выбранные уровни размышления отмечены галочкой. В узком окне подписи скорости остаются видны без горизонтальной прокрутки.

- В использовании отдельные карточки сводки, компактные фильтры и более понятные сведения о запросе. В узком окне карточки отчёта подписаны, а переключатели сводки такие же, как в остальном Relay.

- Окна Relay используют один компактный вид и закрываются по щелчку на свободном месте вокруг окна. Меню действий, списки вариантов, выбор режима, контекстные меню и оглавление справки на узком экране используют ту же поверхность и тоже закрываются по щелчку снаружи. Escape и кнопка закрытия работают как раньше. Длинные формы подключений, прокси, автоматизаций и экспорта держат одинаковую ширину полей, область действий, состояние выбора и границы прокрутки.

- Пул отдельно показывает число участников, управление маршрутизацией и текущую активность. Подключения и Пул используют компактную сводку со счётчиками в строке. В узком окне поиск по аккаунтам занимает всю ширину. В сведениях о запросе по-прежнему указан использованный транспорт.

- Ошибки инструментов Excel / Basis Points называют неверное поле конверта и не раскрывают аргументы инструмента. Неоднозначный инструмент без полного имени пространства или аргумент функции, который не является объектом, отклоняется, а не вызывает не тот инструмент. Вывод Responses разворачивается перед преобразованием в Chat Completions, Messages или Gemini, включая их форматы событий SSE. Повреждённый завершённый вывод один раз ограниченно пересобирается, после чего Relay возвращает окончательную ошибку 502. Аккаунт при этом не уходит на паузу, и повторной попытки после этой пересборки нет.

- Маршруты аккаунтов через Excel / Basis Points используют конверт инструментов v0.1.14: имя инструмента клиента передаётся в `references`, а `code` содержит аргументы функции или ввод пользовательского инструмента напрямую. Прежние вызовы клиента, даже если их описания уже нет в запросе, восстанавливаются в этот конверт. Инструмент, исключённый `tool_choice`, отклоняется как ошибка выбора. Инструменты с пространством имён сохраняют полное имя. Запросы также несут устойчивые сведения о ходе и точные заголовки клиента Excel, включая исправленные имена заголовков Office, чтобы внешний адрес не отвечал общим отказом 422 на иначе правильный запрос. Строгое тело Basis Points совпадает с внешним адаптером: описания инструментов клиента остаются в инструкциях разработчика, `tools` и `tool_choice` не пересылаются, исходный признак потока сохраняется. Повреждённая передача инструмента клиента получает ту же одну ограниченную пересборку, что и официальный адаптер, прежде чем Relay запишет окончательную ошибку 502.

- Отключение управляемого профиля ChatGPT восстанавливает только настройки и поля входа, которыми владеет Relay. Внешние правки `config.toml` и более новый ручной вход OAuth сохраняются и не блокируют отключение. Повторное подключение по-прежнему не может тихо заменить этот новый вход.

- Изменённый файл каталога моделей ChatGPT больше не делает резервную копию профиля повреждённой. Отключение возвращает прежние настройки и вход, не удаляя каталог, изменённый снаружи. Более новый вход остаётся защищён.

- Преобразованные ходы с инструментами сохраняют параллельные вызовы функций Responses вместе для провайдеров Chat Completions. Отказы Messages и Chat Completions доходят до клиента с правильной причиной завершения, включая пустой отказ в Codex. Неполный ответ Messages больше не выглядит успешным завершением. Сведения о запросе отличают модель, которую запросил клиент, от идентификатора, отправленного выбранному источнику, если они различаются.

- Сборки macOS подписываются ad-hoc, без сертификата Apple. DMG и архив обновления проверяются до публикации, у файлов релиза есть контрольные суммы. Первый запуск по-прежнему требует одно разрешение в настройках macOS.

- Каталоги Codex сохраняют Ultra для точных моделей, которые поддерживает установленный Codex или подходящая нативная карточка аккаунта, если пул может провести Max и усилие субагента. Обычный Max у API или адаптера больше не создаёт вводящий в заблуждение вариант Ultra. Этот режим оркестрации Codex отделён от скорости запроса.

- Обновление моделей и баланса API-источника делит ограниченную работу между ручным и фоновым запросом. Смена адреса, ключа или каталога отбрасывает запоздалый результат. Повторное открытие страницы использует статистику текущего сеанса. Явное обновление запрашивает данные снова. Если запрос не удался, последняя успешная сумма показывается как устаревшая вместе с причиной, без отключения маршрутизации. Фоновые наблюдения появляются в Пуле и в режиме «Выбрать API» без лишнего обращения к провайдеру. Псевдонимы протокола включают периодичность своего физического API-источника и больше не принимаются за аккаунты.

- Карточки пула и API не показывают в обычном виде подробности обновления моделей, квоты и баланса. Значения, ошибки, с которыми можно что-то сделать, и предупреждение об устаревшем чтении остаются там, где они помогают. Подробное состояние и время проверки остаются в диагностике и не влияют на маршрутизацию.

- Приложение и сервер Relay делят обновление квоты и моделей аккаунта между фоновыми задачами и ручными запросами. Списки квоты и моделей обновляются независимо. Повторные обновления соблюдают паузу провайдера, а закрытие ожидающего запроса не отменяет общую работу. Подготовка авторизации для первичной квоты, моделей и кредитов сброса идёт отдельной задачей по требованию с зарезервированной ёмкостью. Учётные данные не попадают в кэш наблюдений. Запоздалый результат и ошибка не затирают более новый вход, настройку прокси или заменённый аккаунт. Более новая квота из запроса моделей важнее более старого чтения. Свежие заголовки квоты из запроса генерации могут отложить следующую автоматическую проверку, если сведения о подписке актуальны. Ручное обновление и проверка сброса остаются в расписании. Сервер Relay также проверяет квоту вскоре после сообщённого будущего сброса. Запоздалый ответ 401 со старым ключом не может сделать недействительными более новый токен или заменённый вход во время восстановления квоты или моделей. Запоздалая запись OAuth и задач агента не заменяет учётные данные, установленные последующим импортом. Старая сборка среды выполнения не может опубликовать состояние после этого импорта или удаления аккаунта. Повторный импорт и удаление на сервере закрывают старый маршрут аккаунта до смены сохранённых учётных данных. Неудачное удаление из хранилища восстанавливает запись и не открывает заново ожидающую работу старой среды. Управляющие запросы к провайдеру идут через общую ограниченную очередь HTTP с резервом для входа и восстановления. Изменённый аккаунт или источник не может отправить устаревшее чтение из очереди. Более старое обновление приложения или ожидающая запись токена не может заново создать учётные данные после удаления аккаунта и его повторного добавления.

- Обновление 1.1.3 при запуске само переводит существующие профили пула на текущую ротацию, сохраняя настройки участников и включённое состояние шлюза. Отдельное подтверждение, уведомление, ручная остановка, запуск или откат не нужны. Устаревшие пороги ошибок и параметры ранжирования отбрасываются. Старые профили и пресеты по-прежнему открываются. Старый сервер не принимает неподдерживаемые настройки ротации.

- Перенос аккаунтов на свой сервер закрывает ожидающую локальную отправку через импорт и проверенную очистку. Прерванный перенос или аккаунт, уже принадлежащий серверу, не может снова появиться в локальной маршрутизации после перезапуска. Неудачная локальная активация оставляет владение в восстановлении и не включает две копии.

- Если удаление локального аккаунта не удалось и его учётные данные или профиль нельзя восстановить, локальный шлюз останавливается и не обслуживает устаревший маршрут.

- Ожидание ёмкости и восстановления использует общие пределы числа и сохранённых байтов для среды и для каждого ключа, пробуждение по событиям и один накопленный бюджет ожидания на повторы и смену транспорта. Занятая ёмкость распределяется между ключами справедливо. Отмена освобождает учёт очереди и не штрафует провайдера. Когда заняты все физические слоты, новый запрос больше не просматривает всю очередь ожидания заново. Неподдерживаемый маршрут по-прежнему завершается ошибкой, не входя в эту очередь.

- Выбор в автоматическом режиме использует нормализованную локальную загрузку. Физический участник делит ёмкость между псевдонимами протоколов. Повторы используют один бюджет запроса через транспорты и исправления. Неопределённый результат провайдера не повторяется молча. Обязательная пауза провайдера устанавливается до того, как слот станет доступен другому запросу. У API-источников сервера больше нет второй, отдельно отсчитываемой блокировки шторма вне ротации пула. Ожидающая отправка не может использовать старый маршрут аккаунта или источника приложения, пока меняются его участие, адрес, прокси, вход или разрешения. Уже начатая работа может завершиться.

- В сведениях об использовании примерный срок кэша стоит в скобках рядом с чтением кэша, а если чтения нет — рядом с записью. Окно, которое сообщил провайдер, используется как есть. Для GPT-5.6 и новее при отсутствии окна берётся документированный минимум OpenAI в 30 минут. Остальные пропуски помечаются как несообщённый срок. Оценка считается от последней записи или чтения кэша в этом запросе и не является живым сроком у провайдера.

- Порядок каталога держит семейства одной номерной линейки вместе и использует устойчивые идентификаторы семейств. Более поздняя дата выпуска сама по себе больше не поднимает GPT-6 Sol выше GPT-6 Astra.

- Фоновое обновление больше не может заменить сохранённые настройки более старым снимком. Уход с подключения и возврат к нему также сбрасывают ожидающее обновление, поэтому медленный предыдущий запрос не блокирует текущий экран. Открытые страницы удалённого пула подхватывают изменения сервера периодически и при возврате к окну, без локального события об изменении состояния.

- В настройках API есть небольшой переключатель стандартной или автоматической оптимизации инструментов для локального пула и совместимого сервера. Стандартный режим передаёт полный каталог. Автоматический режим применяет отложенный поиск инструментов нативного Responses к каждому подходящему запросу и один раз повторяет запрос без этих полей, если выбранный адрес их не поддерживает. Пороги и списки разрешения или запрета по именам больше не показываются. Переключатель сохраняется сразу. Полное поведение описано в справке.

- Обновление каталога пула один раз определяет маршруты участника для всех его моделей и не повторяет политику обнаружения по мере роста списка. Отключённые от сети модели сохраняют редактируемые сведения и поля цены кэша. (`3e6e174`)

- Схемы инструментов Codex сохраняют область ссылок, ограничения проверки и буквальные данные, ограничивая раскрытие ссылок, чтобы глубокие или ветвящиеся описания не раздували память. (`3e6e174`)

- Преобразованные ответы Chat Completions сохраняют текст размышления рядом с вызовами инструментов в JSON, потоке и последующей истории. Непрозрачное состояние размышления по-прежнему требует своего нативного владельца. (`e211560`)

- Повтор настройки источника после неудачного снимка или обновления участия в пуле использует уже сохранённый источник и не создаёт дубликат. Убраны неиспользуемые элементы и вспомогательный код интерфейса для прежнего порядка ролей API и ручного редактора маршрутов. (`54b0c75`)

- Долгая генерация больше не обрывается из-за таймеров HTTP, SSE или WebSocket самого Relay. Relay ждёт провайдера, поддерживает поток и сохраняет отмену клиентом и повторы для настоящих сбоев до появления ответа. (`3e6e174`)

- Каталоги-справочники удерживают меньше памяти между обновлениями. Данные источника остаются компактными, производные кэшированные данные не читаются при загрузке, а сохранение каталога больше не загружает в память ещё одну полную копию предыдущего файла.

- Запуск и обновление каталога делают меньше работы: совпадения моделей индексируются один раз, неизменяемые справочные данные используются совместно. Обновление больше не сливает один и тот же каталог дважды и не дублирует его дерево JSON в памяти.

- Исправлен аварийный выход Windows при обновлении участников пула. Массовое обновление квоты сохраняет ограниченную параллельность и возвращает результат каждого аккаунта, не переполняя стек команды приложения.

- Запуск Codex применяет ожидающие обновления каталога моделей до открытия клиента. Подключение локального пула проверяет каталог до закрытия Codex, а запуск клиента больше не ждёт промежуточного обновления интерфейса. Ошибка подготовки профиля надёжно снова открывает ранее работавший клиент. Запросы каталогов аккаунтов идут параллельно в общем пределе времени, а сверка истории не перечитывает каждый файл только ради отпечатка. Успешное подключение повторно использует свой каталог при запуске. Ошибка настройки OpenCode больше не мешает обновлению каталога Codex.

- Повторное подключение Codex больше не срывается из-за того, что уже совпадающая история чатов превышает предел исправления истории. В бюджет перезаписи входят только файлы, которым нужна смена провайдера, поэтому обновление модели и скорости завершается без резервного копирования неизменённых разговоров.

- Сведения о моделях берутся из общего справочного каталога для аккаунтов и API-провайдеров. Если метаданных нет, применяются одинаковые значения по умолчанию. Поля возможностей участника больше не скрывают размышление или инструменты, лишний опрос метаданных убран. Цена провайдера по-прежнему важнее, если она передана.

- Модели OpenAI предлагают Обычную, Быструю и Сверхбыструю скорость по правилу Relay, даже если аккаунт или провайдер не вернул поля скорости. Явный выбор скорости сохраняется при ротации и перекрывает значение пула по умолчанию. В каталоге скоростей есть описания, поэтому у Сверхбыстрой в Codex больше нет пустого пояснения.

- API-источники, сохранённые прокси и автоматизации квоты используют более читаемую раскладку и компактные редакторы. Импорт прокси может сразу проверить новый адрес и показать фактический выходной IP, страну и время ответа. Неудачная проверка сохраняет прокси и назначения. Заявленное место показано отдельно от наблюдаемого. Правила квоты выполняются сами, без отдельной кнопки запуска и выбора режима. Прежние ручные правила обновляются, отключённые остаются отключёнными. Имена правил по умолчанию отличают запуск отсчёта квоты от сброса недельного лимита, в том числе у уже существующих правил. Свои имена не меняются. Редактор начинается с типа автоматизации и нужных ей полей. Своё имя правила необязательно. Общие подписи автоматизаций больше не говорят только о квоте. На вкладке автоматизаций нет общей строки поиска и обновления: список обновляется при изменении правила. Частые действия в строке — значки с подсказками. В строке API-источника запуск стоит перед правкой, дополнительные действия — последними. (`0b65f07`, `54b0c75`)

- При создании источника выбор провайдера остаётся над одним столбцом полей ключа, адреса и имени. Быстрая настройка собирает ход, выбор и переход в компактной области и не повторяет логотип окна. Настройка своего API показывает все обязательные поля и позволяет продолжить после их заполнения. Повторный выбор уже активного провайдера сохраняет правки. (`54b0c75`)

- Вкладка API собирает состояние, счётчики пула, адрес и ключ в одной панели подключения с подписанным копированием. Порт настроен в своей строке, а перевыпуск ключа доступен из меню действий ключа.

- Добавление подключений в пул использует один список с поиском, разделы аккаунтов и API, вид выбранных подключений и закреплённые действия добавления. Выбор не сбрасывается при фильтрации.

- Нативный запрос больше не проваливает проверку совместимости адаптера только потому, что каталог источника пропустил уровень размышления или ошибочно пометил возможность как неподдерживаемую. Преобразованные маршруты сохраняют свои проверки возможностей и размышления.

- Настройки больше не повторяют под переключателем уведомление о включённом подробном режиме и кнопку папки операций. Открыть папку по-прежнему можно в диагностике.

- Сигналы поддержания потока ждут границы целого события, чтобы пауза в разорванном ответе не вставила служебный кадр внутрь его JSON.

- Неверные события потока сохраняют безопасную диагностику разбора в сведениях о запросе: категорию ошибки JSON, позицию и размеры кадра, без записи содержимого ответа. Эта диагностика отличает разбор Relay от сообщения об ошибке провайдера.

- Поток со смешанными окончаниями строк LF, CRLF и CR больше не склеивает отдельные события и не завершается ошибкой `stream_invalid`. Общий разбор сохраняет порядок событий для нативных потоков, адаптеров протоколов и сжатия контекста.

- Преобразованные запросы Codex принимают трассировку клиента, ключи привязки кэша и необязательный выбор зашифрованного вывода размышления. Обычный текст и пустое управление размышлением больше не исключают иначе совместимый маршрут. Неподдерживаемый параметр называет поле. Зашифрованный ввод по-прежнему требует нативный маршрут.

- Сжатие контекста Codex принимает завершённые элементы вывода, пришедшие до последнего события потока. Маршрутизация аккаунта держит в памяти только инфраструктурный cookie ChatGPT, отдельно для аккаунта и авторизации. Продолжение WebSocket после смены учётных данных переподключается только при переносимой истории. Запоздалое обновление каталога не может перезаписать привязку другого профиля Codex.

- Relay принимает JSON-запросы gzip и zstd от текущих клиентов. Сжатие аккаунта переходит на механизм сжатия Responses, только если старый адрес сжатия явно недоступен, и сохраняет контекст и использование. Подсказки состояния хода остаются у точных сеанса, модели и учётных данных аккаунта, включая обновление авторизации и одновременные ответы.

- Видимость модели, разрешения моделей участника, вывод из ротации, предпочтение запуска и включение автоматизации используют тот же вид переключателя, что и настройки приложения.

- Правила моделей пула могут сбросить порядок моделей и групп к каталогу по умолчанию. Сначала идут OpenAI, Anthropic, Google и xAI, затем остальные компании по алфавиту. Внутри компании семейства каталога держатся вместе, более новые версии — первыми внутри семейства, одинаково у всех провайдеров. Новые семейства группируются сами по метаданным. Неудачная перестановка возвращает показанный сохранённый порядок, а вторая перестановка не накладывается на ещё не сохранённую.

- Обновление каталога сохраняет цены и справочные уровни размышления, даже если старые списки маршрутов или объявления адресов неполны. Проекция клиента пула следует автоматической маршрутизации, а преобразованные запросы сохраняют цены источника во всех четырёх протоколах. Фоновое обновление сервера также обновляет метаданные протокола, не затирая одновременные правки и не возвращая удалённые источники. Codex получает общие справочные уровни размышления через подходящие маршруты Responses, включая преобразованные модели, с учётом областей ключа и ограничений адаптера.

- Правила моделей пула сохраняют имена, группы, цены, режимы размышления и сохранённый порядок для всех моделей аккаунтов и API в пуле, даже если участник отключён или недоступен. Одинаковые идентификаторы моделей показаны одной строкой и считают только участников пула.

- Подписи моделей Codex используют общее справочное имя, а затем компактную подпись из идентификатора модели, одинаково в пуле и при прямом подключении API. Справочное имя не портится из-за повреждённых метаданных участника. Верная карточка аккаунта-владельца важнее несовместимой. Идентификаторы моделей и порядок пула сохраняются.

- С карточек и меню аккаунтов убрано техническое действие принудительного обновления входа. Восстановление входа остаётся в обычном входе, а обновление квоты — единственное видимое действие обновления.

- Восстановление пула автоматическое: рассматриваются все совместимые участники, временный сбой даёт паузу не меньше пяти секунд, повторные сбои увеличивают паузу. Недоступная модель, отклонённые учётные данные и непрозрачный отказ шлюза больше не мешают перейти к другому подходящему API или аккаунту. Ручные настройки числа повторов и паузы последнего кандидата убраны.

- Английская и русская справка переписаны вокруг текущего порядка подключений, режимов, ротации пула, квот, балансов и восстановления. Добавлено связанное оглавление, исправлены устаревшие пути настроек и описания поведения. У справки есть колонка чтения, закреплённое оглавление с текущим разделом, нумерованные шаги настройки и строки ошибок, которые помещаются в узкое окно. На маленьком экране оглавление становится компактным меню. Справочник ошибок группирует точные коды с причинами и конкретными шагами восстановления, категории раскрываются, доступен поиск по коду или симптому.

- Сброс недельной квоты использует компактную строку действия и отдельный счётчик кредитов на карточках аккаунта и пула.

- Правила участника пула открываются компактным списком моделей с одним переключателем на модель. Раскрываемые группы провайдеров сохраняют отключённые модели, длинные списки можно искать. Цены API выровнены столбцами рядом с моделью, включая запись кэша на 5 минут и 1 час. В узком окне цены подписаны по строкам. Вторичные настройки выровнены на своей вкладке. Порядок моделей аккаунта и API следует метаданным и сохранённому ручному порядку, не перемещая отключённые модели и не ставя выше переопределение цены.

- У аккаунтов и API-провайдеров три режима ротации пула: Автоматически, По порядку и По кругу. Сохранённый Smart при обновлении становится Автоматически. Один переставляемый список заменяет отдельные роли API, с долями участников и общими пределами запросов. Настройки применяются, не прерывая уже идущие запросы. Недоступные участники не блокируют остальных. Одновременные правки обнаруживаются до сохранения, пресеты сохраняют смешанный порядок. В редакторе одна полоса прокрутки, короткие подписи состояния и элементы, нужные выбранному режиму. По центру стоят доля запросов и одновременные запросы, для снятого предела показано «Без ограничений». Подробности — в справке. Автоматический режим выбирает наименее занятых подходящих участников и использует долю запросов только при равной загрузке. Он не ранжирует по ручному порядку, балансу или проценту квоты. По порядку сохраняет ручную очередь. Режимы выбираются с клавиатуры. Недавний сбой теряет штраф планирования в течение минуты, поэтому восстановившийся участник может вернуться сам. Завершение более старого запроса не освобождает чужую пробу восстановления. Занятые пробы допускают ограниченное ожидание. Изменение состава пула на сервере сразу применяет сохранённый порядок и пределы запросов. Подсказка следующего кандидата следует планировщику и не показывается, если выбор зависит от модели или формата. Устаревшие события активности не затирают состояние более новой среды выполнения.

- Управление пулом использует компактную панель, отдельную строку текущего и следующего маршрута и рамку с затенённой полосой состояния и разделителями между счётчиками. Подключения используют ту же панель для поиска, фильтров и действий аккаунта. Обе сводки сохраняют общую сумму кредитов провайдера, если она есть. Полные имена маршрутов и моделей переносятся на узком экране. У действий-значков остаются подсказки.

- Скорость запросов пула — один перетаскиваемый переключатель на три положения: Обычная, Быстрая и Сверхбыстрая, вместо меню и отдельного выключателя. Компактная подпись показывает только выбранный режим, переход плавный и учитывает настройку уменьшения движения. Выбор доступен с клавиатуры и касанием. Перетаскивание сохраняется при отпускании.

### Добавлено

- Пул одновременно обслуживает Responses, Chat Completions, Messages и Gemini, с нативными путями и поддерживаемым преобразованием для JSON и потоковых запросов.

- Маршрутизация API-источника полностью автоматическая. Relay не показывает выбор адаптера, предпочитает объявленный нативный адрес каждой модели и при необходимости преобразует запрос из Responses, Chat Completions, Messages или Gemini. Chat Completions поддерживает функции-инструменты и историю их результатов.

- Новый API-источник определяет форматы сам по объявлениям провайдера и настройкам адреса. Каждая модель каталога остаётся маршрутизируемой через запасной путь источника, если объявлений нет. Старые поля ручного режима принимаются при импорте и игнорируются. Прежние назначения физических адресов остаются подсказкой запасного пути, поэтому источник со смешанными протоколами продолжает работать после обновления.

- OpenCode использует группы SDK по протоколам и сохраняет рабочие идентификаторы моделей и пользовательские параметры при обновлении каталога. Codex использует поток HTTP, когда модели нужно преобразование, а нативные соединения WebSocket по-прежнему поддерживаются.

- Карточки API распознают балансы Sub2API, New API, совместимого биллинга One API, DeepSeek и SiliconFlow. Статистика OpenRouter работает с обычным ключом генерации. Карточки различают кошелёк, остаток ключа, подписку, валюту и собственную оценку использования Relay, не показывают внутренние подписи адаптера и отсутствующее число запросов и помечают неудачное обновление, не отбрасывая последний известный баланс. Ход обновления виден на анимации кнопки, без второй строки состояния под счётчиками.

- Сведения о неудачном запросе показывают исходные код, тип, сообщение и статус HTTP провайдера отдельно от категории Relay. Сообщение можно скопировать. Секретное содержимое скрыто, длинное сообщение ограничено. У старых записей явно показано, что сообщение провайдера не было сохранено.

- В настройках есть **Диагностика**. Relay ведёт отдельные ограниченные по размеру и очищенные журналы ошибок, аварийных завершений и важных этапов операций. Каждую папку можно открыть из приложения. Подробный журнал операций выключен по умолчанию и включается там же, когда нужно разобраться в сбое.

- Импорт переносимых аккаунтов сохраняет безопасное имя и ограниченные
  неповторяющиеся метки, не раскрывая учётные данные. При повторном импорте
  метки Relay остаются главными.

### Исправления

- Блоки запроса Gemini и пустые кандидаты, отфильтрованные или ограниченные по токенам, доходят до клиента как неполный ответ в JSON и в потоковом преобразовании, а не как повреждённый вывод провайдера или успешное завершение.

- Действия на карточке аккаунта разделены и выглядят одинаково, без вложенной рамки заголовка. Состояние квоты, требующее входа, имеет более понятную цель для клавиатуры и указателя.

- Режимы ротации пула сохраняются при первом использовании и когда участники входят или выходят, и не возвращаются к Smart из-за устаревшего сохранённого списка. Одновременные правки по-прежнему обновляются и безопасно повторяют сохранение. ([#74](https://github.com/F0RLE/zenith-relay/pull/74))

- Выбор аккаунта ChatGPT и резерв квоты находятся в одной компактной панели. Переключатели возможностей — плоские строки с коротким описанием. Настройки участника пула больше не добавляют вторую рамку внутри окна. Подзаголовок вкладки OpenCode говорит «Использовать пул в OpenCode». ([#74](https://github.com/F0RLE/zenith-relay/pull/74))

- Имена моделей GPT в ChatGPT и Codex сохраняют исходные идентификаторы, даже если у выбранного аккаунта нет подходящей карточки каталога. Явные префиксы ключей и существующие псевдонимы поддерживаются. Возможности берутся у совпавшей модели. ([#74](https://github.com/F0RLE/zenith-relay/pull/74))

- Восстановление истории инструментов Responses сохраняет связь вызова и результата, если клиент ссылается на идентификатор элемента или не передаёт идентификатор вызова. Результаты сопоставляются по виду и пространству имён, без удаления истории и без угадывания среди параллельных вызовов. HTTP, поток и WebSocket используют одно и то же ограниченное восстановление. ([#74](https://github.com/F0RLE/zenith-relay/pull/74))

- Ротация пула сразу сохраняет режим, порядок и настройки участников. Перетаскивание работает одинаково, одновременное изменение состава обновляет редактор, а конфликт сохранения повторяется и не возвращает удалённых участников. ([#74](https://github.com/F0RLE/zenith-relay/pull/74))

- Действия правил моделей стоят в одной выровненной группе, элементы формата, размышления, скорости и включения одного размера. ([#74](https://github.com/F0RLE/zenith-relay/pull/74))

- Поля цены провайдера имеют одну скруглённую обводку, отдельный знак валюты и числа, выровненные вправо. Фокус и неверное значение подсвечивают всё поле одинаково в обеих темах. ([#74](https://github.com/F0RLE/zenith-relay/pull/74))

- Подписи скорости пула и сводки подключений помещаются при более широком системном шрифте и не обрезаются и не переносятся без нужды на обычном экране. ([#74](https://github.com/F0RLE/zenith-relay/pull/74))

- Несколько маршрутов одного участника делят вес ротации и ёмкость запросов. Неподдерживаемый адрес не отключает остальные форматы участника. Сбои квоты и авторизации остаются общими. Повторы продолжения сохраняют полную историю, результаты инструментов и непрозрачное владение.

- Преобразованный запрос отклоняет неподдерживаемые параметры, а не отбрасывает их молча. Схемы сохраняют ограничения, инструкции одного хода не переносятся в следующие, уровни размышления предлагаются только там, где их можно сопоставить. Потоковые инструменты сохраняют идентификаторы и порядок. Использование отражает фактический формат провайдера и не выдумывает отсутствующие счётчики.

- Привязка кэша больше не выбирает участника вне лучшей доступной группы и не обходит учёт ротации.

- Задержка повтора, которую сообщил провайдер, соблюдается для перегруженных и недоступных моделей и других ошибок службы, в том числе если настроена более короткая задержка восстановления.

- Обработка TLS обновлена до rustls 0.23.45, чтобы закрыть RUSTSEC-2026-0285.

- Пул больше не сообщает, что недоступны все участники, если телеметрия маршрутизации отсутствует или устарела, но готовые участники есть. Если готовых нет, предупреждение отдельно показывает ожидание квоты, недоступных и отключённых участников, с понятной причиной ошибки. Пул и Подключения группируют аккаунты по доступности и сохраняют порядок маршрутизации внутри группы.

- Ошибка провайдера внутри ответа HTTP 200, в JSON или в потоке, остаётся ошибкой в истории использования. Ограничение частоты и лимит расходов различаются. Явная ошибка проверки запроса не отправляет аккаунт на паузу.

- Общий отказ запроса провайдером или отключённая модель больше не блокируют переход к другому совместимому участнику. Ошибка службы на маршруте аккаунта сохраняет принадлежность аккаунту и не помечает аккаунт нездоровым.

- Продолжение инструментов проверяет всю историю, включая вызовы после первых шестнадцати. Завершённые старые вызовы больше не выбирают владельца нового результата инструмента, а выводы разных владельцев нельзя смешать неявно. Примеры в схемах инструментов и вложенные данные результата больше не влияют на маршрутизацию.

- Обновление моделей аккаунта больше не стирает независимую блокировку, требование входа или ошибку проверки и не понижает окончательную ошибку каталога после временного сбоя сети. Ошибка наблюдения за квотой больше не скрывает причину недоступности аккаунта в Подключениях и Пуле.

- Активность пула сопоставляет последний аккаунт или API-источник по идентичности, поэтому запросы с одинаковым временем не подсвечивают не того участника.

- Восстановление сохраняет исходную привязку ответа для других веток чата и повторов и использует параметры текущего запроса, не возвращая старые инструкции или настройки транспорта. Ссылки провайдера на разговор и элементы или результаты инструментов без их вызовов больше не считаются полным локальным повтором.

- Чат с переданным сжатым контекстом может продолжиться без устаревшей ссылки на ответ по HTTP или WebSocket. Сжатое окно и сохранённые результаты инструментов остаются. Нечитаемое сжатие больше не отбрасывается молча.

- Восстановление чата требует сохранённую цепочку разговора, прежде чем убрать прежнюю ссылку на ответ, если не передано полное сжатое окно. Так прежний контекст не теряется молча.

- Ручное обновление учётных данных и наблюдаемый вход клиента возвращают аккаунту возможность маршрутизации. Карточки удалённых аккаунтов показывают паузу среды выполнения и недоступные маршруты.

- Восстановленные аккаунты пула остаются доступны, не удаляя другого участника. Запоздалое обновление авторизации больше не отменяет более новое отключение аккаунта. Карточки аккаунтов показывают временную недоступность среды во время паузы.

- Импорт аккаунта или API-источника без добавления в пул больше не перезапускает работающий локальный шлюз. Посторонний перезапуск приёмника не прерывает Relay при импорте только в список.

- После прерванного запуска или импорта диагностика при следующем старте записывает последний очищенный этап операции, даже если подробный журнал был выключен.

- Чат, у исходного аккаунта которого кончилась квота, может продолжиться через следующий совместимый здоровый аккаунт или API-источник. Это работает и для обычных запросов Responses, и для WebSocket.

- Чат может восстановиться после истёкшей ссылки на ответ по локально сохранённой истории Relay, в том числе на уже открытом WebSocket. Полная история инструментов может перейти вместе с пулом. Неполное состояние инструмента больше не удаляется молча при восстановлении.

- Неизвестная ссылка на ответ отклоняется до обращения к несвязанному аккаунту. Клиент может отправить полную историю без старой ссылки. Иначе Relay сообщает, что контекст продолжения недоступен.

- Аккаунт, которому нужен вход, который на паузе или у которого ошибка, отключает только этого кандидата, в том числе если статус изменился при уже работающем локальном шлюзе. Остальной пул и его доступные модели продолжают ротацию.

- Импорт аккаунта и очистка OAuth больше не оставляют устаревшее временное состояние, которое может закрыть Relay или заблокировать следующий импорт. JSON-аккаунт можно добавить в Relay, не добавляя его в пул.

- Обновлённые кредиты аккаунта и пределы квоты применяются к работающему пулу, в том числе когда Relay открыт только в трее. Как доступный баланс показываются только настоящие кредиты провайдера.

- Фоновые проверки пробуждения ChatGPT используют живую среду шлюза, включая обновление токена, паузу, использование и диагностику, и остаются закреплены за аккаунтом, который выбрал планировщик.

- Модель можно вернуть на Обычную скорость, пока её маршрут временно недоступен. Быстрая и Сверхбыстрая заново считаются для каждого кандидата повтора, поэтому неподдерживаемая скорость не переносится на следующий аккаунт или API-источник.

- Правила моделей сохраняют каждую модель участников пула, включая отключённых и временно недоступных. Доступность запроса проверяется при выборе участника, поэтому отсутствие маршрута не удаляет строку модели.

- Сохранение неполной перестановки правил моделей сохраняет недоступные модели и модели, которые есть только как привязка, и не считает их удалёнными.

- Карточки Пула и Подключений одинаково показывают сведения об аккаунте и управление повторным входом. Недоступные аккаунты остаются видны со своим статусом, и из-за них весь пул не выглядит недоступным.

- На странице завершения OAuth больше нет кнопки закрытия, которая не может сработать.
## [1.1.2] - 2026-09-03

Zenith Relay 1.1.2 improves provider failover, usage visibility, application
integration, recovery, and the local-first desktop workflow.

### Security

- Local and user-managed server API keys are fetched only after an explicit
  **Copy API key** action. Relay does not render or retain them in the desktop
  interface.
- API keys can be reissued from the API page. The replacement is copied
  directly, and the previous key is invalidated only after the new one is ready.

### Routing, availability, and usage

- Relay preserves a ChatGPT session's preferred healthy route to retain upstream
  prompt-cache affinity, while still allowing bounded fallback when capacity,
  quota, or health requires it.
- Pool activity now follows the runtime's reported candidate, including a
  lower-priority stabilizer, and remains visible across concurrent state
  refreshes instead of predicting a "next choice" from the card order.
- A confirmed quota exhaustion cools down only the affected account/source and
  model route instead of taking the whole provider out of rotation.
- Namespaced API models stay on their declared API source; Relay no longer
  silently falls back to a ChatGPT subscription route for those requests.
- Unclassified provider rejections before response data reaches the client now
  cool only the affected source/model route and continue with the next eligible
  source. Known request, tool-call, context, and continuation errors remain
  terminal, and an active response is never switched mid-stream.
- Source capability refreshes use the live upstream catalog. OAuth refresh-token
  reuse is treated as a temporary recovery state rather than forcing an
  unnecessary sign-in.
- Adding an account or refreshing quotas now updates its available models in
  the same operation. API-source refresh updates balance, models, prices, and
  reasoning capabilities together, including an authoritative empty catalog.
- Temporary network and provider failures no longer label an account as
  blocked. Usage history keeps a stable, redacted label for removed accounts
  and reports the service tier observed from the upstream response.
- The pool now exposes two request-speed states: Standard and Fast. Pool-managed
  OpenAI requests follow the selected state, while external API clients retain
  their explicit tier; the upstream-reported tier remains a diagnostic value.
  The speed control and persisted per-model policy are limited to OpenAI-family
  models; legacy Fast overrides for other families are ignored safely.

### Pricing

- Replaced the hand-maintained OpenAI price file with one validated LiteLLM
  catalog shared by desktop and Relay Server. Account estimates use only an
  exact record in the account's declared official family; API sources resolve
  provider evidence, LiteLLM exact, declared-family canonical, then a manual
  source value.
- Kept input, cache-read, cache-write (5m/1h), output, and image/request
  tariffs independent. Missing components remain unpriced instead of falling
  back to another tariff or displaying `$0`.
- Usage now normalizes OpenAI-style inclusive cached input and
  Anthropic-style separate cache reads/writes into one breakdown. Cached and
  reasoning tokens are shown as subsets where required, zero-only rows are
  omitted, and totals no longer double-count those components.
- Added immutable catalog snapshots, local ETag/Last-Modified cache metadata,
  stale/offline fallback, atomic replacement, and revision-aware invalidation
  for usage and API-equivalent totals. Pricing remains informational and never
  changes routing or quota decisions.

### Model catalog

- Added a separate `models.dev` metadata catalog for provider, family, display
  name, release dates, and safe descriptive capabilities. LiteLLM remains the
  only source of pricing; metadata never adds models, enables routing, or
  changes reasoning and availability decisions.
- Model metadata is loaded from the local cache at startup and refreshed in the
  background with conditional HTTP validation. Stale or unavailable metadata
  remains usable, and backend-owned family/order data is shared by all model
  pickers without frontend name heuristics.

### Desktop UI

- Account cards show a **Credits** row and the pool shows **Total credits**
  when the connected provider explicitly reports a positive or unlimited
  ledger. Values use at most one decimal place; zero or missing ledgers stay
  hidden and do not affect billing or Relay's API-equivalent calculation. When
  quota headroom is otherwise tied, the pool prefers the larger fresh ledger
  before continuing its normal fair rotation.
- The Overview application launcher now only starts an already connected
  application; the connect-time launch preference is shown only during setup.
- ChatGPT recovery provides named, protected snapshots of `config.toml` and
  sign-in state with confirmed restore and deletion. Managed profile switching
  still preserves unrelated settings, rejects a newer manual sign-in, and uses
  reversible history repair in both directions.
- Pool connections now include OpenCode. Connecting writes a managed
  `zenith-relay` OpenCode provider with the currently enabled pool models and
  preserves the previous OpenCode configuration for one-click recovery. An
  authoritative empty pool now replaces stale OpenCode models with a zero-model
  catalog instead of leaving an obsolete selection behind.
- Relay-managed OpenCode models now advertise image attachments, so image
  inputs can be selected and forwarded through the pool.
- OpenCode reasoning variants follow the model's explicit Relay reasoning
  policy, so unsupported effort choices are not advertised to the client.
- The application picker remembers whether a connected application should be
  launched immediately, and its compact layout remains consistent across
  desktop window sizes.
- API setup separates the local endpoint, ChatGPT, and OpenCode responsibilities
  while keeping address, port, and copy-only key actions together. ChatGPT and
  OpenCode launch only when the user enables the remembered launch option.
- Tooltips now wait briefly for pointer hover, appear immediately for keyboard
  focus, and remain available for disabled actions without browser-native
  `title` popups.
- The startup screen and application chrome no longer allow accidental text
  selection, while editable fields remain selectable.
- Usage refreshes no longer wait on an extra UI delay, while sign-in, setup,
  and snapshot countdowns share one deadline-based clock instead of separate
  polling intervals.
- The Usage view warms its default report after the runtime is ready and keeps
  a small per-query result cache, so returning to the page renders immediately
  while the latest aggregates refresh in the background.
- Update discovery starts with the application instead of an arbitrary startup
  delay, and compact icon-only controls keep their accessible action names.
- Runtime and usage refreshes are limited to the pages that consume them;
  revision checks, coalesced events, cached aggregates, and bounded clocks avoid
  rebuilding long request tables or blanking Overview during background work.
- OpenCode recovery now mirrors the ChatGPT recovery view with an explicit
  original-config snapshot, creation date, path, and confirmed restore action.
- Relay-owned recovery files now use an application-first layout under
  `recovery/applications`, while legacy recovery directories migrate safely on
  startup without overwriting conflicts.
- Pool configuration can be exported without credentials, previewed as a diff,
  and applied only to unambiguous existing accounts and sources. Desktop and
  user-managed server imports share the same validation and legacy-field merge
  behavior.
- Help, planning, and release documentation now describe the current ChatGPT
  and OpenCode integrations and the actual local storage boundaries.

## [1.1.1] - 2026-08-28

Zenith Relay 1.1.1 is the maintenance release after 1.1.0. It improves
multi-protocol reliability, account discovery, cache-aware routing, quota
visibility, and the everyday desktop workflow without changing the product's
local-first security boundary.

### 1.1.0 -> 1.1.1 at a glance

| Area | Changes in 1.1.1 |
| --- | --- |
| Account access | More reliable ChatGPT subscription discovery, exact-account reauthentication, and preserved catalogs during temporary checks |
| Routing | Stable source ownership for continuations, safer WebSocket recovery, and cache affinity that survives restarts |
| Providers | More complete Responses bridges for Anthropic Messages and Gemini, including tools, streaming, images, thinking, and usage |
| Quotas | Weekly reset-credit automation and clearer separation between provider quota and Standard/Fast request speed |
| Usage | API-equivalent remaining estimate, request-count and E2E-speed analytics, and safer route diagnostics |
| Desktop UI | Clearer source tabs, reliable drag-and-drop policy editing, compact model rules, responsive dialogs, and refreshed help/screenshots |

### Account discovery and recovery

- Newly added ChatGPT subscriptions discover their model catalog with the same
  registered Codex authorization used by the runtime. Accounts that require
  Agent Identity now show quota and models together, with OAuth Bearer kept as
  a safe fallback when no Agent Identity is available.
- A temporary quota or model-discovery failure keeps the last usable catalog
  instead of making an account appear empty.
- Reauthentication can target the exact expired ChatGPT account. A fresh OAuth
  login keeps local routing and settings, and does not invent an expired
  subscription date without new provider metadata.

### Routing and provider compatibility

- Tool-call continuations from Messages and Gemini sources stay on the exact
  source route that created the response, preventing rotation from breaking an
  active task.
- Native Responses WebSocket requests recover strict provider-owned message
  identifiers in the same bounded way as HTTP. Parallel account-backed
  sessions keep their leases and response affinity independent.
- Relay-owned WebSocket timeouts and stream-size failures are reported as Relay
  errors instead of being attributed to a provider.
- Responses Lite follows the provider tool contract with explicit serial tool
  execution and rejects malformed values before forwarding them.
- Responses bridges now cover function, namespace, and direct custom tools
  across Anthropic Messages and Gemini, including tool-choice filtering,
  JSON/SSE continuations, multimodal input, thinking metadata, and normalized
  usage.

### Cache, quota, and usage

- Prompt-cache affinity keeps the original account preferred until a real
  failure, while source priority, exhaustion, health, and bounded spillover
  still apply. Opaque prompt/session bindings persist across Relay restarts,
  and rotating headers no longer split one session into separate cache keys.
- Server pools can automatically redeem an available reset credit when a
  configured weekly quota reaches zero, with per-account locking and cycle
  deduplication.
- The lifetime-based monetary "Potential" estimate is replaced by **API equiv.
  left**, shown only when Relay has complete priced usage for the current
  provider quota window. Activity outside Relay is excluded.
- Overview adds request-count and end-to-end output-speed charts for every
  selected period.
- Usage diagnostics show the attempt number, safe route kind, and endpoint
  route for each request, including failed attempts, without recording hosts,
  credentials, prompts, cookies, headers, or provider response bodies.
- Official OpenAI reference prices for GPT-5.6 Sol, Terra, and Luna now include
  cached-input and cache-write rates.
- Standard/Fast request speed is no longer presented as a second user-facing
  quota. Provider priority metadata does not create another quota meter.

### Desktop workflow

- Reopening ChatGPT and switching within the same adapter no longer rescans the
  full history. History repair now runs only when a profile crosses between
  OAuth, Relay, and API sources.
- API-source editing separates connection settings, model and format routing,
  and per-source pricing into focused tabs. Refresh checks only the saved
  source and stays beside its connection summary.
- Source policies support pointer-based reordering within roles and direct
  drops onto API-first, stabilizer, or last-resort roles. Saving closes the
  editor immediately while Relay persists the policy in the background.
- Pool model rules support pointer dragging with wheel scrolling, visible drop
  targets, and collapsible provider groups. Reasoning dialogs remain readable
  in compact and full-size windows with all backend-provided modes visible.
- README, localized Help, and Overview, Connections, Pool, and Usage screenshots
  were refreshed to match the current three-mode product.

## [1.1.0] - 2026-08-23

Zenith Relay 1.1.0 is the first complete Relay release after Zenith Codex
1.0.5. It changes the product from a small desktop API client into a
local-first personal relay for a user's own ChatGPT accounts and compatible API
sources. Relay is separate from the production Zenith Gateway, Control API, and
account pool.

### 1.0.5 -> 1.1.0 at a glance

| Area | 1.0.5 | 1.1.0 |
| --- | --- | --- |
| Product | Desktop client focused on a single API-key workflow | Local-first desktop relay with a private OpenAI-compatible endpoint |
| Operating modes | One desktop experience | This computer, Choose API, and My server |
| Accounts | Profile recovery and basic local state | ChatGPT OAuth, profile import, account health, quotas, pool membership, and routing |
| API sources | Limited source configuration | Responses, Messages, Chat Completions, and explicitly assigned Gemini routes |
| Models | Basic model presentation | Discovery, semantic ordering, capability-aware reasoning, and price provenance |
| Quotas | Status display | Provider windows, weekly reset credits, scheduled refresh, and confirmation-safe reset actions |
| Usage | Basic timing history | Token/cache/reasoning details, generation speed, E2E speed, and incremental totals |
| Recovery | Configuration repair | Snapshots, verified full restore, OAuth rotation recovery, and portable history repair |
| Deployment | Desktop release only | Cross-platform installers, signed updates, portable Windows replacement, and an optional user-managed server |

### Product and account management

- Added the three explicit Relay modes with local-first state, a generated
  loopback key, and capability-gated management of a server owned by the same
  user.
- Added ChatGPT OAuth sign-in, existing-profile import, account identity and
  availability state, pool membership, configured routing order, proxies, and
  reliable response continuity.
- Added provider quota windows in Connections and Pool. Provider quota,
  direct API-equivalent usage, and optional purchase-cost payback remain
  separate values; a quota percentage is never treated as money.
- Added explicit account export in several transfer formats. Account exports
  contain the OAuth credentials required for the selected import and must be
  handled as secrets. Diagnostics, snapshots, support bundles, telemetry, and
  usage history remain redacted: prompts, response bodies, cookies,
  authorization headers, and raw keys are not recorded there.

### Sources, models, and routing

- Added support for Responses, Messages, Chat Completions, and validated
  Responses-to-Gemini compatibility, including tool-call continuations.
- Added source model discovery, clear price provenance, image generation/edit
  prices, semantic model ordering, and declared reasoning catalog modes.
- Catalog refresh runs as an asynchronous conditional check at startup and
  approximately every 24 hours during an active app session. A deterministic
  spread prevents synchronized daily checks, while 5/30/120-minute retry
  deadlines remain exact after failures. Catalog failures stay visible after
  restart; reasoning modes remain catalog metadata and changing a reasoning
  setting does not probe a provider.
- Reasoning policies apply only to pooled API sources. Native OAuth models keep
  their provider capabilities unchanged.
- Added native WebSocket support and an HTTP/SSE compatibility path for
  providers that do not expose WebSockets.
- Routing follows the configured source order while keeping protocol
  continuations on the correct account.

### Quotas, resets, and usage

- Added explicit weekly reset-credit status and a simple Yes/No confirmation
  flow for an available reset. The automation path is weekly-limit aware; it
  does not confuse a five-hour window with the weekly reset.
- Background quota, model, and wake workers run only while an active Relay
  session is open. Tray-only startup does not perform provider checks.
- Added cache and reasoning token details, requested versus applied reasoning
  effort, provider generation speed, and full-request response speed.
- Usage totals remain available even after detailed request history is cleaned
  up according to its retention policy.
- Pool service tiers now use Standard/Fast terminology and synchronize Codex's
  official priority setting with the selected tier.

### Profile recovery and persistence

- Added reversible Codex profile attachment with one first-launch original
  snapshot, named snapshots, full restore verification, and a visible Yes/No
  confirmation. Hidden pre-restore copies are not created.
- OAuth rotation recovery adopts a newer token for the same account before
  restoring the profile, avoiding false restore failures.
- History repair updates only affected conversations and keeps recovery paths
  portable on Windows.
- Snapshot deletion and history-repair backups use bounded cleanup and explicit
  confirmation safeguards.

### Interface and desktop experience

- Added responsive English and Russian UI coverage for compact and desktop
  windows, a static startup screen, compact tables/cards, and shared dialogs for
  confirmations and errors.
- Model-availability and catalog errors remain visible instead of disappearing
  after a failed check. Global errors open in a centered details dialog and can
  be copied in a redacted form.
- Improved OAuth completion layout, source-route editing, usage request details,
  model price editing, pool card sizing, and semantic model-family ordering.
- Added signed in-app updates, in-place replacement and rollback for the
  portable Windows executable, and release artifacts for Windows, Linux, and
  macOS on x64 and ARM64.

### Optional Relay Server

- Added a standalone user-managed server with encrypted vault storage, durable
  state, management API, protocol negotiation, backup/restore, and strict
  redaction.
- The server is an optional personal deployment. It is not a connection to
  Zenith production systems, and live server acceptance remains a separate
  deferred gate.

### Security boundary

- Relay never receives Zenith production credentials, customer API keys,
  backend tokens, account-pool inventory, provider cabinet credentials, or
  internal Gateway/Control API business or routing logic.
- User-owned credentials can move only after an explicit confirmed transfer to
  that user's own server. Desktop secrets stay in the operating-system
  credential store; server secrets stay in the encrypted user-managed vault.

## [1.1.0-beta.1] - 2026-07-29

- First Zenith Relay product release, rebuilt from Zenith Codex 1.0.5.
- Added the local personal pool, compatible API sources, the three operating
  modes, reversible profile management, usage diagnostics, and the user-owned
  Relay Server.
- Added cross-platform desktop and server release artifacts, updater support,
  localized Help, and the initial production-readiness roadmap.
- This remains a beta: the real-account P0 acceptance path is not complete.

## [1.0.5] - 2026-07-07

- Added response timing to usage history.
- Fixed recovery from broken Codex configuration.
- Improved release version synchronization, updater-manifest publication, and
  release documentation.

## [1.0.4] - 2026-06-16

- Added API display balances.
- Standardized the main-branch and contribution flow for releases.

## [1.0.3] - 2026-06-09

- Published the third Zenith Codex release and fixed release-asset upload
  automation.

## [1.0.2] - 2026-06-07

- Published the second Zenith Codex release with the established desktop
  release artifacts.

## [1.0.1] - 2026-06-07

- Published the first maintenance release of Zenith Codex.

## [1.0.0] - 2026-06-06

- Initial Zenith Codex desktop release.

[Unreleased]: https://github.com/F0RLE/zenith-relay/compare/v1.1.5...HEAD
[1.1.5]: https://github.com/F0RLE/zenith-relay/compare/v1.1.4...v1.1.5
[1.1.4]: https://github.com/F0RLE/zenith-relay/compare/v1.1.3...v1.1.4
[1.1.3]: https://github.com/F0RLE/zenith-relay/compare/v1.1.2...v1.1.3
[1.1.2]: https://github.com/F0RLE/zenith-relay/releases/tag/v1.1.2
[1.1.1]: https://github.com/F0RLE/zenith-relay/releases/tag/v1.1.1
[1.1.0]: https://github.com/F0RLE/zenith-relay/releases/tag/v1.1.0
[1.1.0-beta.1]: https://github.com/F0RLE/zenith-relay/releases/tag/v1.1.0-beta.1
[1.0.5]: https://github.com/F0RLE/zenith-relay/releases/tag/v1.0.5
[1.0.4]: https://github.com/F0RLE/zenith-relay/releases/tag/v1.0.4
[1.0.3]: https://github.com/F0RLE/zenith-relay/releases/tag/v1.0.3
[1.0.2]: https://github.com/F0RLE/zenith-relay/releases/tag/v1.0.2
[1.0.1]: https://github.com/F0RLE/zenith-relay/releases/tag/v1.0.1
[1.0.0]: https://github.com/F0RLE/zenith-relay/releases/tag/v1.0.0
