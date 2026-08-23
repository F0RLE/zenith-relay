# Changelog

All notable Zenith Relay changes are recorded here. The `Unreleased` section
tracks merged or review-ready work that has not been published as a release;
release entries are kept concise and link to the corresponding tag.

## [Unreleased]

No changes are currently queued for the next release.

## [1.1.0] - 2026-08-23

Zenith Relay 1.1.0 is the first complete Relay release after Zenith Codex
1.0.5. It changes the product from a small desktop API client into a
local-first personal relay for a user's own ChatGPT accounts and compatible API
sources. Relay is separate from the production Zenith Gateway, Control API, and
account pool.

### 1.0.5 -> 1.1.0 at a glance

| Area | 1.0.5 | 1.1.0 |
| --- | --- | --- |
| Product | Desktop client focused on a single API-key workflow | Local-first Tauri relay with a private OpenAI-compatible endpoint |
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
  response-owner affinity.
- Added provider quota windows in Connections and Pool. Provider quota,
  direct API-equivalent usage, and optional purchase-cost payback remain
  separate values; a quota percentage is never treated as money.
- Added redacted account export, diagnostics, snapshots, telemetry, and usage
  history. Prompts, response bodies, cookies, authorization headers, and keys
  are not recorded.

### Sources, models, and routing

- Added provider-neutral Responses, Messages, Chat Completions, and validated
  `Responses -> Gemini` route contracts, including bounded continuation state
  for Responses-to-Messages tool flows.
- Added source model discovery with provider/manual price provenance, image
  generation/edit prices, semantic model ordering, and confirmed reasoning
  capabilities.
- Catalog refresh runs at startup and every eight hours during an active app
  session. Catalog failures stay visible after restart; reasoning probes remain
  manual and changing a reasoning setting does not start a background probe.
- Reasoning policies apply only to pooled API sources. Native OAuth catalogs and
  native request capabilities remain unchanged.
- Added native upstream WebSocket support with an HTTP/SSE bridge for providers
  that do not expose WebSockets. A single WebSocket lane keeps one `stream_id`
  and rejects an unknown session or second concurrent lane.
- Source order now wins over prompt-cache affinity for initial routing, while
  response-owner affinity remains available for protocol continuations.

### Quotas, resets, and usage

- Added explicit weekly reset-credit status and a simple Yes/No confirmation
  flow for an available reset. The automation path is weekly-limit aware; it
  does not confuse a five-hour window with the weekly reset.
- Background quota, model, and wake workers run only while an active Relay
  session is open. Tray-only startup does not perform provider checks.
- Added prompt-cache lifetime reporting, protocol and cache-write token fields,
  requested versus normalized reasoning effort, provider generation speed, and
  full-request E2E speed.
- API-equivalent totals update incrementally in SQLite while raw request logs
  keep their bounded retention policy.
- Pool service tiers now use Standard/Fast terminology and synchronize Codex's
  official priority setting with the selected tier.

### Profile recovery and persistence

- Added reversible Codex profile attachment with one first-launch original
  snapshot, named snapshots, full restore verification, and a visible Yes/No
  confirmation. Hidden pre-restore copies are not created.
- OAuth rotation recovery adopts a newer token for the same account before
  restoring the profile, avoiding false `profile_restore_blocked` failures.
- History repair updates only threads linked to processed rollouts, rewrites
  relevant session markers, and keeps recovery paths portable on Windows.
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

- Added a standalone user-managed server with encrypted vault storage, SQLite
  state, append-only migrations, management API, protocol negotiation,
  backup/restore, and strict redaction.
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

### Release verification

- 79 frontend unit tests, 120 visual Playwright scenarios, 177 operational
  Playwright scenarios, and 354 serialized desktop Rust tests passed.
- Rust formatting, Clippy, dependency audits, Linux Secret Service checks,
  cross-platform packaging, updater-manifest generation, and release asset
  validation passed for the 1.1.0 release.

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
