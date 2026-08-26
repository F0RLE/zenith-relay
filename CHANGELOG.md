# Changelog

All notable Zenith Relay changes are recorded here. The `Unreleased` section
tracks merged or review-ready work that has not been published as a release;
release entries are kept concise and link to the corresponding tag.

## [Unreleased]

- Tool-call continuations from Messages and Gemini API sources now stay on the
  exact source route that created the response, preventing route rotation from
  interrupting an active Codex task with a continuation mismatch.
- Responses Lite requests now follow the provider tool contract by sending
  serial tool execution explicitly and rejecting malformed values.
- Prompt-cache affinity no longer moves permanently to a temporary spillover
  account; the original account remains preferred until a real failure, even
  when another eligible account reports a fresher or larger quota. Provider
  source priority, exhaustion, health, and bounded load spillover still apply.
  Opaque prompt/session bindings now persist across Relay restarts, and
  rotating window or installation headers no longer split one session into
  separate affinity keys. Persisted bindings are restored before the first
  post-restart selection, so the cache owner is used immediately.
- Server pools can automatically redeem an available reset credit when a
  configured weekly quota reaches zero, with per-account locking and cycle
  deduplication.
- Usage diagnostics now record the safe upstream route kind used for each
  request (without hosts, credentials, prompts, or response bodies).

- Updated the bundled official OpenAI API reference prices for GPT-5.6 Sol,
  Terra, and Luna, including cached-input and cache-write rates.
- The API-source editor now separates connection settings, manual model and
  format routing, and per-source pricing into clear tabs. Model refresh stays
  beside the saved connection summary and checks only the saved source.
- Source-policy editing now uses pointer-based dragging in the desktop WebView:
  sources can be reordered within a role or dropped directly onto API first,
  stabilizer, or last-resort roles. Saving closes the policy window immediately
  while Relay persists and refreshes the updated rules in the background.
- Pool model rules now use WebView-safe pointer dragging with normal wheel
  scrolling during a move, visible drop targets, and collapsible provider
  groups. Native drag events keep a text payload as a compatibility fallback.
- Replaced the lifetime-based monetary "Potential" estimate with **API equiv.
  left**. It appears only when Relay has complete priced usage recorded since
  the current provider quota window began; the estimate excludes activity
  outside Relay and is omitted when the input is incomplete.
- Added request-count and end-to-end output-speed charts to the Overview for
  every selected period.
- Reworked the README and localized Help guides around the three user-facing
  modes, setup, quota/reset behavior, recovery, privacy, and troubleshooting.
- Regenerated the Overview, Connections, Pool, and Usage screenshots from the
  current desktop UI.
- Clarified that request speed (`Standard`/`Fast`) is not a second user-facing
  quota; Fast/priority provider metadata is no longer rendered as another quota
  meter.
- Reauthentication can now target the exact expired ChatGPT account. A fresh
  OAuth login keeps the account's local routing and settings, does not restore
  an expired subscription date without new provider metadata, and preserves a
  usable model catalog when quota or discovery checks temporarily fail.
- Completed the Responses bridges for function, namespace, and direct custom
  tools across Anthropic Messages and Gemini, including tool-choice filtering,
  JSON/SSE continuations, multimodal input, thinking metadata, and normalized
  usage.

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
- Catalog refresh runs at startup and every eight hours during an active app
  session. Catalog failures stay visible after restart; reasoning modes remain
  catalog metadata and changing a reasoning setting does not probe a provider.
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

[Unreleased]: https://github.com/F0RLE/zenith-relay/compare/v1.1.0...main
[1.1.0]: https://github.com/F0RLE/zenith-relay/releases/tag/v1.1.0
[1.1.0-beta.1]: https://github.com/F0RLE/zenith-relay/releases/tag/v1.1.0-beta.1
[1.0.5]: https://github.com/F0RLE/zenith-relay/releases/tag/v1.0.5
[1.0.4]: https://github.com/F0RLE/zenith-relay/releases/tag/v1.0.4
[1.0.3]: https://github.com/F0RLE/zenith-relay/releases/tag/v1.0.3
[1.0.2]: https://github.com/F0RLE/zenith-relay/releases/tag/v1.0.2
[1.0.1]: https://github.com/F0RLE/zenith-relay/releases/tag/v1.0.1
[1.0.0]: https://github.com/F0RLE/zenith-relay/releases/tag/v1.0.0
