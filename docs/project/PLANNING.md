# Zenith Relay architecture

Current implemented product contracts and limits. Source code and focused tests
define exact behavior; this document explains the shared contracts. Unfinished
work and acceptance gates are in [ROADMAP.md](ROADMAP.md), check commands in
[CONTRIBUTING.md](../../CONTRIBUTING.md), and user steps in Help. The separate
[pool rotation design](ROTATION_DESIGN.md) includes both connected components
and unfinished target requirements; it is not proof of full acceptance.

## Product and ownership

| Mode | Runtime and data |
| --- | --- |
| This computer | Desktop process, loopback endpoint, local protected storage |
| Choose API | Selected compatible API and its local secret reference |
| My server | User-operated Relay Server, encrypted vault, SQLite |

Relay is separate from production Zenith. A personal Zenith API key is an
ordinary user source; production credentials, customer inventory, and internal
Gateway/Control business logic never enter Relay. Transfer to the user's own
server is an explicit confirmed operation, not automatic synchronization.
Connectors use permitted authentication and automation paths. Relay does not
implement fingerprint/IP spoofing or sharing-concealment features to evade
upstream controls; a loopback endpoint does not authorize account resale.

| Path | Responsibility |
| --- | --- |
| `src/src` | React rendering, i18n, UI state, typed Tauri wrappers |
| `src-tauri/src` | Desktop I/O, credentials, OAuth, profiles, process lifecycle |
| `crates/relay-core` | Shared discovery, scheduling, protocols, gateway, quota, usage |
| `relay-server` | User-managed runtime, vault, persistence, management API |

Desktop and server share runtime contracts. React does not read secrets/files,
call providers, or implement routing. Closing the main window hides and reuses
its WebView while tray/background runtime survives; explicitly quitting the
process stops the local pool. Distributed multi-server coordination is not
implemented.

Renderer snapshot reads accept only the latest requested result. Mode changes
and explicit refreshes retire previous background reads and retries without
blocking new updates; events arriving during a read trigger one coalesced
follow-up. Visible remote runtime pages also poll once per minute and on focus
because server changes do not emit desktop events. Inactive pages do not poll
full runtime snapshots.

`relay-core/error_codes` defines stable error identifiers and upstream defaults
for public codes, HTTP status, and safe messages. Desktop and server reuse these
definitions; domain modules retain classification, retry, and recovery decisions.
Unknown provider codes remain distinct. Catalog tests require both localized
Help references to document each code and its public alias. Help renders those
same Markdown tables as searchable groups, loading only the selected local
language asset; it does not maintain a separate troubleshooting catalog.

## Storage and credentials

Desktop state uses the platform local-data location: normally
`%LOCALAPPDATA%\Zenith Relay` on Windows,
`~/Library/Application Support/Zenith Relay` on macOS, and
`$XDG_DATA_HOME/Zenith Relay` (normally `~/.local/share/Zenith Relay`) on Linux.
Within it, `data/database`, `data/vault`, `data/catalogs`, and
`data/migrations` keep durable Relay state separate; `cache` holds temporary
imports, OAuth state, locks, and the WebView profile; `exports` holds generated
deployment bundles; `logs/errors`, `logs/crashes`, and `logs/operations` hold
bounded redacted diagnostics. A tiny redacted last-stage marker survives an
interrupted session and is cleared after a clean exit; detailed operation
logging is opt-in through Settings; and `recovery` holds application-specific
backups and history-repair operations. The credential-store implementation
owns secret access. Codex's directory is touched only for reversible
integration.

Ordinary snapshots, usage, diagnostics, screenshots, and exports are redacted.
Explicit account export is a separate credential-bearing transfer document,
not a support bundle. Management tokens and pool request keys are distinct.
Server migrations are append-only; upgrades and backup/restore validate state
and encrypted references before activation.

## Connections, discovery, and metadata

ChatGPT is the shipped subscription connector. Accounts support OAuth, existing
local profiles, and compatible imports. API sources are generic records with
endpoint, secret reference, protocol bindings, models, priority/weight,
recovery settings, price evidence, and optional manual prices. Proxies are
optional and may be shared. No provider-name branch defines routing.

Stored-proxy diagnostics are explicit desktop operations using only the selected
proxy and a fixed HTTPS Cloudflare trace endpoint. Checks have bounded response
size, time and concurrency, with no redirect or direct fallback. The renderer
receives only observed IP/country, request duration and stable failure codes.
Session-only results do not change routing or assignments; username-declared
geography remains separate. Import can request checks for newly added entries.

Automation presentation uses shared type definitions for labels, conditions and
model-field visibility. Editors select the type first and accept an optional
custom name, falling back to the type's name. Current quota editors save
automatic execution only. Desktop loading converts
legacy confirmation rules and their unfinished cycles to automatic execution,
preserving enabled state, cycle deadlines, attempts and history. Execution stays
in the background workers; the Connections UI has no manual start controls.

Account/source inventory preserves all models the source actually makes
available, regardless of the currently implemented endpoint. Discovery does
not discard an entitled model because of an endpoint-support flag or a
hardcoded model allowlist. Explicit user filters remain separate policy.

Inventory and executable client projections are different: the current public
OpenAI model view uses enabled Responses/Chat candidates, while the managed
Codex projection requires Responses routes, including declared bridges and
in-pool accounts. Protocol compatibility is checked at the client/routing
boundary; it must not erase the underlying source inventory. Metadata and
prices cannot create a route.

The management pool catalog includes every model of each in-pool member,
including disabled, excluded, credential-less and temporarily unavailable
members. Desktop and server enrich that complete inventory with names, groups,
prices, reference reasoning modes and saved order before rendering it. Matching
IDs are deduplicated case-insensitively; distinct upstream IDs are not merged
by display name. Reasoning policy remains editable without a live route;
executable route projections and request admission enforce actual support.
Each snapshot resolves a member's protocol configuration once for the entire
catalog, then projects indexed routes and upstream cache-price availability
onto its model rows. The index belongs to that snapshot; configuration changes
cannot reuse stale route entries.

