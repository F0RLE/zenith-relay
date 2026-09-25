## Contributor Agreement

<!-- Read the linked agreement, then check this box yourself. Keep the wording unchanged. -->

- [ ] I have read and agree to the [Contributor Agreement v1.0](https://github.com/F0RLE/zenith-relay/blob/release/1.1.3/CONTRIBUTOR_LICENSE_AGREEMENT.md), including its copyright assignment. If this PR is accepted, I assign to F0RLE the copyright I own in my original changes and confirm that I have permission to submit them.

<!-- Only if applicable: list co-authors, employer-owned or pre-existing material, third-party sources/licenses, and their consent below. Otherwise leave this blank. Keep private identity and employer documents out of the PR. -->

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

- [ ] `cd src && bun run verify`
- [ ] `cd src && bun run test:e2e`
- [ ] Not run (explain why below).

## Compatibility and migration

- [ ] No migration, config, updater, or compatibility impact.
- [ ] Impact is described below.

Details:

## Risk and rollout

<!-- Mention packaging, updater behavior, Codex config writes, rollback, or operational risks. -->

Describe the risk and the rollout or rollback plan.
