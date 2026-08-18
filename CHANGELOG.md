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
- Streamed Responses-to-Messages continuation coverage, including tool-context
  reuse across a follow-up request.

### Changed

- Pool cards now retain the configured routing order for API sources with multiple protocol routes.
- The source-route editor is more compact: formats are added on demand, model
  assignment columns stay aligned, and upstream API keys appear before routes.
- The OAuth success page is centered and schedules its browser tab to close ten
  seconds after the account callback succeeds.
- Usage request details now open as a compact overview with token, tool, and
  route sections; stream speed uses all output tokens over the full request
  duration without presenting successful requests as errors.
- Pool speed controls now use the clearer Standard/Fast terminology while
  preserving the existing service-tier behavior.
- Pool member cards fill the available grid width at larger windows while
  preserving a readable minimum width and responsive layout.
- Local snapshots preserve the canonical account state and show a sanitized
  warning when the active gateway has no matching OAuth candidate.
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
- API candidates remain eligible when a provider has not supplied reasoning
  metadata. An explicitly selected reasoning effort is a source/API policy,
  not proof that an account route supports that effort.
- Relay-managed profile restore preserves a user-changed
  `model_reasoning_effort`. Provider, base URL, authentication, and model
  catalog changes still block managed restore; an explicitly selected full
  snapshot restore may restore the snapshot as a whole.
- Provider quota presentation ignores expired reset timestamps and selects the
  next future reset.

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