All sources use automatic protocol selection. Discovery preserves endpoint
declarations (`supported_endpoint_types` and provider equivalents, including
Gemini generation methods) with their origin and check time. Known service
defaults and a pasted full endpoint are explicit configuration hints. An
unknown `/models` response preserves inventory and routes every catalog model
through the source fallback protocol. Existing records keep their stored data,
but their routes are recomputed by the same automatic resolver.
Capabilities distinguish declared, confirmed, unsupported, and unknown states.
Source setup, catalog refresh, and automatic routing do not send generation
probes. A legacy explicit diagnostic endpoint remains for compatibility; its
result does not add or remove catalog models or control route eligibility.
Address/key changes invalidate evidence and bump the configuration revision;
late diagnostics and discovery cannot overwrite the new configuration.

Model semantics are resolved once from fixed, validated reference sources:
models.dev identity and detailed records, the public OpenRouter catalog, and
LiteLLM. The references fill missing fields, including models absent from the
primary registry; exact/qualified/unique-leaf matching keeps ambiguous IDs
separate. Accounts and API participants supply inventory and endpoint hints,
not authoritative model capabilities. Their optional reasoning, tool, image,
context or speed fields cannot replace reference metadata. No additional
participant metadata prefetch runs during management polling or catalog export.
Native Codex cards retain account-owned transport controls and instructions;
model labels and semantic capabilities use the same resolver as API sources.
Codex Ultra is a client-side orchestration mode, not an upstream reasoning
effort. The desktop projection reads the installed Codex's bundled model
catalog offline; an exact model card may expose Ultra only when the pool can
route Max and any specified subagent effort. A matching native ChatGPT account
card can also provide this evidence to the gateway catalog. Provider model
names, a Messages-to-Responses bridge, and a generic Max declaration do not
by themselves enable Ultra. Other reasoning levels remain reference and route
constrained; Codex's own toggle still controls whether Ultra is visible.

Missing reference fields use Relay's common text/image, text-output, function
tool and structured-output baseline. Explicit reference exclusions remain in
force. Unknown reasoning enums and numeric limits stay unknown; no model-version
allowlists supply them. Model Rules may narrow known levels. Adapters still
check whether a parameter can actually be represented in the target protocol.
Prices are explicitly separate: valid participant prices take precedence over
trusted catalog prices, followed by manual fallback. Actual usage and cache
semantics always come from the executed upstream protocol.

Desktop/server load the validated local metadata cache, then independently
refresh sources roughly hourly with HTTP validators. Failed sources become
stale without discarding healthy sources or blocking requests. Replacement is
schema-checked and atomic. Source payloads are shared as compact immutable JSON
across refresh and rollback states, parsed only for validation and merging;
requests use the resolved catalog. Cache reload borrows source envelopes without
building the untrusted stored merged tree. The cache schema and independent
source hash validation remain unchanged. Each merge indexes model versions and
provider-scoped leaves once; persistence and publication reuse the same merged payload.
Cache-file equality checks use a bounded buffer instead of reading a second
complete file into memory.
Backend ordering places OpenAI, Anthropic, Google,
then xAI first, followed by other companies alphabetically, and uses
contiguous catalog families ranked by their newest release. Families that share
the same numbered model generation form a cohort and use the newest release in
that cohort, then normalized family IDs; this keeps sibling variants together
instead of letting a later launch date outrank another variant. Release/update
dates order versions within a family. Missing families follow known families;
missing dates follow dated versions in the same family. Equal dates use
normalized family and model IDs as deterministic tie-breakers, so provider
inventory order cannot change catalog ranking. Company/family ordering comes
from validated catalog metadata except for the documented company presentation
order; no model-name or version lists are maintained. Selectors show catalog families within each
company while preserving backend order. Explicit manual order takes precedence.
The model-order editor keeps the supplied sequence and company blocks draggable;
selection and price editors first align member inventory to snapshot order, then
show metadata families. IDs absent from that snapshot follow in stable ID order.
An empty model
order update clears the persisted override in desktop and server, restoring
catalog ordering for existing and future models. Remote clients offer reset
only when the server advertises `model_order_reset`. Both hosts use the shared
`relay-core` model-edit policy for canonical ID lookup, partial-order completion,
and reasoning overrides. Storage, locking, rollback and transport error mapping
stay in the owning host. Member snapshots apply that same metadata/manual
ordering to their complete discovered inventory,
independently of membership and allow/deny rules. React appends configured IDs
missing from inventory without moving discovered models. `gateway.modelCatalog`
provides advisory company/family metadata for the complete member inventory and
saved rules/prices, so excluded models keep their group. It never grants routes;
older snapshots fall back to metadata on operational model rows. Member editors open on
model selection, with source prices and secondary settings on separate tabs.
Model Rules are operational; source prices remain in the source/member editor.

## API source statistics

`relay-core/sources/stats` owns balance transport, format detection, and amount
normalization for both desktop and server. Zenith and DeepSeek use their key
endpoints. SiliconFlow retired `/user/info` on 2026-08-14; balance is reported
unsupported without a credentialed probe until an official replacement exists.
OpenRouter uses `/key` for inference keys and queries `/credits`
only for a reported management key. Custom sources try Sub2API `/v1/usage`,
New API `/api/usage/token/`, then compatible One API billing subscription/usage.
Requests preserve the configured origin and reverse-proxy prefix, disable
redirects, remove query/fragment data, and bound response size and total time.
Rate limiting stops discovery. Public `/api/status` receives no credential.

Statistics distinguish wallet, key quota, and subscription allowance, preserve
native currency/unknown quota units, and parse decimal values with integer
arithmetic. New API conversion requires its published `quota_per_unit`;
Sub2API `actual_cost` is spend, while `cost` is only an equivalent. Legacy
billing usage is cents in the server's advertised display currency; absent
status metadata cannot safely be treated as USD. Legacy
microUSD fields contain USD only. Additional fields default when reading an
older Relay Server. Missing spend remains missing. React presents provider
spend separately from Relay's local estimate and marks retained values stale
after an unsuccessful refresh. Adapter names remain internal and do not add a
caption to the cards. These statistics never decide route eligibility.
Desktop and server share a bounded, resource-scoped refresh owner for source
models and balance. Page entry reads a runtime-only cached statistic when one
exists; an explicit refresh requests new provider data and joins an in-flight
read. A successful value has an observation time. A failed read retains the
last value for the same source revision with a stale warning and failure reason;
confirmed unsupported statistics have no periodic recheck. The cache is not
durable across application/server restarts. Source key, address, catalog and
eligibility edits retire late reads behind durable incarnation/configuration
revisions, including delete/re-add. Models and balance remain independent;
neither a stats denial nor a stale balance changes routing eligibility.
The redacted source summary exposes a non-secret `refreshRevision`; local and
remote pages use it to clear a previous key's balance even when address and
credential-availability flags are unchanged. Older servers may omit the field.
Source records and their revisions are captured together. The optional
`providerStats` projection carries the cached value in normal snapshots, so
background completion updates both Pool and Choose API without another
provider read. Automatic address normalization also checks the exact observed
endpoint before delivering cached data. Preparation failures reach explicit
callers but do not erase a same-scope last observation. Explicit force-refresh
intent applies to one UI operation, not future page/configuration changes.
Local and server summaries also expose independent `refreshState` for source
models/balance and account models/quota. `fresh` and `stale` reflect the current
refresh owner; saved model/quota observations start `stale` after restart rather
than pretending that the new process has checked them. Missing saved observations
remain `unknown`, and only confirmed adapter results are `unsupported`. These labels
are advisory, never substitutes for routing eligibility or a provider error.
Older server snapshots without this field remain readable.

