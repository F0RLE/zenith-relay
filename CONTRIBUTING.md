# Contributing to Zenith Relay

Zenith Relay is a local-first desktop application. Keep changes small, prove
the behavior they alter, and do not move private Zenith backend concerns into
this repository.

## License and contributor agreement

Zenith Relay is published under **AGPL-3.0-only**, as recorded in [LICENSE](LICENSE)
and the package manifests. AGPL grants permission to use, modify, and distribute
the project; it does not transfer a contributor's copyright to the maintainer.
Keep its standard license text intact.

Before submitting a PR, read the [Contributor Agreement, version
1.0](CONTRIBUTOR_LICENSE_AGREEMENT.md). It assigns copyright in your accepted,
original Contribution to the Project Owner, `F0RLE`, and permits relicensing of
rights the Project Owner actually receives. You retain a license to reuse your
own Contribution. Existing AGPL grants and third-party licenses remain in force.
This agreement does not apply retroactively to earlier contributions or impose
extra conditions on people who only use, distribute, or fork the project.

For each PR, describe the change and validation, open the agreement linked in
the template, and check **one Contributor Agreement checkbox** yourself. The
checkbox covers agreement, assignment, authority, and required disclosures
together. Keep its wording unchanged. If the Contribution is entirely your own
and has no other rights holders or excluded material, no extra ownership form
or separate consent comment is required by this workflow.

Only when relevant, list co-authors, employer ownership, and included
pre-existing or third-party material with its source and license. Obtain
consent from every relevant rights holder; co-authors can repeat the same
confirmation in comments under their own accounts. An authorized employer
representative or a separate signed instrument may be needed. Do not publish
private identity or employer documents in the PR.

Use the agreement version in the PR's **base branch**, not changed terms
proposed by that PR. Keep consent accurate when updating the PR. If the
agreement version changes, read it and give fresh consent before acceptance.
To withdraw consent before acceptance, say so explicitly in the PR and clear
the checkbox.

The `Release context` check validates the template and confirmation. It does
not verify legal identity, employer authority, or co-author consent. Before
acceptance, the maintainer must review those records and retain the accepted
commits, agreement version, and consent. Complete any legally required separate
signature or identity formalities privately before accepting the Contribution.

Only dependency maintenance PRs from GitHub's `dependabot[bot]` that touch
manifests, lockfiles, or Actions workflow files are exempt from the human PR
template. That exemption does not assign ownership of dependencies or cover
original work by human authors; the maintainer must review provenance and obtain
their consent separately. Other bot PRs are not exempt.

## Repository boundaries

| Area | Owns |
| --- | --- |
| <code>src/src</code> | React UI, i18n, typed snapshot rendering, and Tauri command wrappers. |
| <code>src-tauri/src</code> | Desktop storage, OS secret services, OAuth callbacks, local process lifecycle, profile attach and recovery. |
| <code>crates/relay-core</code> | Shared account/source state, scheduler, gateway execution, quota, protocol, and redacted usage logic. |
| <code>relay-server</code> | Standalone user-managed runtime, encrypted vault, SQLite state, migrations, and management API. |

The frontend does not access files, secrets, provider endpoints, or client
configuration directly. Keep private provider economy, customer billing,
Zenith inventory, and public gateway business logic out of this repository.

## Secret and logic boundary

Zenith Relay is a separate desktop/personal-pool product. It must not receive
or forward Zenith production credentials, customer API keys, backend tokens,
account-pool inventory, provider cabinet credentials, or internal Gateway and
Control API business/routing logic. Do not copy those values into code,
fixtures, documentation, tests, or support artifacts.

User-owned provider secrets belong in the existing desktop credential store or
the encrypted vault of a server owned by that user. A desktop-to-server transfer
is allowed only through an explicit, confirmed management operation targeting
that user-managed Relay Server. It is not an upload to Zenith production.

Remote state, usage, telemetry, exports, diagnostics, screenshots, and API
snapshots must stay redacted. They may contain identifiers, models, timings,
status, and aggregates, but never raw credentials, cookies, authorization
headers, prompts, response bodies, or provider session material.

## Safety rules

- Use stable dependencies only.
- Never put credentials, cookies, authorization headers, session exports, or
  account identities in source, fixtures, screenshots, logs, snapshots, exports,
  telemetry, or support output.
- Keep desktop secrets in the existing credential-store path and server secrets
  in the existing encrypted vault.
- Management tokens and pool request keys are different credentials. Do not
  accept either one in the other's boundary.
- Never use a management token as a `/v1` profile credential, and never expose
  either credential in a snapshot, log, export, or example.
- Preserve the distinction between quota monitoring and routing eligibility.
  Do not reinstate a Free-account routing policy or a hard-coded quota window.
