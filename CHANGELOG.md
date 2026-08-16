# Changelog

All notable Zenith Relay changes are recorded here. The `Unreleased` section
tracks merged or review-ready work that has not been published as a release;
release entries are kept concise and link to the corresponding tag.

## [Unreleased]

### Added

- Usage history now shows the requested reasoning effort and the normalized
  effort actually sent to the selected provider.
- Streamed Responses-to-Messages continuation coverage, including tool-context
  reuse across a follow-up request.

### Changed

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