## Quota and execution

Accounts and API sources use the same pool rotation admission engine and a
versioned `poolRouting` member order. New profiles use policy version 2 and
**Automatic**. On the normal 1.1.3 upgrade, desktop and server automatically
convert version-1 policies (including profiles without a saved policy) before
building a runtime. No confirmation or migration notification is required.
The host-refresh and remaining acceptance gates in
[ROADMAP.md](ROADMAP.md) remain open. The design is a target, not proof of
full acceptance.

Automatic selection compares normalized local load (`in_flight / effective
capacity`) among eligible physical members, then uses weights among equal-load
members. It does not score quota percentages, balance, recent latency or money.
Unknown and stale quota are neutral; confirmed exhaustion remains a block.
Soft affinity only breaks a tie among equally eligible automatic winners.
In order selects the first ready member in the saved order; busy or blocked
members do not prevent trying the next one. Round robin uses smooth weighted
rotation and ignores soft affinity. Hard response ownership applies in every
mode. Protocol aliases share physical capacity and do not gain extra weight.

Weights range from 1 to 100. A member's limit covers all its protocol routes and
text/image lanes; zero means no additional member cap, not infinite runtime
capacity. OAuth images retain their separate one-request limit. Reservation
and release belong to their own lease, including recovery permits. Preview does
not advance weighted credits. Capacity and recovery waits share a bounded
runtime queue: 1,024 waiters / 256 MiB, at most 256 waiters / 128 MiB per request
key. Retained parsed envelopes, repair copies and queue metadata are charged;
requests with immediately free capacity do not consume queue slots. Capacity
admission selects the oldest compatible waiter with round-robin turns between
principals, so an incompatible head cannot block another model. Cancellation
removes its registration synchronously. Events, known due times and deadlines
wake waiters; there is no periodic availability polling. The 30-second total
queue budget survives retry passes and WS/HTTP handoff; explicit persistent
waiting removes the time bound, not count/byte limits or the dispatch budget.
Local saturation/expiry returns a Relay-origin 503 (`admission_queue_full` or
`admission_wait_expired`), without a provider-health vote or generation debit.

One logical request owns the dispatch and transport budgets, retry window and
irreversible execution latch through HTTP, SSE, WebSocket, images, auth replay
and compatibility repair. The default is three generation dispatches and the
validated setting allows 1..8. A failed connection attempt still consumes its
transport/dispatch budget. A rejected final admission fence consumes no
generation. Before dispatch the runtime rechecks live principal scope and the
reserved configuration, auth, quota, rate and circuit observations.

Replay needs an explicit pre-execution rejection or a proven not-sent result,
repeatable input, preserved semantics and permitted ownership. Generic 5xx,
broken streams, disconnects after send and uncertain acceptance do not authorize
a second generation, even with complete history. Once output is committed or
execution is unknown, repair, a new driver and persistent waiting cannot reset
the latch. A quiet active generation does not expire the retry window: the
window starts at the first replay-safe rejection. The default retry window is
30 seconds; optional persistent text-route waiting for all four API inputs can remove that deadline, not the send
budget. Recovery waits hold no lease.

Dispatched physical members are tracked separately from incompatible routes
in the shared request context, including WS-to-HTTP handoff. A rejected alias
cannot masquerade as an untried independent source. A proven compatibility
repair may retry its owner without refunding any sends. Ordinary same-member
recovery starts a new selection pass only after waiting for the scheduler.

Provider header/body retry hints use the longer deadline. Mandatory rate and
access observations are installed with settlement under one scheduler lock,
before release notifications or another admission. Shared resource failures
cover the source's aliases; route-specific access failures leave other models
and unrelated upstream protocols usable. Configured source delays apply to
these mandatory/provider pauses and cannot shorten an explicit provider hint.
Transient inference health is separate: the first two independent request
failures pace the route for 250 and 500 ms; three within the incident window
open its circuit. Open backoff starts at 2 seconds and is capped at 60 seconds.
A logical request contributes at most one failure vote per circuit incident.
Recovery uses one half-open lease and a pool-wide exploration budget rather
than a last-member exemption. Late outcomes release their own resources but
cannot update a removed/re-added identity or close a newer circuit incident.

Desktop and server install the same engine from the complete configured pool,
including unavailable members; direct core callers also use it. Legacy roles
still map to the initial saved order (primary APIs, accounts, ordinary APIs,
reserve APIs). The forward-only startup converter maps Smart to
Automatic; In order/Round robin retain their mode, order, weights and limits.
The conversion is idempotent, persists before listener construction and does
not change gateway enabled state, credentials, membership, source delays,
persistent waiting or other user controls. No migration preview/apply API,
confirmation banner or compatibility rollback UI remains. Old scalar threshold,
keep-last and scoring fields are accepted in older JSON/presets and ignored by
pool rotation; startup removes them from desktop state and the server migration
deletes their metadata keys. New snapshots and presets omit them. Older clients'
scalar fields on routing requests are accepted but ignored. Source roles seed
the visible order; they do not add a hidden gate. Only `poolRouting` and
`maxRetryCandidates` alter rotation on cold construction and hot updates.
There is no hot switch between two engines and no database downgrade promise.
Desktop and server advertise `rotation_v2`. Both UI and Rust remote transport
refuse unsupported rotation-policy writes; management protocol version remains 2. Older
presets undergo the same conversion after ID remapping; omission of a policy
preserves the current destination policy.
Policy saves compare the previous policy and membership before applying
atomically. Hot policy changes preserve leases, cooldowns, and response ownership.
The rotation editor applies edits immediately through a serialized queue. Each
batch reads current settings and reapplies only identity-based user edits, with
at most three CAS attempts. Membership updates cannot resurrect removed members;
failed writes restore the stored values. Remote routing conflicts retain their
typed code, including the HTTP 400 response of older servers.
Server membership updates reconcile and install the same policy as a restart
before refreshing internal request-key scopes.
Portable presets remap tagged member IDs before validation and application.

