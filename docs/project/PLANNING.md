# Zenith Relay architecture

Current implementation contracts. Code and focused tests define exact behavior.
Use [ROADMAP.md](ROADMAP.md) for open work, [CONTRIBUTING.md](../../CONTRIBUTING.md)
for development checks, and [Help](../help/en/README.md) for user steps.

## Scope and ownership

Relay is a local-first desktop application. The retired personal-server runtime
is removed. It is separate from production Zenith Gateway and Control. Production
credentials, customer data, inventory, billing, and routing logic do not belong
here. [Hosted Relay](HOSTED_RELAY_ARCHITECTURE.md) is a proposal, not a shipped service.

| Area | Owns |
| --- | --- |
| `src/src` | React UI, i18n, snapshots, typed Tauri calls |
| `src-tauri/src` | Desktop I/O, credentials, OAuth, profiles, process lifecycle |
| `crates/relay-core` | Shared discovery, routing, protocols, quota, scheduling, usage |
| `web` | Independent Hosted UI preview; no live Hosted backend |
| `catalog` | Versioned model reference data and export contract |
| `scripts` | Build, setup, verification, release and maintenance commands; shared helpers in `lib` |
| `docs/project`, `docs/help`, `docs/releases` | Architecture and plans, localized user steps, release history |

The renderer does not read secrets, call providers, or decide routing. Desktop
uses `relay-core` in-process. Closing the desktop window leaves the local process
running; explicitly quitting stops it. LAN access shares that same local runtime.

### File organization

Rust uses `module.rs` with child files in `module/`; for example,
`src-tauri/src/codex_config.rs` owns `codex_config/auth.rs`,
`codex_config/provider.rs`, and `codex_config/tests.rs`. Keep module declarations,
imports, and tests aligned when moving a file. Compatibility readers and recovery
backups remain with their owners; an old format is not proof of dead code.

Desktop UI lives in `src/src/features/relay`: `pages/` owns page-specific views and
models, `state/` owns shared state, `api/` owns typed IPC, and `shell/` owns
navigation. Cross-page domain helpers stay at the feature root. `src/tests/`
contains unit and browser tests; `web/src/` and `web/tests/` belong only to the
independent preview. `scripts/{build,setup,check,release,maintenance}/` groups
commands by purpose; helpers used only by one group stay in that group.

## Core layout

Paths in this table are under [relay-core/src](../../crates/relay-core/src).

| Module | Responsibility |
| --- | --- |
| `gateway/` | HTTP/WebSocket ingress, body limits, request execution, streaming, error responses |
| `runtime/` | Configured sources and keys, authorization, candidate leases, model projection, session state |
| `scheduler/` | Capacity, selection, affinity, cooldowns, refresh and rotation policy |
| `protocol/` | Wire contracts, native forwarding, typed codecs, terminal validation, replay |
| `accounts/`, `providers/` | Token authority, account-route group policies and provider-specific account transport, including ChatGPT/BPS |
| `sources/` | API-source endpoint configuration, discovery, service profiles, statistics |
| `model_metadata/`, `catalog/` | Reference identity, capabilities, native-protocol and display policies |
| `quota/` | Provider quota observations and their freshness |
| `usage/`, `pricing/` | Request/attempt telemetry and API-equivalent estimates |
| `automations/`, `request_timezone.rs`, `tool_policy/` | Shared explicit background and request policies |

Desktop IPC DTOs, including `DesktopCapabilities`, live in `protocol/management/`.
They are separate from model and provider capabilities in `model_metadata/` and `sources/`.

Usage query normalization lives beside its DTO and takes an explicit observation
time. Desktop supplies protected route-identity facts to core policies and owns
credential access, dispatch fences, atomic persistence and rollback; automation
route remapping lives in `automations/`.

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

The desktop entrypoint is `src-tauri/src/desktop.rs`. Its IPC surface serves the
current UI; the retired Ready API activation, separate catalog and top-up flows
are not exposed. Explicit source launches retain read-only access to saved legacy
keys. Profile backups and history recovery remain owned by the pool integration.

Data pages use the workspace beside the sidebar. Card tracks remain compact;
settings, dialogs, and reading content have bounded readable widths. User-facing
labels and workflows belong in localized Help, not this architecture document.
Action-menu item panels mount only while expanded; their triggers and dismissal
handlers remain attached so rapid outside clicks and Escape still close them.
Sidebar auto-collapse follows viewport changes until the user explicitly chooses
a sidebar state for the current session.

Live activity keeps each candidate's newest event within its runtime generation.
Events from different candidates may arrive out of order; a late release still
updates that candidate without moving the latest global activity marker backwards.

The mode picker offers Computer and Relay Hosted. Hosted is a localized
development placeholder: no runtime snapshots, server connections, or inference
are started there. Switching modes does not stop the local API.
Development builds additionally expose an explicit Hosted demo action. It checks
the fixed loopback web preview and opens a separate incognito `hosted-preview`
window with no Desktop capabilities, no external navigation and no account/runtime
access. Release builds hide the action and reject the native command. This is a
UI-only demo, not Hosted activation or a server connection.
The retired personal-server desktop mode migrates to Computer. Its transport,
IPC commands, background reconciliation, transfer state machine and UI are removed.
Saved server metadata and incomplete operations are archived without deleting
credentials. Affected local accounts remain disabled and outside the pool.
Explicit local recovery requires confirmation that the external owner is stopped;
it releases the saved ownership tag but does not enable the account or contact the
old server. No retired server runtime is shipped; Hosted is a separate future service.

Computer API binds loopback by default. Explicit LAN sharing binds IPv4 and
admits only private IPv4 peers with a literal private Host and no browser Origin.
A separate credential-store request key gates network inference; local profiles
keep their loopback URL and key. Both keys use the same runtime and scheduler.
Scope changes persist and restart a running listener with rollback on failure;
they never start a stopped API. No management API, firewall rule, port forwarding,
or public-internet exposure is provisioned. LAN HTTP requires a trusted network.
Automatic startup shares the settings transaction lock with stop and configuration
changes, then reads the committed enabled state, port and bind scope.
Stopping or pruning a finished listener withdraws its published runtime before
draining connections. Idle route receivers retain no runtime/storage state;
already-dispatched work keeps its own state while finishing or being cancelled.
The connection panel lists private IPv4 addresses by operational interface rather
than guessing from the default route (which may use a VPN). While visible, it
refreshes on focus, reconnection and every 15 seconds without changing the listener.

