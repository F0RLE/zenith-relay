## Target Branch

- [ ] This PR targets main.

## Summary

<!-- Required: explain the change in a few concrete sentences. -->

Describe what changed and why.

## User-visible changes

<!-- Required: write what a user will notice, or explain why there is no user-visible change. -->

-

## Release Notes

<!-- Required: choose one option and complete the release note when applicable. -->

- [ ] Release-worthy change is described below.
- [ ] No release note is needed because this is internal-only, documentation-only, or test-only.

### Ready-to-publish note

<!-- Write 1-3 concise bullets that can be copied into the GitHub Release body. -->

-

<!-- For in-app updater text, use the release body markers documented in CONTRIBUTING.md, such as relay-notes:en and relay-notes:ru. -->

## Validation

- [ ] cd src && bun run check
- [ ] cd src && bun run build
- [ ] cd src && cargo test --manifest-path ../src-tauri/Cargo.toml --locked
- [ ] Not run (explain why below).

## Compatibility and migration

- [ ] No migration, config, updater, or compatibility impact.
- [ ] Impact is described below.

Details:

## Risk and rollout

<!-- Mention packaging, updater behavior, Codex config writes, rollback, or operational risks. -->

Describe the risk and the rollout or rollback plan.