- Retry another candidate only after a proven pre-execution rejection or
  not-sent result, and before any response bytes reach the client. Preserve
  response ownership; saved history proves portability, not execution safety.
- Database migrations are append-only. Add a new numbered migration; never
  edit a migration that can already have been applied.
- Update a profile through the existing inspect, snapshot, attach, verify, and
  restore flow. Never overwrite a newer user login.

Read [AGENTS.md](AGENTS.md), [PLANNING.md](docs/project/PLANNING.md), and
[ROADMAP.md](docs/project/ROADMAP.md) before changing a cross-cutting behavior.

## Documentation policy

The tracked human-facing documentation is deliberately small:

~~~text
README.md
CONTRIBUTING.md
docs/project/PLANNING.md
docs/project/ROADMAP.md
docs/project/ROTATION_DESIGN.md (target design; see ROADMAP for open gates)
docs/releases/CHANGELOG.md
docs/help/<locale>/README.md
docs/screenshots/*.png
~~~

<code>AGENTS.md</code> is repository guidance. <code>LICENSE</code> and
<code>CONTRIBUTOR_LICENSE_AGREEMENT.md</code> are legal documents, and
<code>relay-server/openapi.yaml</code> is the machine-readable server contract.
Do not add parallel architecture, design, handoff, or historical planning
documents. The existing <code>docs/project/ROTATION_DESIGN.md</code> is a
user-requested target design, with a connected core but unfinished acceptance
gates. Do not mark the entire design implemented or use it as current-runtime
evidence. Put implemented contracts in
<code>docs/project/PLANNING.md</code>, accepted unfinished work in
<code>docs/project/ROADMAP.md</code>, and user steps in localized Help files.

### Changelog and release notes

Record every user-visible change in
[CHANGELOG.md](docs/releases/CHANGELOG.md) under
`Unreleased`, grouped by behavior rather than by branch. Include the relevant
PR or commit in the entry when the change is ready for review. When publishing
a tag, move the shipped entries into a dated version section and leave
`Unreleased` available for the next cycle. Release-body translations used by
the updater remain separate and must use the `relay-notes:<locale>` markers
described below.

Keep the audience boundary explicit: changelog entries and updater notes are
for users. Do not put test counts, CI job status, formatter or audit output,
review history, or unfinished acceptance evidence in them. Keep contributor
commands in this file, current implementation facts in `docs/project/PLANNING.md`,
and unfinished or live-provider acceptance gates in `docs/project/ROADMAP.md`.

Stable release tags must have a matching dated section in
`docs/releases/CHANGELOG.md`. The
release workflow fails when that section is missing instead of publishing an
automatically generated pull-request list as user-facing notes.

Define stable error codes in `crates/relay-core/src/error_codes.rs` and reuse its
constants in emitters and classifiers. Add causes and recovery steps to both
localized Help error tables, including public aliases. The catalog test rejects
undocumented codes; classification and recovery policy remain in their owning
modules. Preserve unknown external provider codes instead of forcing them into
a Relay enum.

To add a locale, add one sequential guide at
<code>docs/help/&lt;locale&gt;/README.md</code>, register its translation
resources, and update the bundled Markdown registry in
<code>src/src/features/relay/help/HelpCenter.tsx</code>. Keep the guide
accurate for the UI's current labels and section order.

Screenshots are generated from the mocked desktop shell. Change the scenario
when the UI changes, then regenerate rather than editing images by hand:

~~~powershell
cd src
bun run screenshots
~~~

## Verification

Run the narrowest relevant checks while iterating. Before a commit that changes
the frontend, desktop host, shared runtime, or server, run the corresponding
commands below.

For PR template or metadata-workflow changes, run the isolated policy tests:

~~~powershell
bun test ./.github/tools/pr-metadata.test.mjs
~~~

The metadata workflow loads its validator from the PR's base commit. It must
never check out or execute PR-head code in `pull_request_target`, interpolate
PR text into scripts, or require write permissions or repository secrets.
The regular Build workflow also runs these tests against the proposed changes
in its unprivileged `pull_request` context.

### Frontend and desktop

~~~powershell
cd src
bun run check
bun run test:unit
bun run build
bun run test:e2e
~~~

For a release UI or layout change, run the focused Playwright suites as well:

~~~powershell
cd src
bunx playwright test tests/e2e/visual-matrix.spec.ts
bunx playwright test tests/e2e/operations.spec.ts
bun run screenshots
~~~

The visual matrix is the responsive/appearance gate; the operations suite is
the interaction and state-transition gate. The screenshots command regenerates
only the committed `docs/screenshots` assets.

<code>bun run verify</code> runs the unit tests, frontend build (including the
TypeScript build), and desktop Rust tests. Packaging or updater changes also require:

~~~powershell
cd src
bun run app:build
~~~

### Shared runtime

~~~powershell
cargo fmt --manifest-path crates/relay-core/Cargo.toml --all -- --check
cargo check --manifest-path crates/relay-core/Cargo.toml --all-targets --locked
cargo clippy --manifest-path crates/relay-core/Cargo.toml --all-targets --locked -- -D warnings
cargo test --manifest-path crates/relay-core/Cargo.toml --locked
~~~

### User-managed server

~~~powershell
cargo fmt --manifest-path relay-server/Cargo.toml --all -- --check
cargo check --manifest-path relay-server/Cargo.toml --all-targets --locked
cargo clippy --manifest-path relay-server/Cargo.toml --all-targets --locked -- -D warnings
cargo test --manifest-path relay-server/Cargo.toml --locked
~~~

Use the real server acceptance gate in
[ROADMAP.md](docs/project/ROADMAP.md) before
claiming that the remote pool works in production. Unit and mocked browser
tests cannot prove real account, proxy, streaming, or server persistence
behavior.

## Change and release flow

1. Inspect the active branch and working tree. Preserve unrelated local work.
2. Change the owning layer and update its callers only when its contract
   changes.
3. Add or update the smallest regression test that would fail without the
   behavior.
4. Run the relevant checks and regenerate screenshots if their UI changed.
5. Review the diff for secret leakage, stale Help wording, and generated-file
   noise.
6. Update <code>docs/releases/CHANGELOG.md</code> for user-visible behavior, or state in the
   PR why no entry is needed.
7. Verify contributor consent and rights, required checks, and the applicable
   CODEOWNER review. Target the development/release branch designated by the
   maintainer; do not assume every PR should go directly to <code>main</code>.
8. Merge reviewed release work into <code>main</code> when the maintainer is
   ready to release it. Tag and publish a product release
   after the release checks pass; reserve a <code>production-ready</code> claim
   for the live acceptance gates in <code>docs/project/ROADMAP.md</code>.

### macOS distribution

macOS builds use Tauri's ad-hoc identity (`APPLE_SIGNING_IDENTITY=-`); they do
not need an Apple Developer Program membership or Apple secrets. CI checks the
app inside the DMG and the updater archive for a valid ad-hoc signature, then
publishes after all platform builds succeed. Release assets include SHA-256
checksums. Ad-hoc signing does not establish a trusted developer identity and
is not Apple notarization, so downloaded apps still require the one-time macOS
approval described in the localized Help. Test fresh installation and in-app
updating on separate Intel and Apple Silicon Macs before claiming that those
flows work end to end. Never remove quarantine from the whole system or all
downloads. The `TAURI_SIGNING_PRIVATE_KEY` secret remains required for signed
in-app updates; it is separate from macOS code signing.

### GitHub merge requirements

In GitHub branch protection or rulesets for the development/release branches
and <code>main</code>, require PRs, the <code>Release context</code> status check,
an up-to-date branch, and code-owner review for the paths in
<code>.github/CODEOWNERS</code>. Restrict
bypass permissions to the intended maintainers. These files alone do not enable
branch protection. A PR author cannot approve their own PR as a code owner;
owner-authored policy changes need another authorized reviewer or an explicitly
managed maintainer exception.

GitHub loads the <code>pull_request_target</code> workflow from the repository's
default branch; this workflow explicitly loads the validator from the PR's base
commit. Bootstrap the workflow in the default branch and the agreement,
template, validator, and tests in each target branch through maintainer review
before making the check required. If an Actions event policy blocks
<code>pull_request_target</code>, allow this reviewed metadata workflow there
before relying on the check. Do not enable unreviewed workflows or broaden token
permissions as a workaround.

Policy changes use the agreement currently in the base branch and must not use
their proposed wording to authorize themselves. Keep the template's agreement
link pointed at the reviewed version in the designated development/release
branch. When changing that link or agreement terms, update the template,
validator, and tests together; terms changes also require a new version and
fresh consent on open PRs. Update those PRs against the new base before merging.

### Updater changelog for release admins

The updater changelog is read from the GitHub Release body. In the published
Release, put each translation after a <code>relay-notes:&lt;locale&gt;</code>
marker. A section continues until the next marker:

~~~markdown
<!-- relay-notes:en -->
- English changes

<!-- relay-notes:ru -->
- Изменения на русском
~~~

Use lowercase locale codes. For a new language, add another section such as
<code>&lt;!-- relay-notes:zh --&gt;</code>; no updater code change is required. The
app selects the exact locale, then its base language, then English, and finally
the first available section.

After editing the Release, rerun the <code>Publish updater manifest</code> job.
It copies the current Release body into <code>latest.json</code>; editing the
Release without rerunning that job does not update the in-app changelog.
