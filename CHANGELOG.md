# Changelog

All notable Zenith Relay changes are recorded here. The `Unreleased` section
tracks merged or review-ready work that has not been published as a release;
release entries are kept concise and link to the corresponding tag.

## [Unreleased]

### Added

- Compatible Messages sources can select a 5-minute or one-hour prompt-cache
  write lifetime. Usage records retain the lifetime the provider actually used.
- Usage history now shows protocol and cache-write tokens, refreshes from
  recorded requests, and lets users choose the visible summary metrics.
- Provider-reported quota windows are shown in Connections and Pool, with
  separate visibility controls for the optional account value summary.
- Regression coverage keeps provider quota, API-equivalent usage, and purchase
  cost as separate values.
- Usage history now shows the requested reasoning effort and the normalized
  effort actually sent to the selected provider.
- Usage now shows provider generation speed separately from the full-request E2E
  speed, keeps E2E speed in Overview, and lets the summary include generation speed.
- Streamed Responses-to-Messages continuation coverage, including tool-context
  reuse across a follow-up request.
- Explicit `Responses -> Gemini` source routing for discovered Gemini models;
  the new bridge starts unassigned and does not advertise a model until it is
  selected.

### Changed

- The desktop shell now shows a static startup screen immediately while the
  WebView and first runtime snapshot finish loading, then removes it after the
  interactive frame with a bounded fallback timeout.
- Startup runtime snapshots no longer wait for the diagnostic SQLite write;
  performance telemetry is persisted asynchronously after the state response.
- WebSocket turn state is now accepted only for a known session/account owner;
  single-lane WebSocket connections preserve one `stream_id` and reject a
  second lane instead of silently treating multiplexing as supported.
- Managed Codex profiles enable Responses WebSocket transport. Relay probes
  each candidate/model, uses native upstream WebSocket when available, and
  bridges HTTP/SSE-only providers without removing them from the pool.
- Image models now show official per-request generation/edit prices, and API
  image requests keep the selected `gpt-image-*` model instead of forcing
  `gpt-image-2`; native accounts continue using the Responses image tool.
- A confirmed native Messages model is now linked into the same pool for
  Responses clients through Relay's Messages adapter. Explicit native
  Responses and Gemini assignments still take precedence for that model.
- Configured API source order now takes precedence over prompt-cache affinity;
  response-owner affinity remains intact for protocol continuations.
- Pool cards now retain the configured routing order for API sources with multiple protocol routes.
- Source discovery keeps the native Responses catalog fallback fresh when
  other models are assigned to a Messages or Responses-to-Messages route.
- The source-route editor is more compact: formats are added on demand, model
  assignment columns stay aligned, and upstream API keys appear before routes.
- The OAuth success page is centered and schedules its browser tab to close ten
  seconds after the account callback succeeds.
- Usage request details now open as a compact overview with token, tool, and
  route sections.
- Generation speed now uses successful post-first-output intervals and the
  remaining visible output tokens, excluding separately reported reasoning;
  the full-request E2E speed remains in Overview.
- Pool speed controls now use the clearer Standard/Fast terminology and sync
  Codex's official priority setting with the selected pool tier.
- Pool member cards fill the available grid width at larger windows while
  preserving a readable minimum width and responsive layout.
- Local snapshots preserve the canonical account state and show a sanitized
  warning when the active gateway has no matching OAuth candidate.
- Profile recovery now creates one first-launch original snapshot, exposes only
  full restore with Yes/No confirmation, never saves a hidden pre-restore copy,
  and guards snapshot deletion with a ten-second confirmation cooldown.
- Profile recovery now adopts a rotated OAuth token for the same account before
  restoring the native ChatGPT profile, avoiding false `profile_restore_blocked`
  errors.
- History recovery now updates only threads linked to processed rollouts,
  rewrites every relevant session marker, and keeps recovery paths portable on
  Windows.
- Global operation notifications now stay in a bottom-left overlay above the
  Help controls instead of shifting the page from the upper-right corner;
  compact sidebar mode uses a small status toast and opens error details in a
  centered dialog.
- Model lists and source price editors order familiar model families
  semantically by company, tier, version, and variant; unknown model IDs keep
  their upstream order.
- Connections, Pool, and Usage now show direct token-based API-equivalent and
  optional purchase-cost payback beside the provider-reported quota window.
  Relay no longer turns a quota percentage into a monetary potential; legacy
  calculation state migrates to the direct purchase-cost field.
- Reasoning defaults now use a verified model whitelist when available (with
  separate levels per company/model); provider declarations remain the
  fallback for unknown models. Model Rules still allows a manual override and
  an optional local Pool probe.
- Relay writes the selected model's valid reasoning effort into the managed
  Codex profile on activation. Profile restore removes Relay's own value,
  while preserving a user-changed `model_reasoning_effort`; provider, base
  URL, authentication, and model catalog changes still block managed restore.
- Provider quota presentation ignores expired reset timestamps and selects the
  next future reset.
- API-equivalent totals now update incrementally in SQLite instead of regrouping
  the full request log on every refresh; the 30-day raw-log retention and
  long-term usage totals remain separate.

### Maintenance

- Shared Playwright configuration now covers application and documentation
  runs consistently.
- Removed redundant build-script passes and documented the release workflow,
  roadmap, and branch integration state.

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

[Unreleased]: https://github.com/F0RLE/zenith-relay/compare/v1.1.0-beta.1...main
[1.1.0-beta.1]: https://github.com/F0RLE/zenith-relay/releases/tag/v1.1.0-beta.1
[1.0.5]: https://github.com/F0RLE/zenith-relay/releases/tag/v1.0.5
[1.0.4]: https://github.com/F0RLE/zenith-relay/releases/tag/v1.0.4
[1.0.3]: https://github.com/F0RLE/zenith-relay/releases/tag/v1.0.3
[1.0.2]: https://github.com/F0RLE/zenith-relay/releases/tag/v1.0.2
[1.0.1]: https://github.com/F0RLE/zenith-relay/releases/tag/v1.0.1
[1.0.0]: https://github.com/F0RLE/zenith-relay/releases/tag/v1.0.0