Startup paints a themed, localized HTML loader before importing the React UI.
It reports interface/data loading, not invented progress percentages. The shell
is revealed after a successful initial snapshot and paint; slow or failed startup
offers a window reload without clearing settings. Render failures reveal the
existing error boundary. Secondary pages load on navigation intent, not all at
startup. Existing first-frame and interactive metrics track renderer readiness,
not provider or gateway health.

## State and secret boundaries

- Credential access belongs to the Desktop credential store and protected vault.
  Database, recovery, export, cache, and log areas are separate.
- Snapshots, usage, and diagnostics are redacted. Account exports are explicit
  credential-bearing documents, not support bundles.
- Provider credentials and inference request keys are different credentials.
- Local database migrations are append-only. Recovery validates protected
  references before activation.
- Runtime replacement fences delayed dispatches. Already dispatched streams can
  finish; a save acknowledges durable storage and runtime publication, not a
  later client-catalog refresh. Late UI reads cannot undo a successful command.

## Connections, discovery, and metadata

Connections stores accounts, API sources, and proxies. Saving a connection does
not enable pool membership. Access owns listener/key settings and account
request policies; Pool owns membership and per-model rules.

Connections offer Codex (`codex`, the default), ChatGPT/SIWC (`siwc`), and
Excel/Basis Points (`excel_bps`) in the Tauri sign-in window. Each supports new sign-in,
resume and reauthentication with its original issuing client; no connection is
deleted, relabeled or silently converted to another login method.
SIWC uses a separate `siwc-` WebView profile and the existing effective proxy.
Its exact redirect is captured by the same native callback validator as manual
submission, without routing the loopback callback through the proxy. State,
PKCE, nonce, issuing-client checks and cancellation fences remain unchanged;
external pages receive no Tauri IPC permissions or saved-login automation.
This embedded SIWC flow is a deliberate Desktop deviation from the official
system-browser guidance linked below, not a provider compatibility guarantee.
The copy-link action retains manual system-browser recovery; live WebView
acceptance is tracked in ROADMAP.
Both new sign-in and reauthentication expose optional account notes. New notes
stay in memory under the login ID, survive folding the section, and use the
existing notes command only after account completion. Cancellation clears the
draft. Reauthentication edits the targeted account's existing notes. Normal
sign-in remains manual. On Windows, macOS and Linux, right-clicking an
Excel/BPS account's sign-in-required action immediately starts an automatic
attempt without a menu, using its saved
email/password and, on an identified authenticator challenge, a fresh TOTP.
The existing isolated WebView profile, effective proxy, OAuth callback validation
and target-identity checkpoint remain authoritative. Notes stay native until an
exact HTTPS auth.openai.com login form needs one value; no external-page IPC is
enabled. Each evaluation carries a deadline checked in the page before filling
and submitting, so a delayed evaluation cannot submit after its native timeout.
An empty hydrating form gets a bounded five-second grace per page/transition;
CAPTCHA and explicit errors still stop immediately. Each stage is submitted at
most once. CAPTCHA, errors, consent, social
login, unknown forms or a 45-second limit stop automation and reveal the same
window. Manual resume stops automation without replacing that window; cancel,
replacement and window destruction fence the task. No unattended completion is
guaranteed and no new provider authentication API is introduced.
Native manual-fallback notifications carry only login ID and redacted automation
state through the existing OAuth event listener. They cannot rewind callback or
terminal status; early events survive start completion. Manual continuation also
clears its progress hint locally after a successful resume.
Persisted loopback attempts retain the issuing client's exact redirect host:
`localhost` for Codex and `127.0.0.1` for SIWC, including after restart.
Early status events are matched by login ID, so another attempt cannot hide a
callback received before start returns. Targeted reauthentication uses the
account's effective proxy or direct override for token exchange; the common
proxy does not replace that route.
SIWC persists a protected registration (issued client ID, verified subject,
host ID, granted scopes, expiry and rotating tokens) using the existing vault.
The initial dynamic client ID is never used for exchange or refresh. Callbacks
use PKCE/state/nonce and 127.0.0.1; RS256/JWKS verification checks issuer,
audience, expiry and nonce. Reauthorization stays with the selected registration.
Missing plan permission preserves sign-in but prevents pool admission/inference;
an explicit later sign-in requests consent. SIWC rotation checkpoints fence an
uncertain refresh reply and recover a staged token set without refreshing twice.
Hard deletion attempts provider revocation before removing local credentials;
trash is reversible and does not revoke. Remote revocation cannot be undone if
a subsequent local delete fails: rollback restores local data but closes the
gateway; that connection needs reauthorization before restarting the gateway.
SIWC uses account-scoped public `/v1/models` and `/v1/responses` HTTP/SSE,
`store=false`, complete input history, and terminal `response.completed`.
Unsupported semantic options/tools are rejected, not stripped. SIWC compaction
uses Relay's existing text-summary bridge on the same `/v1/responses` account
route, for both compaction triggers and `/v1/responses/compact`. Summary requests
omit tool declarations and return a Relay-owned portable checkpoint; subsequent
turns unwrap only that checkpoint. Foreign encrypted compaction remains an
explicit incompatibility. Native Codex compact endpoints are not used by SIWC.
Bare function/custom declarations move to `additional_tools` with the required
`role: "developer"`, without renaming calls or results. Existing tool-loading
history stays in place: unchanged definitions are not duplicated, and new or
changed root definitions append after history, preserving current-root precedence.
Without tool-loading history, the initial catalog still precedes the input;
legacy calls missing a declaration retain a prefix definition before replay.
Relay does not invent a hidden cross-request catalog or remove client tools.
Clients that replace instructions, definitions or earlier history can still
invalidate the provider prefix; this conversion does not guarantee cache hits.
Client WebSockets use the existing HTTP bridge; native upstream WebSockets are
not enabled. A confirmed SIWC quota rejection before generation
uses the existing cooldown and bounded pool fallback, for HTTP 429 and an early
SSE failure alike. Selection still respects configured membership, model/key
scope and continuation ownership; Relay never injects another billing route.
Other SIWC failures remain terminal. Any generation evidence or uncertain
execution forbids replay, even when the terminal error reports exhausted quota.
The structured `subscription_sharing_user_not_eligible` refusal has its own
diagnostic category. It ends the request without credential repair, account
cooldown, health penalty or automatic account switching; it does not revoke a login.
Runtime projections retain a quota/rate wait as `quotaWait`, rather than
collapsing it into unavailable. Account cards show the waiting state even when
SIWC does not provide counters; this does not create a zero quota measurement.
Configuration/auth failures and transient provider outages remain distinct.
Codex quota endpoints/headers, Agent Identity, direct native-profile writes
and Hosted transfer are not SIWC capabilities. Plan
limits are not a new independent allowance; absent quota evidence stays unknown.
Missing SIWC quota counters are displayed as not provided, not as a queued check.
Keep optional plaintext plan/expiry claims only from a verified ID token; neither
their presence nor quota counters are promised by the public SIWC token contract.
Encrypted access-token metadata stays opaque. Do not substitute Codex quota
endpoints or infer a plan from model access, scopes, email or another connection.
Reference: https://developers.openai.com/siwc/token-sharing-open-source/token-reference
Live provider acceptance remains separate from automated local checks.
Contract: https://developers.openai.com/siwc/token-sharing-open-source/sign-in
and its token, models-and-inference, preview-limitations and recovery references.

