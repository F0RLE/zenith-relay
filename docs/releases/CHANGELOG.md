# Changelog

All notable Zenith Relay changes are recorded here. The `Unreleased` section
tracks merged or review-ready work that has not been published as a release;
release entries are kept concise and link to the corresponding tag.

## [Unreleased]

### Security

- Local and user-managed server API keys are fetched only after an explicit
  **Copy API key** action. Relay does not render or retain them in the desktop
  interface.
- API keys can be reissued from the API page. The replacement is copied
  directly, and the previous key is invalidated only after the new one is ready.

### Routing, availability, and usage

- Relay preserves a Codex session's preferred healthy route to retain upstream
  prompt-cache affinity, while still allowing bounded fallback when capacity,
  quota, or health requires it.
- Pool activity now follows the runtime's reported candidate, including a
  lower-priority stabilizer, and remains visible across concurrent state
  refreshes instead of predicting a «next choice» from the card order.
- A confirmed quota exhaustion cools down only the affected account/source and
  model route instead of taking the whole provider out of rotation.
- Namespaced API models stay on their declared API source; Relay no longer
  silently falls back to a ChatGPT subscription route for those requests.
- Source capability refreshes use the live upstream catalog. OAuth refresh-token
  reuse is treated as a temporary recovery state rather than forcing an
  unnecessary sign-in.
- The pool now exposes one request-speed choice: Standard or Fast. Managed
  ChatGPT/Codex requests follow it directly, while external API clients retain
  their explicit tier; the upstream-reported tier remains a diagnostic value.

### Pricing

- Replaced the hand-maintained OpenAI price file with one validated LiteLLM
  catalog shared by desktop and Relay Server. Account estimates use only an
  exact record in the account's declared official family; API sources resolve
  provider evidence, LiteLLM exact, declared-family canonical, then a manual
  source value.
- Kept input, cache-read, cache-write (5m/1h), output, and image/request
  tariffs independent. Missing components remain unpriced instead of falling
  back to another tariff or displaying `$0`.
- Added immutable catalog snapshots, local ETag/Last-Modified cache metadata,
  stale/offline fallback, atomic replacement, and revision-aware invalidation
  for usage and API-equivalent totals. Pricing remains informational and never
  changes routing or quota decisions.

### Desktop UI

- ChatGPT recovery now restores only Relay-managed configuration and sign-in
  state. Named and full-profile snapshots were removed, while reversible
  history repair remains active for ChatGPT-to-Relay and Relay-to-ChatGPT
  transitions.
- Pool connections now include OpenCode. Connecting writes a managed
  `zenith-relay` OpenCode provider with the currently enabled pool models and
  preserves the previous OpenCode configuration for one-click recovery.
- Relay-managed OpenCode models now advertise image attachments, so image
  inputs can be selected and forwarded through the pool.
- OpenCode reasoning variants follow the model's explicit Relay reasoning
  policy, so unsupported effort choices are not advertised to the client.
- The application picker remembers whether a connected application should be
  launched immediately, and its compact layout remains consistent across
  desktop window sizes.
- Tooltips now wait briefly for pointer hover, appear immediately for keyboard
  focus, and remain available for disabled actions without browser-native
  `title` popups.
- Usage refreshes no longer wait on an extra UI delay, while sign-in, setup,
  and snapshot countdowns share one deadline-based clock instead of separate
  polling intervals.
- The Usage view warms its default report after the runtime is ready and keeps
  a small per-query result cache, so returning to the page renders immediately
  while the latest aggregates refresh in the background.
- Update discovery starts with the application instead of an arbitrary startup
  delay, and compact icon-only controls keep their accessible action names.
- OpenCode recovery now mirrors the ChatGPT recovery view with an explicit
  original-config snapshot, creation date, path, and confirmed restore action.
- Relay-owned recovery files now use an application-first layout under
  `recovery/applications`, while legacy recovery directories migrate safely on
  startup without overwriting conflicts.
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

[Unreleased]: https://github.com/F0RLE/zenith-relay/compare/v1.1.1...main
[1.1.1]: https://github.com/F0RLE/zenith-relay/releases/tag/v1.1.1
[1.1.0]: https://github.com/F0RLE/zenith-relay/releases/tag/v1.1.0
[1.1.0-beta.1]: https://github.com/F0RLE/zenith-relay/releases/tag/v1.1.0-beta.1
[1.0.5]: https://github.com/F0RLE/zenith-relay/releases/tag/v1.0.5
[1.0.4]: https://github.com/F0RLE/zenith-relay/releases/tag/v1.0.4
[1.0.3]: https://github.com/F0RLE/zenith-relay/releases/tag/v1.0.3
[1.0.2]: https://github.com/F0RLE/zenith-relay/releases/tag/v1.0.2
[1.0.1]: https://github.com/F0RLE/zenith-relay/releases/tag/v1.0.1
[1.0.0]: https://github.com/F0RLE/zenith-relay/releases/tag/v1.0.0