Monitoring can refresh enabled accounts outside the pool, while draining, or
with unknown quota. Provider windows/reset times are displayed as reported.
Desktop and server account quota, account/source models and source balance use independent resource-kind
jobs in the shared refresh service. Startup, manual requests and wake verification join
that owner; cancellation of one caller does not cancel shared work. Jobs use
monotonic due times, bounded runtime/origin concurrency and start spacing,
weighted class fairness and provider Retry-After floors. Quota reads use the
active/idle 5/15-minute cadence; account models use 8/24 hours. A UI read alone
does not mark an account active. Disabled accounts have no periodic read.
Dispatch activity marks the physical `account:` or `source:` member, including
protocol aliases; both hosts retain recently used members in the active cadence
for ten minutes. This activity read does not build a routing-preview snapshot.
After a new passive inference quota snapshot is persisted, both hosts can defer
the automatic quota poll only while every reported window is still fresh and
subscription metadata is not due. This does not defer manual requests, confirmed
quota errors, independent model/balance work or a provider-reported future
reset check. The reset check uses a stable short jitter and still respects
provider Retry-After. A header without a fresh quota window does not count as a
successful poll.
Account and source observations update the latest stored record in a transaction,
with durable login/configuration/incarnation fences; they do not save a pre-HTTP
configuration snapshot or resurrect a deleted member. Account quota/model
readers and initial reset-credit checks on both hosts request an on-demand Auth
prerequisite through the same
service. Auth has reserved worker capacity, joins concurrent readers, and
continues to use the existing token authority for refresh/persistence. Its
prepared credential is shared only with current waiters, never kept in the
observation cache; stale login/configuration revisions are rejected before
the initial provider read. Explicit token refresh, post-401 recovery and Agent
task retry remain owner-local rather than separate scheduled refresh jobs.
Server quota/model 401 recovery compares the exact rejected bearer tokens
with the token authority. A late rejection cannot invalidate a newer OAuth
generation or a replacement login with a reused generation; an Agent assertion
has no OAuth bearer and does not trigger that recovery path. Server token and
Agent-task persistence binds to the credential reference captured before HTTP;
import switches that reference and retires the previous authority slot before
new work. Server runtime builds serialize their publication with account
import/delete: a build started before the switch finishes before the commit,
and the replacement runtime is built before metadata HTTP. Delayed preparation
and writes cannot target the replacement login. Provider-facing management
HTTP on both hosts shares a process-local concurrency gate with per-origin
limits, reserved Auth permits and waiter slots, bounded waits, and a permit
held through response-body consumption. Each retry obtains a new permit;
account/source reads recheck their durable revision after admission and before
the physical send. This does not replace the installed-client freshness/reason,
large-pool traffic or live-provider gates in ROADMAP.
Quota updates and account model reads synchronize the existing server and
desktop runtime instead of replacing its scheduler. Changed OAuth model
inventory updates the candidate, executor and model registry under the same
scheduler lock, preserving live physical leases, cooldowns and health. Removed
model routes reject an unstarted lease at final dispatch; a changed virtual
image base also revokes an unstarted image lease without spending a generation.
Final dispatch also rechecks Auth execution fences, capability blocks, protected
quota reserve, the principal scope revision and the exact revision of a bound
response owner. An invalidated or rebound opaque owner cannot send through its
old pending lease, even if the same candidate id is rebound. An exclude/re-add
of a principal's source also revokes an older lease; an unchanged scope save
does not. Candidate permission edits retain a separate revision, so a
disable/re-enable or model removal/re-add cannot revive a pending lease;
weight-only changes do not revoke it. Rejected leases do not spend a wire
attempt.
Prepared OAuth authorization is bound to its in-memory token-slot revision;
refresh, invalidation, replacement and removal retire the old preparation even
when the persisted generation or bearer repeats. Agent-task replacement has
its own revision. HTTP and WebSocket payload dispatch validate that revision
inside the request-budget/scheduler transaction, so a rejected preparation
does not spend a generation. This is the dispatch start boundary, not a lock
held through upstream network I/O.
Desktop and server apply a changed pool policy together with their internal
gateway key scopes under the same scope/scheduler lock order. A missing key
leaves both unchanged rather than exposing a partially updated routing graph.
Server single-account policy edits hold the configuration/build locks;
permission-changing edits additionally hold a candidate dispatch fence from
before the durable save through hot apply or replacement. An old pending lease
cannot send during that gap. Priority/weight-only edits do not fence it. A failed
rollback/rebuild retires the previous runtime rather than serving stale
permissions; already started attempts may still settle. Fences are scoped to
the candidate incarnation, so releasing an old guard cannot unfreeze a
removed and re-added candidate.
Server single-source updates and deletion also hold the build lock and fence
every physical protocol route before changing the credential, endpoint or
saved permission. Priority/weight-only edits do not fence pending work. A
failed source credential restoration or an uncertain vault
delete retires the old runtime. A policy-only save may keep the scheduler;
transport replacement retires its previous runtime after publication.
Server batch pool-membership edits validate all members first, then hold the
configuration/build locks and fence only changed account and physical source
candidates across the atomic store commit and policy/key-scope update. A failed
apply restores membership and rebuilds under that same lock; failed restore
retires the previous runtime. Unchanged membership does not fence requests.
Server account re-import and deletion fence the old candidate before replacing
its credential reference or deleting its store row. A failed vault deletion
restores the record and builds a fresh runtime; an unrecoverable restore retires
the old runtime rather than reopening its pending dispatches.
Server common/required proxy policy, per-account proxy and bulk assignment
edits also hold the configuration/build locks. Before a durable transport
change, affected account candidates are fenced through the replacement build
or rollback; an unchanged assignment does not add a dispatch fence. Started
attempts retain their existing transport until settlement.
Desktop batch membership, single-account policy and source policy/endpoint/key
edits also fence affected physical candidates before saving until the live
scope/policy update or replacement/rollback finishes. Desktop common and
required proxy settings, individual/bulk assignments and successful source
model discovery fence the old routes through transport replacement. Applying
a configuration preset fences its previous pool across both durable writes;
failure of the second write restores the first. If a desktop replacement build
fails after a partial hot apply, the restored records produce a fresh runtime
before dispatch resumes; an unrecoverable restore retires the old runtime.
Rotating the desktop request key fences the previous physical pool before
changing the saved principal secret and replacing its listener.
Desktop re-import and OAuth sign-in fence the previous account candidate before
committing a replacement login and keep the fence through runtime replacement.
If OAuth cannot restore a failed account write with certainty, it retires the
running gateway instead of allowing old pending dispatches.
Desktop single and batch account deletion fence the old physical candidates
before touching credentials. If restoring a failed deletion cannot recover
credentials, profiles, wake state or proxy assignments, the gateway stops and
is disabled before those fences are released.
Desktop ownership moves fence local candidates through remote import and its
verified cleanup. A pending move or remote-linked account is excluded from the
local runtime and key scope even after a restart. Once remote ownership is
committed, a failed local runtime replacement leaves the local route disabled
for recovery instead of reactivating two owners. Returning an account restores
its previous inactive ownership on failed activation. Remote reconciliation
also applies a live policy/scope change or replaces the runtime; it never rolls
back to an erroneously enabled remote-owned record.
Refresh reads cannot lift a newer live health block unless the durable read
records a health transition. A superseded server runtime, or a desktop runtime
being restarted, rejects new admission and pre-dispatch attempts while allowing
started leases to settle; waiting requests wake without a synthetic retry.
Other desktop token/login/ownership transitions and delayed-result cases remain
in the final-dispatch matrix tracked in ROADMAP.
Desktop quota/model readers also apply observations against the latest record
behind durable account/configuration revisions. Login/import, secret-backed
proxy changes, enable/ownership changes and delete/readd retire older reads;
restoring an earlier configuration does not restore its revision. Normal token
rotation, usage, display settings and pool weighting do not invalidate quota.
Preparation errors obey the same fence as successful observations, and a newer
passive quota snapshot wins even if the wall clock moves backwards. Superseded
quota reads do not emit wake/reset transitions. The desktop owner reconciles
registrations on durable configuration/eligibility events, not a polling scan;
ordinary usage does not rescan inventory. UI reads do not activate accounts.
Quota reads no longer discover models inline; plan changes mark the independent
model job dirty. Reported future resets can accelerate quota work without
bypassing provider floors. Only the quota job finalizes automation transitions;
reset verification reads within that job instead of recursively waiting on it.
Deletion retires both resource kinds before secret changes. Rollback derives
registrations from current storage, never from a cloned queue or old revision.
Opening desktop storage alone does not start provider work; the native host
starts the service, which keeps no strong host reference while idle. Start and
completion events publish the committed in-flight state to existing UI listeners.
Backoff retains the actual safe failure reason; successful refresh can restore
health. Credential, proxy, auth, and capacity failures are not all quota errors.
Model discovery can recover its own errors, but cannot clear an independent
account block, authentication failure, or verification requirement. Transient
catalog errors preserve a previously terminal discovery failure until a
successful catalog refresh.