Zenith account bundles preserve all three OAuth client kinds. SIWC exports retain
the issued client ID, subject, issuer, host ID, scopes and refresh timing alongside
the tokens; Codex-only and unqualified third-party formats reject SIWC rather
than substituting the bootstrap client. Import detects the kind and checks
registration consistency. File metadata is not proof of authorization: every
SIWC import requires a fresh sign-in before token use, refresh, model
discovery or pool admission. Reauthorization reuses the issued client with this
installation's distinct host ID and verifies the selected subject. Import cannot
overwrite an existing SIWC connection; deleting an unactivated import does not
revoke the source host's grant. This conservative Relay policy prevents copied
rotating grants from becoming two active sessions; it is not a provider ban on
sharing a client registration. Host/client contract:
https://developers.openai.com/siwc/token-sharing-open-source

Credential authorities remain separate even for the same principal. Local account
cards can explicitly associate up to one Codex, SIWC and Excel/BPS connection.
The client badge selects the route in both Accounts and Rotation; only that
connection joins the pool. Association does not copy tokens, infer entitlement,
combine quota ledgers or automatically match accounts by editable login notes.
Codex/BPS association checks the saved principal and workspace; SIWC association
checks the original sign-in identity, not a workspace claim or shared allowance.
Association checks every existing connection, so SIWC cannot bridge mismatched
Codex/BPS workspaces or principals.
The user must select the intended workspace during authorization. Existing
connections are not silently merged. Each route keeps its models, quota,
authorization and renewal. Common login notes reference one retained credential
record; other saved notes are not erased. Pool policy, weight and concurrency
follow the selected connection. A switch fences dispatch and rejects while any
affected route has active requests; continuation identifiers are never rewritten.
Group membership and selected route are saved atomically with account records.
Explicit account selectors in automations follow the selected route in the same
transaction, without moving wake history or reset-credit fingerprints. A switch
rejects an enabled automation whose explicit model is unsupported or excluded by
the destination; edit or disable that automation first. Runtime rollback restores
task targets while preserving newer wake history.
Refreshing an inactive connection persists its own metadata without rebuilding
the active gateway or publishing another dispatch candidate.
The existing detach command retains credentials outside the pool; each card
resumes its own saved notes, without copying or overwriting credentials. It is
not exposed by the current account-card UI.
Shared notes remain with their original owner; the other card may need its login
notes entered again. Account-card deletion/export
explicitly includes all grouped connections; exported registrations remain
separate, and local card associations must be re-established after import.
Account and pool cards show the plan separately from the issuing-client badge:
Codex, SIWC or BPS. Missing legacy client metadata displays as Codex.
When SIWC omits its plan or quota windows, explicitly grouped Codex/BPS
observations fill the existing plan badge and quota meters without an extra section.
Stored SIWC metadata remains unchanged. Fresh primary/secondary windows from
the newest usable linked Codex or BPS connection also rank SIWC in automatic
rotation while SIWC has no own quota evidence. The reference retains its source
and expires at the oldest window's freshness bound, any reported reset, or
subscription expiry. Missing counters, failed reads, recovery, unavailable
credentials and reauthentication cannot supply this ranking signal.
Association does not verify SIWC's workspace or equivalent client allowances.
Search, filters and groups use the selected connection's own metadata. A missing
subscription date can display a linked Codex/BPS date for the same displayed plan,
with its source in the tooltip; it never changes the saved subscription or access.
Each OAuth connection displays only its own token expiry, including Codex and SIWC.
Display plan and quota sources are chosen independently;
quota recency uses the oldest displayed window observation, not a later snapshot
update. Windows are not added or mixed. Own SIWC zero quota, reported limits and
reauthentication remain authoritative. Recovery, unavailable credentials and
reauthentication exclude reference donors.
Source, observation time and the workspace/allowance caveat are available on
hover and keyboard focus, and in accessible labels, without extra info icons.
Reference data is marked stale
after 20 minutes (the core default observation bound), or earlier at a reported
reset/subscription expiry, failed/stale refresh, or missing/future timestamp.
Neither displayed references nor ranking hints grant execution capacity. Stale values retain
their warning and historical-data tooltip; local timers do not poll providers. Manual quota
refresh uses the owning Codex/BPS reader without switching the selected route,
including when the group has no saved counters yet. Bulk refresh deduplicates
reader accounts. Active group traffic keeps its linked readers on the same
active refresh cadence; the existing shared jobs, backoff and revision fences apply.
Reference refreshes update ranking without rebuilding the gateway or changing
the selected execution route. Zero reference quota only withdraws its ranking
advantage; it does not exhaust SIWC. Own quota, auth, cooldowns, model access and
opaque continuation ownership remain authoritative. References never copy
credentials, balances, reset credits or execution eligibility, and introduce no
extra background requests.
The route selector contains only the three clients. Missing connections start
their sign-in flow directly, without a connection-confirmation dialog. Existing
connections switch directly. The account-card menu has no connection-management
entry or dialog. Association still follows completed, validated route sign-in.
Route sign-in notes read and save the original card's existing note owner,
separately from the OAuth reauthentication target. This does not reuse another
client's grant or copy notes into an unverified account. Ordinary new-account
sign-in keeps its independent blank draft.
On Windows, macOS and Linux, ordinary selection of an unconnected Codex or
Excel/BPS route requests the existing protected automatic-login driver. The
route selector has no right-click sign-in shortcut.
The native command validates a separate notes-source account, never treats it as
the new client's reauthentication target, and keeps its proxy/profile context
through start, resume and token exchange. Pending sessions are additionally
scoped by that source account; legacy snapshots default to no source. Missing
email/password falls back to manual route sign-in. The sign-in dialog omits the
automatic-login description and retains its manual continuation action. SIWC
uses manual sign-in. Source removal/recovery stops automation
before another form fill.
Refresh, reauthentication, import, export, and transfer preserve the issuing
client. Legacy credentials without a client hint remain Codex; captured BPS
headers do not establish an OAuth client.
The connection determines the transport; there is no global BPS switch.
Legacy global flags are ignored on read and omitted from settings, presets,
and runtime snapshots. Per-account BPS facts remain credential-derived.

Native Codex connections are for direct account switching and as an explicitly
linked source of displayed SIWC plan/quota metadata, never for Desktop pool
admission. Sign-in, storage, export, renewal and quota reads remain supported.
Desktop admission, imports, presets and runtime assembly enforce this boundary;
startup removes legacy Codex membership without deleting credentials or notes.
Selecting Codex also removes the group's pool membership and routing policy;
enabled account-targeted automations must be edited or disabled first.
SIWC and BPS are pool-only and have no direct-launch button. They do not write
native Codex authentication. Known Free BPS connections cannot be imported,
discovered, or dispatched. Unknown plans do not prove access.

Client-login observations use only an unambiguous loopback CDP endpoint. Each
snapshot exchange has one connect/send/reply deadline; control frames and
unrelated responses do not renew it. Observations affect warnings and direct
launch, never saved credentials or pool admission.
CDP discovery bypasses system proxies and does not follow HTTP redirects.
Managed ChatGPT catalog reads also reach the local gateway directly, without
system proxies or HTTP redirects; provider proxy routing is unchanged.

Discovery preserves inventory separately from executable routes. A model name,
`/models` row, group label, or reference catalog does not prove endpoint access.
Normal refresh does not send generation probes. Endpoint declarations retain
their origin; address/key changes invalidate old evidence.

The renderer preserves Rust's resolved routes, including empty model lists.
Only an unresolved sole legacy native binding may inherit the source catalog.
Invalid adapters, duplicate routes, unavailable model references, and cache TTLs
on non-Messages routes cannot become native-launch or pricing capabilities.

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
- Access-token expiry and subscription expiry are separate metadata. Imports
  retain supplied access-token expiry and authenticated subscription dates;
  an ID token's expiry does not describe the access token. Missing dates stay
  unknown. Passive quota updates cannot discard an independent subscription read.
- Model, balance, quota, and usage refreshes are independent resource-scoped
  jobs. Readers join in-flight work. Manual refresh does not cancel shared work.
  Configuration/credential/delete revisions fence late results.
- Passive window headers do not refresh credit-ledger evidence. A reset time
  without a remainder does not refresh that remainder. Traffic cannot postpone
  a required full poll indefinitely; stale credit-only admission expires.
- The last positive 1% schedules a quota verification after one minute, or at
  an earlier reported reset, through the same shared refresh job. Fresh headers
  do not postpone it. Zero remains exhausted; provider Retry-After, backoff,
  concurrency limits and account revision fences still apply.
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
  Access-only expiry applies to the rejected token snapshot and preserves an
  existing terminal sign-in reason.
- Account trash retains credentials and usage but excludes the registration
  from routing, monitoring, exports, and presets. Restore returns it disabled
  and outside the pool with a new revision. Permanent deletion is explicit.

## Pool and execution

Automatic selection first filters compatible, enabled, available members, then
compares normalized load. Fresh positive quota takes precedence, including the
explicit linked-connection ranking reference for SIWC with unknown quota; candidates
less than five percentage points below the best fresh quota remain in the same
selection band. At a lead of five points or more the lower remainder yields.
Fresh provider credits are the fallback. Soft health/first-output latency and request weights
resolve remaining choices. Wallet prices do not rank members. Missing latency
does not exclude a member. Soft affinity can yield to a better eligible member;
opaque response ownership cannot.
Soft prompt affinity follows the admitted native route of its physical member
when protocol aliases are deduplicated. It cannot restore an excluded member
or move an opaque continuation to another route.

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
balance alone is not spendable capacity. Snapshots also publish `selectionRank`:
a read-only fallback preference, excluding all aliases of each preceding physical
member through the same selector. Ranking stops where text model/protocol routes
disagree or lack a candidate; it never reserves work or advances rotation.
Cards show members with active requests first, then follow this common preference
right-to-left, then down, ahead of waiting, unavailable and disabled/draining
members. Card contents retain their normal text direction. Only members with
active requests get a subtle card outline, without an activity badge; next
candidates remain neutral. Unranked cards
use fresh resource headroom in automatic mode and saved order otherwise. Timed
waits sort by retry time; quota waits, retry cooldowns and occupied capacity keep
distinct labels. Bare unavailable telemetry alone does not override a healthy
inventory status. New activity invalidates ranks and queues a coalesced lightweight
scheduler refresh; periodic polling remains the fallback. Active highlighting is
independent of readiness, so draining requests remain visible until released.
Recovery and retry details stay in status-icon hints, not duplicate lines in
account cards on Pool or Connections.
This is not a future dispatch queue: model, affinity and load can change selection.