Pool and Connections display operational groups in the same order: rotation,
quota wait, unavailable, disabled. Scheduler order remains intact within each
group; Connections can additionally group by subscription. The rotation editor
also groups by operational status in Automatic and Round robin, while In order
preserves the editable manual queue. Only In order exposes
reordering, and only Automatic and Round robin expose weights. The dialog uses one
scrolling body, concise status labels and detailed explanations in Help.
The scheduler supplies the next-route preview using the request key's scope,
model rules, protocols and current member capacity. It names a physical member
only if all fresh text routes agree; continuations keep their own affinity.
Activity events invalidate older previews until a fresh snapshot arrives.
Runtime IDs and activity revisions reject stale reserve/release events across
runtime replacement. A missing preview is not evidence that all members are
unavailable. Pool warnings use member operational state
and the enabled model catalog; active requests take precedence over stale
availability snapshots. These presentation rules do not change dispatch.

Execution checks membership, enablement/draining, credentials/proxy, model,
protocol/adapter support, health/cooldown, quota, and capacity. Response IDs and
active connections preserve upstream ownership. Soft prompt/session affinity
cannot force an unhealthy member. Retry and credential refresh share the request budget;
no transparent fallback occurs after response bytes reach the client or when
remote execution is uncertain. Native
Responses continuations keep a bounded local materialized replay chain. After a proven pre-execution rejection and before
any response bytes are visible, Relay can use that chain to move a
continuation from a temporarily unavailable owner to another compatible
candidate; the replacement receives the full input without the old opaque
response reference. When no local replay exists, opaque ownership remains
mandatory and the request waits or fails safely.
Replaying a turn releases only that request's routing constraint; the saved
response owner remains available for other branches and retries. Only history
is inherited; options come from the current request, including provider
extensions. A server conversation or unresolved input item reference cannot
become a portable replay.
Successful turns containing an unpaired tool output remain owner-bound;
upstream acceptance alone cannot reconstruct the omitted call or its history.