Round-robin mode follows saved member order, skips unavailable/full members,
and wraps. Equal cycling ignores saved weights and soft affinity. Optional
weighted cycling uses physical-member request shares; shares are not token or
spending targets. Equal-credit ties use saved order, not protocol-alias IDs.
Failover selects the first ready member in saved priority order, without cycling.
A due lower-priority recovery route does not displace a healthy primary; a due
primary recovery still follows the bounded recovery admission policy.
A member limit covers all its protocol routes; zero means no extra member cap,
not unlimited provider capacity. Policy saves are serialized and identity-checked.

Rotation policy version three is advertised as `rotation_v3`. Known version-one
and version-two policies upgrade without changing inventory, member order,
weights, concurrency limits, or their equal-cycling semantics. Stored `in_order`
and `round_robin` remain equal cycling; `weighted_round_robin` explicitly opts
into shares, and `failover` explicitly opts into primary/reserve behavior.
Unknown versions and invalid legacy pairs are rejected, not normalized.
Portable preset imports may migrate validated legacy policies; direct local
updates stay version-exact. Retired server compatibility is not retained.

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

An uncertain or completed generation is not safe to repeat. Positive reported
output or reasoning usage proves generation even without visible content and
blocks retries and body repairs; it does not establish first-token timing.
Local saturation does not vote against provider health. Request-preparation
incompatibilities stay excluded for that logical request without consuming a
generation or cooling the member. Recovery cannot loop over the same unchanged
invalid route.
Quota, cooldown, health, access, and per-model blocks remain separate states.

## Continuations, errors, and redaction

An opaque continuation retains its physical owner unless validated local replay
materializes the complete history before dispatch. Complete portable call/result
pairs need only soft affinity; an opaque predecessor is validated separately.
Historic pairs from earlier owners do not override the predecessor's route.
Partial histories, unpaired results, unresolved references, or foreign ciphertext
cannot release ownership. Unknown response references return
`409 response_continuation_unavailable`. Compaction checkpoints explicitly
replace the predecessor; they are not discarded to force a retry.

An explicit pre-output `invalid_encrypted_content` rejection can repair one
reasoning item on the existing account route. Multiple items require a unique
`input[N].encrypted_content` parameter or the exact observed four-character
prefix/suffix diagnostic. Conflicting locators, duplicate identities/ciphertext,
malformed/duplicate error fields and encrypted compaction preserve the original
history and failure. Visible summaries, unrelated items and tool pairs survive.
The exact legacy code-less `Encrypted content for item rs_...` rejection is
accepted only when it uniquely identifies an existing reasoning item; conflicting
codes or locators remain terminal.
The existing single-item fallback, one-repair bound and owner fences remain;
uncertain execution or already emitted output never authorizes a replay.

Open WebSocket connections apply the same Automatic owner-yield policy as HTTP.
Only validated materialized history can transfer; opaque state keeps its owner
and live connection. Completed native WebSocket turns bind tool-call IDs through
the shared bounded capture, independently of full-history replay retention.
Incomplete and failed turns do not establish durable tool ownership.
Native WebSocket sends carry tool and reasoning diagnostics with the prepared
payload after the final authorization and payload guards. Connection setup,
bridge installation, and subsequent turns reuse those diagnostics rather than
rebuilding the request for telemetry. They are not cached across routes or turns;
continuation validation still runs immediately before payload preparation.

WebSocket writes, including control frames and HTTP fallback output, have a
60-second per-message deadline. This local I/O limit does not time out model
generation. A stalled write ends the affected connection; uncertain upstream
execution is not replayed. Reused connections register the next attempt before
writing so send failures retain request diagnostics and release the lease.

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
An upstream `429 rate_limit_exceeded` about TPM remains a rate-limit refusal,
even when its message says `Request too large`; that text alone does not prove
a terminal 413 or context overflow. Existing retry and ownership guards still apply.