HTTP, WebSocket, and account-only endpoints share continuation admission.
An unknown response reference returns `409 response_continuation_unavailable`
before contacting a candidate, even if incoming messages look complete.
An explicit compacted window is a separate stateless contract: a nonempty
compaction checkpoint with materialized retained items and paired tool calls
replaces the predecessor reference before routing. Relay forwards that window
unchanged, without prepending stale replay history or pruning its items.
Compaction settings alone, item references, and incomplete tool state do not
establish such a checkpoint. Invalid encrypted compaction fails explicitly;
recovery must never discard it to retry without the earlier context. See the
[Responses compaction contract](https://developers.openai.com/api/docs/guides/compaction).
Plaintext recovery requires a saved predecessor scoped to the same local key
and owner; it materializes that history before removing the reference. An
unpaired tool output needs a known owner, while paired tool history without an
opaque reference may rotate. After an explicit upstream missing-tool-output
error, native Responses recovery may remove one unmatched function or
custom-tool call from same-key saved history only when its kind and any reported
ID identify it uniquely. It preserves every tool output and leaves ambiguous
history, missing replay state, encrypted content, compaction/context-management
state, and unsupported replay items untouched.
After an explicit upstream tool-link rejection, native HTTP/SSE and WebSocket
may repair one unambiguous call/result mapping before visible output. Call IDs
remain distinct from item IDs; matching respects tool kind and namespace.
Repairs apply atomically and never discard results or guess between parallel
calls. Missing history stays an error.
Ownership checks inspect every tool identifier. Without a predecessor reference,
all unpaired outputs must share a known owner; completed historic calls cannot
choose that owner or authorize an unknown output.
Native stream replay retains complete terminal output or completed output
items, preserving tool state and assistant phase; deltas alone are insufficient.
Capture is bounded in memory and never written to diagnostics. An incomplete
response can continue only on its existing WebSocket, using a connection-local
binding that is not persisted or accepted by another connection.

Generation requests have no Relay total-duration, first-output, or inter-event
timeout, in either JSON or streaming mode. HTTP clients retain bounded connection
establishment; metadata and credential operations retain their own timeouts.
SSE emits keepalive comments between complete frames after response commitment;
pre-output buffering preserves safe retry and HTTP error status. WebSocket pings
keep both sides active while waiting for a provider, including HTTP fallback.
Idle WebSocket cleanup applies only when no request is in flight. Provider
completion/failure, transport failure or client cancellation ends a generation;
silence alone does not release its lease, penalize a source or trigger a retry.
After an SSE terminal event, Relay discards later frames even when they share
the same transport chunk. A completed WebSocket turn also drops late upstream
`response.*` frames while no request owns them; neither path emits a second
usage event for that turn.
For native Responses, a bare SSE `[DONE]` without `response.completed` is an
incomplete response, not a success or a reusable continuation. The HTTP/SSE
to WebSocket bridge must deliver a terminal response event or fail explicitly;
the marker alone cannot leave a WebSocket client waiting indefinitely.
HTTP/SSE success requires the terminal event of the client's wire protocol:
Responses completion, Chat Completions `[DONE]`, or Messages `message_stop`.
Gemini streams have no such marker and require a candidate `finishReason: STOP`
before a clean EOF; foreign protocol markers cannot establish success.

Failures preserve `relay`, `account`, or `provider` route origin plus safe
category, status, and timings across protocols, storage, UI, and exports.
Local and server usage additionally retain a bounded, redacted provider error
envelope (original code, type, message, and observed HTTP status), separately
from Relay's classification. Successful replacement attempts clear stale error
details; legacy records cannot reconstruct messages that were never recorded.
Raw payloads and credentials never enter diagnostic records.
Invalid SSE JSON records Relay parser diagnostics in the existing error envelope
with type `relay_stream_parser`: fixed error category, position, byte/line counts
and format flags only. Upstream event names and payload fragments are excluded.
Upstream origin identifies the selected account or API source independently
of whether the failure affects account health. Generic HTTP 400/422 or
`invalid_request_error` alone does not prove a client error: candidate rejection
can still fall back, while explicit request-validation failures stop retries.

## Protocol adapters

All four JSON generation entrances and account compact/search use one bounded
body decoder for identity, gzip and zstd. Wire and decoded bodies each have a
64 MiB limit; stacked encodings are rejected and zstd windows are bounded.
Multipart image edits retain their separate body contract.

Account compact first uses the legacy endpoint. Only an explicit missing
endpoint (405 or an unambiguous route-not-found 404) permits one Responses
request with `compaction_trigger`, on the same account and within the retry
budget. The bridge requires a complete successful SSE terminal response with
encrypted compaction output, preserves usage and retained output, and does not
replay a failed or interrupted generation. Model errors and quota failures do
not trigger this compatibility path.

Sparse terminal responses are reconstructed from completed output items using
the shared bounded replay collector. Unfinished output and events after the
terminal response cannot produce a successful compact result.

Codex turn-state hints are scoped to local key, session, account, upstream model,
exact state and credential identity. OAuth refresh changes that identity; an
AgentAssertion uses the stable signing credential/runtime/task, excluding its
per-request timestamp. HTTP and WebSocket apply ownership after authorization
is prepared. The bounded expiring store retains only fingerprints, so delayed
responses cannot transfer state to another owner or credential.

An open upstream WebSocket is reused only while its authorization fingerprint
and in-memory credential incarnation match the current preparation. A changed
credential or replaced token slot requires a new connection and complete
portable history; opaque continuation state fails explicitly.
Background Codex catalog updates recheck the original profile binding under
the profile lock before writing a fetched catalog.
Codex launch applies deferred catalog updates before starting the client; a
failed refresh retains the last verified catalog and its visible warning.
An immediately preceding successful attachment needs no second catalog fetch.
An OpenCode configuration error does not prevent the Codex catalog refresh.
Local pool attachment fetches its catalog before stopping Codex or changing
history. Renderer snapshot refresh follows both attachment and optional launch.
History synchronization inspects imported session metadata even when the profile
already uses the target provider. Its aggregate size budget applies only to
rollouts that need backup and rewriting; matching history does not consume it.
Per-file and file-count bounds, snapshot validation and rollback remain enforced.
Native account catalog reads run with at most four concurrent requests and a
shared twelve-second budget; completed results retain account ranking and
unreachable accounts keep their own last known transport metadata.

ChatGPT account HTTP and WebSocket handshakes retain only the infrastructure
cookie `__oailb`, in memory, per account executor and credential. The store is
bounded, respects cookie path and expiry, and only sends to HTTPS port 443 on
`chatgpt.com/backend-api`. Proxy/runtime replacement creates a new store;
late responses hold the old jar. Browser/authentication cookies are excluded,
and no cookie values enter storage, diagnostics, or client responses.

The optional Excel / Basis Points transport is configured in API as
**Model substitution protection** and identified in Usage. The name describes
the intended workaround, not verification of the model running at the provider.
The API control uses the existing shared routing setting and saves immediately.
There are no switches in Connections, Pool or account cards.
The control appears when a compatible account exists, even before it joins
the pool, or when the setting is already enabled so it can be turned off.
Remote servers without the setting field do not expose the control. It uses the
same physical OAuth account, quota and rotation slot.
Clients use any of the four Relay protocols. The account executor maps
function/custom calls and outputs through its native `run_officejs` envelope,
then the protocol adapter converts the result to the client's format. The
upstream returns completed JSON, so requested SSE is buffered and emitted only
after completion.
Images and explicit nonstandard service tiers are incompatible with this route;
opaque `previous_response_id` continuation is rejected before dispatch rather
than silently removed. Completed and incomplete buffered responses retain
their respective terminal status in JSON and synthesized SSE.
it does not publish native Codex Fast/Ultrafast metadata. Relay does not claim
incremental streaming or provider acceptance without a live request.

The adapter registry supports four native contracts and twelve conversions:
Responses (`/v1/responses`), Chat Completions (`/v1/chat/completions`), Messages
(`/v1/messages`), and Gemini (`/v1beta/models/{model}:generateContent` or
`:streamGenerateContent?alt=sse`). Native requests preserve provider extensions.
Catalog feature and reasoning declarations constrain converted routes, not
native request admission; native endpoints validate their own parameters.
Converted requests use typed messages, images, function calls/results, tool
selection, JSON schemas, and reasoning controls where a matching representation
exists. Meaningful unsupported parameters fail before sending. A JSON Schema
is preserved; strict-mode guarantees are not invented for Gemini. Responses
bridges additionally support their existing namespace/custom-text tool contract.
Codex tool-schema normalization expands safe local references once per schema,
within a 1 MiB expansion budget and depth 64. Constraint siblings, resource
boundaries, dynamic references and literal data remain intact. Exceeding the
budget or encountering an unsafe scope preserves the original schema atomically.
Responses bridges accept Codex client tracing and cache-affinity keys without
forwarding them as provider parameters. The optional encrypted-reasoning output
selector does not require fabricated encrypted output; native bridge state stays
local. Supplied encrypted input and compaction still require a compatible native
route. Unsupported known controls report a safe field name in `error.param`
and the error message, never a request value. Neutral text/null controls do not
claim structured-output or reasoning capabilities during admission.

Admission validates the request and continuation, filters compatible routes,
then rotates physical members and prefers a suitable native route within the
selected member before reserving capacity. Routes have stable identities; all
routes of one member share weight and concurrency. Endpoint/model failures
remain scoped, resource failures apply to the physical member, all attempts use
one retry budget, and visible output prevents fallback. Continuation ownership
is retained unless a complete portable history permits replay. Opaque state,
encrypted reasoning and unpaired tool results are never discarded for retry.

Streams retain tool IDs and order, terminal status, and actual upstream usage.
Adjacent Responses function calls remain one assistant turn when bridged to
Chat Completions, followed by their tool results. A Messages `refusal` is a
terminal filtered response in converted JSON and streams, not a malformed
upstream response. The dedicated Responses-to-Messages bridge carries a
reported refusal and output-token limit as an incomplete Responses response,
including when the provider returns no content. It rejects absent or unknown
terminal reasons instead of reporting success. Chat Completions `refusal`
fields and refusal content parts retain their text and filtered terminal state
when translated.
Gemini prompt blocks with an explicit `promptFeedback.blockReason` and no
candidates, and filtered or token-limited candidates without content, retain
their incomplete terminal status in JSON and SSE conversions. Missing candidates
without a known block reason and unknown finish reasons do not become successes.
Native Gemini SSE likewise needs a recognized final reason before EOF can be
recorded as success; an interrupted stream remains a failed attempt. Native
media and code parts count as output for streaming admission without rewriting
their provider-owned bytes or claiming cross-protocol conversion support.
Portable reasoning text is preserved in Chat Completions JSON, streamed deltas
and tool-turn history using `reasoning_content`. Unsigned Messages thinking,
Gemini thoughts and public Responses summaries can use this representation.
Signed, encrypted or otherwise opaque state remains bound to its native route;
foreign reasoning history cannot fabricate native Responses item identifiers.
SSE framing accepts LF, CRLF and CR, including fragmented and mixed line
endings. Multiple data lines remain one event payload; malformed JSON is not
split heuristically. Native stream bytes remain unchanged.
Absent counters remain unknown. Native Messages/Gemini catalogs and model
details enforce the same key scope as generation. The shared Rust projection
exposes routes, feature status, and executable reasoning levels to desktop,
server, and client configuration. Frontend renders this projection.

Cross-protocol WebSocket, Realtime, audio/video conversion, and server-tool
emulation are excluded. Native Responses WebSocket remains available; Codex
uses HTTP/SSE when its catalog includes converted Responses routes.
The current WebSocket bridge processes one response at a time and binds at most
one named `stream_id` per connection. It does not yet implement the official
multi-lane concurrency, per-lane queuing, or cross-lane forks. Named HTTP/SSE
fallback scopes JSON response events to the request's stream; non-JSON opaque
compaction cannot be scoped and fails explicitly rather than producing a
misattributed lane event. Do not claim full WebSocket multiplexing support.

## Tool catalog policy

The shared Rust runtime owns two opt-in modes: `pass_through` (default) sends
the complete catalog unchanged, and `automatic` enables provider-native
deferred schema loading on every eligible request. Relay does not select, hide, or
rename tools by name and does not perform local semantic relevance search.
The complete trusted catalog remains available to the provider; the provider
performs its own tool search and loading. Catalog size does not affect whether
automatic mode is applied.

Automatic mode uses the provider-native Responses `tool_search` contract only
on native Responses routes with automatic or unspecified `tool_choice`.
Converted protocols, explicit tool choices, and WebSocket payloads keep the
ordinary full catalog path. If a compatible native endpoint rejects the
deferred fields, Relay retries once before output without optimization. This
fallback is compatibility behavior, not a second policy mode.

Each request captures an immutable policy through retries and HTTP/WebSocket
fallback. Desktop and server persist compare-and-set updates and hot-apply
them to new requests without restarting the listener. A single UI switch saves
the selected mode immediately; remote editing requires `tool_policy_v1`.
Usage stores aggregate input/forwarded counts and catalog JSON bytes, mode,
outcome, compatibility fallback and whether provider-hosted deferred tool
search was used, not schemas or tool names. Bytes are not token/billing
savings; provider-reported usage remains the source of truth, and changes to
catalog serialization can affect prompt-cache reuse. This is not an execution
authorization boundary.

## Usage and prices

Usage stores observed token/cache splits, service tier, selected member, status,
and timings. API-equivalent is an informational estimate, not subscription
entitlement, provider debit, Zenith customer billing, or a routing input.

Subscription usage uses the exact LiteLLM record in its declared official
family or remains unpriced. API sources resolve provider evidence, exact
LiteLLM provider/model, explicitly declared-family canonical price, then manual
price. Endpoint/protocol changes invalidate stale source evidence. Input,
cached input, cache writes, output, and request/image prices remain distinct;
missing required counters/prices are unknown, not `$0`. Adapters follow the
actual upstream cache contract without borrowing another protocol's semantics.
Explicit 5m/1h prices in a source catalog remain visible in source pricing
regardless of the catalog endpoint; an untagged cache-write price does not
establish a TTL. Displaying a price does not establish route capability.
Usage history shows cache-read and cache-write counters separately. An exact
provider-reported cache-write window is shown as reported; when usage omits the
window, the UI marks it as unreported. Model documentation is separate from
usage evidence: GPT-5.6 and later have an OpenAI-documented minimum `30m` after
the latest write or reuse, shown only as a note. Relay cannot calculate a live
remaining time or exact expiry without cache identity and reuse events.

LiteLLM is the external reference price catalog. Cached startup is nonblocking;
conditional refresh runs at startup and then the cache TTL (currently 24 hours)
with bounded retries. Invalid payloads cannot replace the last good snapshot.
Calculations use immutable catalog/policy revisions; refresh invalidates derived
totals, not stored token facts. Prices do not affect admission or scheduling.

Fast denotes observed upstream priority service, separate from source priority;
it is not a second quota meter or a universal price multiplier. Provider quota
is an operational signal; monetary quota is not inferred from its percentage.
Request-speed options are Relay model-family policy, not participant discovery.
OpenAI conversational models offer Standard, Fast (`priority`) and Ultrafast
(`ultrafast`) by default; other families keep Standard until a project policy
exists. The common family resolver handles new versions without a version list.
No source tier arrays, speed probes, or extra public Codex speed catalog are used.
Management snapshots and native/managed Codex catalogs share this policy even
when the listener is stopped or a member is cooling down. Key scope and model
inventory still govern which models are visible. Explicit client choices take
precedence over pool defaults and survive HTTP/SSE and native WebSocket retries
unchanged. The upstream response supplies the observed tier independently;
publishing a request option never fabricates priority usage or quota entitlement.
Optional purchase cost/payback and remaining API-equivalent estimates are
display-only and require complete priced Relay usage for the relevant window.

## Profiles and user-managed server

Profile changes follow inspect, protected snapshot, attach/apply, verify, and
restore. Ordinary detach undoes only unchanged Relay-owned config leaves and
the Relay-owned login; newer manual sign-ins, edited config leaves, and unrelated
settings survive. An explicit activation may safely detach an old binding,
keep a newer sign-in as the next protected baseline, and then attach Relay.
Automatic rollback never adopts a newer sign-in and stops before replacing it.
Cross-provider history repair is
reversible and rolls back if profile application fails. Named ChatGPT recovery
points are separate explicit restores of configuration/authentication.
An external edit to the known managed model-catalog file does not invalidate
the profile backup: automatic detach restores the previous config/auth but
leaves that edited file untouched. Refresh still refuses to replace it, and
newer sign-ins remain protected. Invalid backup metadata is reported by the
failed invariant and is not auto-repaired.
History scans fingerprint bytes during the metadata pass and skip constructing
JSON trees for conversation/tool events. All session metadata records are still
checked, including later records in imported histories; rollback checks retain
the fingerprint of the complete file.

Codex attachment writes a validated bounded catalog from the selected live
endpoint/key with reversible model IDs. Native rows keep account transport controls; model capabilities use the shared resolver.
Account models and unqualified GPT IDs retain their public spelling even when
an account lacks a native catalog card. Explicit key prefixes stay intact and
old Relay aliases remain accepted. Only the owning account's matching card supplies native transport fields.
Every row uses shared reference model metadata; fallback rows retain the Relay
ownership marker.
Native transport card selection skips incompatible cards when another owning
account has a valid one. Labels always use the resolved reference catalog name
(exact ID or an unambiguous leaf), then a compact label from the ID. The same metadata label resolver serves pool catalogs, management
snapshots and direct API profiles. Labels never merge routes or grant native
account capabilities; numbered GPT fallback labels omit the GPT prefix.
Refresh writes before invalidating the client cache; failure retains the prior
verified profile and exposes a warning. OpenCode preserves the original JSON/
JSONC configuration and restores with a semantic merge of compatible user edits.
OpenCode groups executable models under Responses, Chat, Anthropic and Google
SDK providers, preferring native routes for new assignments while preserving
working provider/model identifiers and the selected model. Automatic refresh
requires unchanged managed endpoint/key/SDK ownership and preserves user
options; it does not restart the client. Direct sources use only native routes.
Relay-owned ChatGPT account calls start with a validated stable Codex fallback,
then asynchronously read the official OpenAI Codex GitHub release feed after
the host loads and once per hour. Only published `rust-v<semver>` releases
without a prerelease component are accepted; the selected version remains
process-local and is never written to a JSON cache. Direct client identity
headers remain client-owned.

Remote management negotiates capabilities and uses revision-checked operations.
`source_protocols_v1` gates endpoint evidence, automatic computed routes, and
the legacy explicit diagnostic operation.
`route_recovery_v1` announces that the existing saved recovery switch applies
to all four text API inputs. Older servers may expose only the legacy
`chatgpt_retry_until_available` feature, which covers ChatGPT clients; the
cross-protocol control is hidden for those servers.
Preset schema 6 requires support for the current rotation policy and accepts versions
2 through 6. Schema 5 added optional tool policy. Presets without a pool policy
preserve the destination policy version; explicit cross-version policies require
migrating the destination first. Member IDs are remapped before application.
An omitted policy preserves the destination's settings; an explicit default
policy resets them. Legacy protocol mode fields are ignored. Adapter or tool
policy contracts unsupported by an older server are rejected before upload.
Secret transfer is separate from profile/config publication. Backup/restore
uses the server CLI with a locked data directory. Real account, proxy, client
tool-use, streaming, restart, and restore acceptance remains necessary before
production-ready claims; mocks or a successful build do not establish it.