For ordinary API-key sources only, a relayed ChatGPT `usage_limit_reached` or
`subscription_sharing_usage_limit_exceeded` does not establish that key's own
subscription window. Its reset cannot impose an account-wide cooldown: apply
short model-scoped backoff, preserving explicit `Retry-After`/`retry_after` and
configured source recovery delays. Native account deadlines and other providers'
quota errors keep their existing scope and resets; no stored cooldown is cleared.
The official [rate-limit guide](https://developers.openai.com/api/docs/guides/rate-limits)
defines rate headers, while the [reasoning guide](https://developers.openai.com/api/docs/guides/reasoning)
requires preserving opaque reasoning history. Diagnostic-based cleanup is a
bounded Relay compatibility policy, not a provider promise of portable ciphertext.

BPS generation rate limits cool only the rejected model unless the provider
explicitly reports an account-wide allowance or quota limit. Millisecond retry
hints retain a five-second minimum pause. Explicit organization/model TPM or
RPM errors establish an opaque quota-group identity, scoped to the upstream
endpoint. Only members that independently reported that group share its pause;
unknown accounts, other organizations, models and native connections remain
independent. Repeated rejection after a shared pause grows backoff from five to
sixty seconds without shortening provider deadlines. Successful recovery resets
backoff, not a newer active cooldown. Group observations are memory-only, bounded
to 4096 entries and ten minutes, and invalidated on removal or permission change.

Shared runtime HTTP clients disable reqwest's internal protocol retries. HTTP/2
refusals return to Relay's existing failure policy instead of silently resending
inside one recorded dispatch. This does not widen retry eligibility or change
the configured candidate limit, provider cooldowns, or uncertain-execution rules.

Native generation HTTP dispatches can opt into
`ZENITH_RELAY_HTTP_HEADERS_TIMEOUT_SECONDS` (1–1800; unset, zero or invalid disables
it). The request budget retains one monotonic deadline across authorization
repair, account changes and HTTP handoff; later attempts cannot extend it.
This covers connection, upload and waiting for response headers, not an isolated
upload-completion measurement. Receiving headers releases the guard; response
bodies and long SSE generation retain their existing behavior. Expiry returns
`http_headers_timeout` with HTTP 504 and Relay origin, does not cool the account,
and cannot authorize replay of possibly accepted work. Expired budgets do not
start another dispatch. BPS progress guards and native WebSocket are separate.

Routing JSON optionally retains the bounded model ID declared by the raw
upstream response before protocol or BPS translation. Request details distinguish
requested, routed and declared models. The declaration is not independent model
verification, never changes routing or pricing, and is absent in older records.
Frozen upstream sessions (`session_blocked_by_cyber_policy`) are terminal policy
refusals, not an account-wide credential failure or a reason to rotate.
Encrypted-history repair refuses ambiguous multiple reasoning ciphertexts and
encrypted compaction checkpoints without modifying history.

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

Relay bounds each incoming SSE event and the buffered pre-output prefix to
64 MiB. This is a local memory safeguard, not a provider limit or a cap on total
response length. Coalesced events share no total stream budget. Oversized events
fail explicitly, retain known usage, and do not authorize replay of uncertain
execution or penalize account availability. BPS keeps its separate 16 MiB event
limit.

SSE parsing borrows single-line data payloads and consumes coalesced adapter,
upstream-usage, and HTTP-to-WebSocket frames in place. With no pending tail,
complete frames are borrowed directly from the input chunk without allocating
or copying a frame buffer; only the incomplete tail is retained. Buffered
fragments move the incomplete tail at most once per chunk. Shared frame consumers
resume at the incomplete tail's last byte instead of rescanning it, preserving
split CRLF and per-event limits.
Native Responses forwarding uses the validated delta fast
path, including escaped text and tool arguments; usage, model identity, terminal
events, and opaque compaction stay on their full validation paths. These local
optimizations do not change service tiers, reasoning, or client cancellation.
Native WebSocket forwarding retains ordinary text/binary buffers when no error
rewrite is needed. Committed output bypasses inapplicable retry parsing, not
served-model checks. HTTP-to-WebSocket fallback consumes parsed JSON and opaque
payloads without cloning them or decoding a serialized copy. Converted streams
retain unchanged text, reasoning, refusal, and tool-call snapshots, including
unchanged normalized argument strings; closed Messages blocks still close once
and reject mutations. Deferred Gemini and tool-search calls stay behind their
output barriers before argument snapshots are copied or normalized.
The ChatGPT WebSocket setting controls client ingress and the managed profile,
not provider transport. Catalog changes do not override it. BPS and converted
routes use the HTTP executor behind the same client socket; usage records the
client transport. A failed client restart does not roll back a saved setting.
`routing.transport` separately records the executor's upstream HTTP/WS boundary,
native/conversion/BPS mode, HTTP-bridge reason, and observed response format.
This is dispatch provenance, not proof of provider acceptance or execution.
HTTP content type is observed rather than inferred from the client stream flag.
WS handshake time covers only the upgrade attempt, not auth preparation, queueing,
or content TTFT; a reused socket and older records leave it absent, not zero.
These fields share existing routing JSON storage and add no retry or transport policy.
Messages streams finalize native text/thinking snapshots when their block closes
and append signature fragments in place. Portable deltas still stream immediately;
only a fully closed response can supply replayable native continuation state.
Converted Messages tool arguments may stream incrementally, but their block-stop
is withheld until the complete upstream response passes validation. A failed
terminal cannot release an executable tool call or establish continuation state.
BPS search progress alone does not mark its output item as delivered; a terminal-only
search item is materialized before the shared protocol bridge validates completion.

An output limit may truncate a tool's JSON object. Responses and Chat clients
retain that incomplete argument string, the limit reason, and known usage;
Responses marks the function-call item incomplete. Only an end-of-input parse
failure in empty input or an object prefix is accepted, never malformed completed
calls or non-object input. Object-valued Messages/Gemini output and custom/tool-search
conversions still reject arguments they cannot represent. Incomplete turns have
no reusable continuation or history key, and Relay never closes the JSON for them.

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
The validated checkpoint is saved before final tool/terminal frames reach the
client, so an immediate tool-result request cannot outrun continuation storage.
Usage and lease settlement remain tied to client-stream completion.
Terminal native output moves into the checkpoint without copying its opaque
payload. Tool-argument normalization borrows unchanged input or an unchanged
buffered prefix; its owning string is retained/truncated in place, and a new
output buffer is allocated only when numeric spelling must change.
The streaming gateway takes the completed translated response by ownership,
without cloning its body or continuation. Taking the result does not reopen a
terminal bridge or repeat its output.
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
  Converted foreign tools with omitted `strict` explicitly use `strict: false`
  on Responses so its automatic strict normalization cannot change optional
  fields or object openness. Explicit booleans and schema contents are preserved;
  native Responses keeps its documented normalization default.
- Known model exclusions for temperature use the client's wire location,
  including Gemini `generationConfig.temperature`, before route admission;
  absent/null values do not request a sampling override.
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
  portable history, not an unfinished foreign tool loop. Without a native
  checkpoint, unsigned Messages summaries before that new user turn become
  visible assistant text on Responses/Gemini; they are never fabricated native
  reasoning. An available checkpoint still restores its original opaque state.
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
  Buffered and streaming discovery preserve these fields
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
  indexes fail closed. Transport framing is not a provider latency guarantee.
  Discovery and direct streaming share native tool identity/index validation;
  completed terminals with changed or missing calls fail before catalog
  continuation or client release.
  Discovery preserves canonical upstream failure messages instead of replacing
  them with generic stream errors. A normalized `upstream_rate_limited` stays a
  rate-limit failure through SSE conversion and usage recording, never a generic
  `upstream_terminal`; output already sent does not authorize replay.
  Untyped SSE error envelopes retain their category. Provider retry delays and
  scope survive client-error normalization in request-local settlement state;
  they are not shortened to the default cooldown or exposed as raw error bodies.
  Both consume coalesced SSE frames without shifting the remaining buffer for
  each event; frame/response limits and the first-terminal boundary still apply.
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
  `bodyMs`), catalog preload/fallback plus final encoding (`catalogMs`), and
  attachment scanning/cache/upload (`attachmentsMs`). These are durations,
  not dispatch-relative offsets. They describe successful preparation in the
  HTTP request/account executors; unobserved stages stay absent. Schema timing
  includes selecting/parsing the tool catalog and structured-output schemas,
  not just validator compilation. Compact fallback revalidation belongs to
  catalog time. Rounding and remaining dispatch setup can leave a gap to the
  total. The recorder retains only counters and does not change validation,
  caching, wire bodies, or upload/retry policy.
- Tool guidance prefers grouping related read-only shell inspections into one
  declared shell-tool invocation, with the configured shell and each command's
  output/failure preserved. Concurrency is limited to independent reads and must
  respect task/tool permissions, approval boundaries, dependencies, and shared
  state. This is guidance, not automatic command rewriting. Actual scheduling
  and approvals belong to the client; concurrent execution is not guaranteed.
- BPS also accepts an explicit Relay-local batch: ordered `references` names
  and `code` containing `relay-tool-batch-v1`, a newline, and a JSON payload
  array. There must be 2–8 calls and one payload per reference, with at most
  1 MiB of code and 1024 output items after expansion. Repeated names and
  namespaces are preserved. Function payloads are argument objects or encoded
  function strings; custom payloads must be strings and stay opaque.
  Batch headers accept LF or CRLF without modifying payload strings. Their JSON
  array and function-string frame headers use the same strict-first, lossless
  escape/control handling as single function payloads; invalid Unicode,
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
- Catalogs use compact schema presentation without changing the original
  validator. Complex/unknown JSON Schema constructs remain complete JSON.
  Compact property names and annotation keywords have deterministic ordering,
  so reordering those schema keys does not rewrite the rendered catalog prefix.
  Annotation rendering borrows their values rather than cloning nested schemas;
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
- Those checkpoints may also retain the Relay-generated catalog prologue, at
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
  Tool-enabled preparation, reasoning effort, and client compaction thresholds
  are unchanged. This includes tool-less compaction sent through ordinary
  Responses, not a change to `/responses/compact`. Prefix continuity does not
  guarantee provider cache hits or lower latency on the undocumented transport.
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
  twice, and cumulative time through the last lookup (already included in total
  request duration), not a sum of cumulative attempt durations. Schema bytes
  count compact preloads and unique full definitions introduced by lookups;
  reuse references do not count the same full schema again. Intermediate visible
  text, reasoning, and completed hosted web-search items are preserved in their
  original order, so the final output retains already-streamed item indices;
  native internal items replay only inside that request, not in a cross-request
  discovery store. Dropping the response drops discovery and its upstream body;
  no detached generation survives client cancellation.
- Inline-image uploads share in-flight success or failure for the same endpoint,
  account, credential fingerprint, media type and image bytes. Only file IDs
  enter the existing 512-entry cache; upload failures are not retained there.
  Up to 512 concurrent keys share request-owned initialization; excess new keys
  keep ordinary upload behavior. Weak entries do not retain requests or image
  data. Cancellation drops the active upload, without detached work; an existing
  independent waiter may perform its own upload. Credential and account scopes
  remain separate, and discovery continuations reuse uploaded file references.
  Within one request, at most four image jobs resolve concurrently after all
  images pass validation. Bytes, input order, and input-ordered failures stay
  unchanged; cancellation/failure drops pending work before generation.
- Opaque `previous_response_id`, non-Standard tiers, native strict output,
  native web search/image generation, and Agent Identity are unsupported.
  Full supplied history reconstructs the tool catalog; the preload hint cache
  is not a BPS provider session store and cannot restore opaque continuation.
- Explicit `relay_bps_output_validation` buffers output and validates JSON or a
  self-contained offline schema locally. It is not constrained decoding;
  `strict: true`, invalid/incomplete output, and external references fail.
  Admission checks native and converted output formats before reserving a BPS
  member; catalog/access discovery stays independent. Incompatible BPS routes
  cannot consume selection capacity ahead of a compatible native route. A BPS-only
  pool still rejects unsupported structured output; no schema or approval check is dropped.
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
rebuild the scheduler.

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

Detailed request telemetry is opt-in through the existing Desktop debug mode.
Without it, request context/schema byte counting, cache-context fingerprints
and comparisons, BPS catalog size diagnostics, and preparation-stage clocks
are skipped rather than calculated and hidden. The runtime reads the host's
setting without rebuilding the listener; admitted tool-policy captures retain
their original mode. The UI hides route, context, and tool diagnostic panels
outside debug mode, including those in historical rows; history is not erased.
Token/cache usage, pricing, total latency, TTFT/generation speed, outcomes,
attempt history, errors, and all authorization/schema/size guards stay active.
No debug setting changes routing, provider caching, or tool execution policy.

New request diagnostics retain only JSON byte counts for client and prepared
instructions, conversation, tool catalog, and whole body, across all four APIs.
These are not token estimates and do not include provider-injected context.
BPS's final prepared instructions include its embedded catalog; recording the
intermediate converted catalog alone is not evidence of final wire size.
BPS executor preparation leaves the final byte count to dispatch after catalog
preloading/fallback, rather than counting the soon-to-be-replaced body twice.
Cached tokens still occupy context; never subtract them from the displayed
window or remove valid instructions/tools merely to reduce the counter.

Usage records one final row per logical request and separate retained physical
attempts. Exact `requestId` history is not an additional cost/tokens total.
Deleting/expiring usage removes corresponding attempts; missing earlier history
is not reconstructed. Actual retry/stop decisions are recorded when known.
An interim successful-HTTP `tool_discovery` observation without error metadata
is pending, not a failed or completed logical request. Attempt timings are
elapsed from the logical request start, not additive per-send durations.
Failures after local BPS catalog lookup retain the bounded, redacted provider
error fields and observed upstream HTTP status in usage diagnostics, for both
buffered and streamed responses. Client-facing discovery errors remain generic;
retaining diagnostics does not authorize another generation or route replay.

Counters, served model, tier, timings, and safe provider error IDs come from
upstream observations. First output excludes heartbeats, empty deltas, lifecycle
events, terminal snapshots, and synthetic client SSE. Unknown timing stays unknown.
Partial cumulative counters merge without null erasure; checked derived totals
stay unknown on overflow, and reported totals are retained.
Request-size diagnostics reuse exact serialized value sizes for both body and
section totals instead of serializing the whole conversation a second time.
Callers that already measured the unchanged tool catalog reuse that byte count.
Outer object and conversation-array delimiters contribute to body bytes;
section totals retain their existing item boundaries. These are not token estimates.

Cache read/write counters remain per-request observations, including explicit
zeroes. Local cache activity is scoped by chat, account/source identity, and
upstream model. Only observed positive reads/writes move the touch; misses and
future or foreign retention records cannot supply its policy. The UI shows a
separate retention estimate, refreshes it through the existing usage revision,
and keeps previous observations after a failed read without extending them.
Model names do not prove an endpoint's TTL. Mixed prefix windows have no single
countdown, and an elapsed estimate is not evidence of provider eviction.

Managed account Responses requests preserve a client-supplied `prompt_cache_key`.
For Codex/BPS, an absent key and identified client context supply a deterministic
opaque key scoped to the local key and resolved model, shared by HTTP and client
WebSocket paths.
SIWC does not synthesize cache keys or upgrade the client's cache policy.
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
Desktop request details itemize input, cache read/write, and output using the
same query-time catalog snapshot and component rounding as the request total.
Reasoning stays included in output, not a second charge. Missing breakdowns or
rates remain unavailable; a measured zero is distinct from an unknown cost.

Debug context comparison uses bounded, salted in-memory fingerprints, never
stored prompts or arguments. It does not prove provider prefix reuse or a cache
hit. Native Responses and SIWC Responses share this diagnostic path, including
the HTTP bridge for WebSocket clients; debug-off requests collect no context
fingerprints. Client cache controls and reasoning settings remain unchanged.
In debug mode SIWC can request the provider's official prompt-cache comparison
against the last successful, non-overlapping response on the same local key,
client session/cache scope, account and model. This uses the existing bounded
in-memory comparison store; response IDs are never serialized into usage.
An explicit client comparison ID (including null) and cache mode/TTL are preserved.
Comparison is diagnostic only: it does not retrieve history, enable continuation,
change cache policy, add another inference, or authorize a retry. Provider
classifications and token counts are retained separately from local prefix
comparisons and observed usage; an unavailable comparison is not a cache miss.
Reference: https://developers.openai.com/api/docs/guides/prompt-caching
and https://developers.openai.com/api/docs/guides/prompt-caching/diagnostics
The manual `quality_probe` is opt-in, loopback-only, bounded, and separate
from background monitoring; see [operator steps](../../CONTRIBUTING.md#manual-quality-observations).

## Application profiles

Profile operations follow inspect → protected snapshot → apply → verify → restore.
Rollback changes only unchanged Relay-owned fields and preserves newer user edits
and sign-ins. Credential snapshots use protected storage. Invalid executable
overrides fail rather than launching another installation.

The local ChatGPT application settings expose separate memory extraction and
consolidation controls. Their model selectors update `memories.extract_model`
and `memories.consolidation_model` respectively. Selection is limited to enabled
Responses-routable pool models and requires an unchanged, active local-gateway
profile. Each command checks the previously read model and atomically replaces
only that stage's key; reset removes it. Models need a manual ChatGPT restart.
Existing memory settings, the main chat model and foreign profiles remain
user-owned. Pool availability does not guarantee memory-task context/capability
compatibility, and no automatic model fallback is added.

The two switches are persistent **Relay admission policies**, both allowed by
default, not invented Codex config keys. They apply independently to new HTTP
and WebSocket requests without restarting Relay. Explicit Codex metadata
`request_kind=memory` identifies extraction even though its `thread_source`
also says `memory_consolidation`; the latter source/trigger identifies the
consolidation worker for other request kinds. Neither model names nor prompt
text identifies a memory task. Clients without these markers cannot be gated
reliably. Blocked memory requests return a local 403 and a zero-dispatch usage
event, never a fabricated empty success that could complete a memory job.
Allowed requests are labeled by memory stage in usage. Disabling a stage does
not cancel in-flight work, clear memory, change `generate_memories`/`use_memories`,
or disable ordinary chat/context compaction. Allowing a stage does not force the
client to enable its memory feature. Activity-summary/title policy is separate.
OpenAI source contract: [memory configuration schema](https://github.com/openai/codex/blob/86e17e98c7f901277aeeb4649cf1b01462b082f2/codex-rs/core/config.schema.json),
[extraction metadata](https://github.com/openai/codex/blob/86e17e98c7f901277aeeb4649cf1b01462b082f2/codex-rs/core/src/turn_metadata.rs#L127-L155),
[request-kind semantics](https://github.com/openai/codex/blob/86e17e98c7f901277aeeb4649cf1b01462b082f2/codex-rs/core/src/responses_metadata.rs#L167-L185),
and [consolidation worker](https://github.com/openai/codex/blob/86e17e98c7f901277aeeb4649cf1b01462b082f2/codex-rs/memories/write/src/runtime.rs).

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

The retired personal-server runtime, management client, deployment templates and
account-transfer operations are removed. Existing ownership evidence is preserved
only for local recovery: affected accounts stay disabled and outside the pool
until the user confirms the external owner is stopped. Recovery never enables
an account or transfers credentials. Hosted is not a migration of that runtime.

## Not provided by the current implementation

Hosted tenants, customer identities/keys, wallets, payments, and billing are not
Desktop features. PostgreSQL checkpoints, encrypted payload
retention, SDK idempotency, and settlement are planned in
[HOSTED_RELAY_ARCHITECTURE.md](HOSTED_RELAY_ARCHITECTURE.md).

Distributed scheduling, account resale, fingerprint spoofing, sharing concealment,
and automatic transfer of opaque conversations between accounts are outside the
first hosted release. Open local acceptance and measured improvements belong in
[ROADMAP.md](ROADMAP.md), not in claims of completed behavior here.
