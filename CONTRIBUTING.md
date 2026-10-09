# Contributing to Zenith Relay

Zenith Relay is a local-first desktop application. Keep changes small, prove
the behavior they alter, and do not move private Zenith backend concerns into
this repository.

## License and contributor agreement

Zenith Relay is **AGPL-3.0-only**. Keep the [LICENSE](LICENSE) text intact.
The assignment terms are only in the
[Contributor Agreement](CONTRIBUTOR_LICENSE_AGREEMENT.md). Read that file
before a pull request. This section does not restate them.

For each pull request, describe the change and the checks, then check the
single Contributor Agreement checkbox yourself. Keep its wording unchanged.
A release note may be in the contributor's own language and may include a
screenshot. If the change is entirely yours and names no other rights holder
or excluded material, no extra form is required.

When it applies, name co-authors, employer ownership, and included third-party
material with its source and license. Get consent from each relevant rights
holder. Do not put private identity or employer documents in the pull request.

The checkbox links to the agreement on `main`. Do not add a version or pin a
release branch. A pull request does not change the accepted agreement until
that text is on `main`. If the text on `main` changes, read it again and give
fresh consent before acceptance. To withdraw consent before acceptance, say so
in the pull request and clear the checkbox.

`Release context` checks the template and the checkbox. It does not verify
identity, employer authority, or co-author consent. The maintainer reviews
those records before acceptance.

Only `dependabot[bot]` pull requests that change manifests, lockfiles, or
Actions workflow files skip the human template. That exemption does not assign
dependency ownership or cover human work. Other bot pull requests are not
exempt.

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

## Development setup

Clone this repository. The commands below are the same on Windows, macOS, and
Linux. Install Bun, Rust, and the native tools listed below before running
`bun run setup`; it installs the locked project dependencies:

- [Bun](https://bun.sh) 1.4.2 or newer.
- [rustup](https://rustup.rs). The committed `rust-toolchain.toml` selects the
  Rust version.
- Native libraries for the desktop shell:
  - Windows: MSVC, Windows SDK, CMake, Ninja, and the WebView2 runtime.
    Relay supports a portable bundle and a regular Visual Studio Build Tools
    installation when no portable bundle is present.
  - macOS: Xcode Command Line Tools.
  - Linux: GTK 3, WebKitGTK 4.1, librsvg, and a secret service such as
    gnome-keyring. The Build workflow lists the packages CI installs.

Then install the locked frontend packages and fetch the Rust crates:

~~~sh
bun scripts/setup/start-dev.mjs
~~~

`bun run start` installs the locked frontend and Rust dependencies, then starts
the desktop app. The setup, start, and `app:*` commands work from the repository
root and from `src`. Use
`bun run setup` when you only want to prepare the checkout.
On Windows, Relay's scripts prefer the portable toolchain in
`%USERPROFILE%\Development\visual-studio\build-tools` and initialize MSVC,
the Windows SDK, CMake, and MSBuild together. Keep the SDK in
`%USERPROFILE%\Development\windows-sdk` or set `ZENITH_WINDOWS_SDK_ROOT` to
its portable location. When portable MSVC is present, the scripts do not fall
back to a system Windows SDK. Set `ZENITH_MSVC_ROOT` when the portable MSVC
bundle is stored elsewhere. Rust checks should use the repository wrapper so
the same environment is applied:

~~~powershell
bun scripts/build/cargo.mjs test --manifest-path crates/relay-core/Cargo.toml --locked
~~~

When cloned inside the Zenith workspace, setup can call the sibling
`scripts/setup/setup-development.ps1` to install missing portable Windows tools
under `Development`. This installer is not included in a standalone Relay
clone. Standalone contributors must prepare native tools first. The workspace
installer requires `winget` and an elevated PowerShell window;
`-InstallBuildTools` forces a repair. For a direct `cargo` invocation from the
workspace, load the same portable environment first:

~~~powershell
. ..\scripts\setup\use-development-env.ps1
~~~

Playwright browsers are optional and kept out of the default setup:

~~~sh
bun run setup:browsers
~~~

## Documentation policy

The tracked human-facing documentation is deliberately small:

~~~text
README.md
CONTRIBUTING.md
docs/project/PLANNING.md
docs/project/ROADMAP.md
docs/releases/CHANGELOG.md
docs/help/<locale>/README.md
docs/screenshots/*.png
~~~

<code>AGENTS.md</code> is repository guidance. <code>LICENSE</code> and
<code>CONTRIBUTOR_LICENSE_AGREEMENT.md</code> are legal documents, and
<code>relay-server/openapi.yaml</code> is the machine-readable server contract.
Do not add parallel architecture, design, handoff, or historical planning
documents. Put implemented contracts in
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

These two checks are the same ones CI runs. They need Bun, not PowerShell.
Use the branch targeted by the pull request as the base reference:

~~~sh
git fetch origin
BASE_REF=origin/main
bun ./scripts/check/check-agent-guardrails.mjs "$BASE_REF"
bun ./scripts/check/check-duplicate-code.mjs "$BASE_REF"
~~~

Replace `origin/main` with the target branch for a release or development pull
request. From `src`, the same commands are
`bun run check:guardrails -- "$BASE_REF"` and
`bun run check:duplicates -- "$BASE_REF"`.

For PR template or metadata-workflow changes, run the isolated policy tests:

~~~powershell
bun test ./scripts/check/pr-metadata.test.mjs
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
bunx playwright test tests/e2e/visual-matrix
bunx playwright test tests/e2e/operations
bun run screenshots
~~~

The visual matrix is the responsive/appearance gate. The operations folder is
the interaction and state-transition gate. The screenshots command regenerates
only the committed `docs/screenshots` assets.

<code>bun run verify</code> runs the unit tests, frontend build (including the
TypeScript build), and desktop Rust tests. Packaging or updater changes also require:

~~~powershell
bun run app:build
~~~

Run it from the repository root or from `src`. To build only the executable,
without an installer:

~~~powershell
bun run app:build --no-bundle
~~~

On Windows, the executable is written to
`src-tauri/target/release/zenith-relay.exe`. Building does not replace or
restart an installed Relay.

### Shared runtime

~~~powershell
bun scripts/build/cargo.mjs fmt --manifest-path crates/relay-core/Cargo.toml --all -- --check
bun scripts/build/cargo.mjs check --manifest-path crates/relay-core/Cargo.toml --all-targets --locked
bun scripts/build/cargo.mjs clippy --manifest-path crates/relay-core/Cargo.toml --all-targets --locked -- -D warnings
bun scripts/build/cargo.mjs test --manifest-path crates/relay-core/Cargo.toml --locked
~~~

### User-managed server

~~~powershell
bun scripts/build/cargo.mjs fmt --manifest-path relay-server/Cargo.toml --all -- --check
bun scripts/build/cargo.mjs check --manifest-path relay-server/Cargo.toml --all-targets --locked
bun scripts/build/cargo.mjs clippy --manifest-path relay-server/Cargo.toml --all-targets --locked -- -D warnings
bun scripts/build/cargo.mjs test --manifest-path relay-server/Cargo.toml --locked
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

### Continuous integration

Pull requests and pushes to <code>main</code> run checks only. They do not build
installers or publish artifacts. Pushing a <code>v*</code> tag builds the desktop
installers and, after those checks pass, publishes the GitHub Release, updater
manifest, and server image. A manual workflow run can build and smoke-test the
same artifacts, but it does not publish, even when started from a tag.

### macOS distribution

macOS builds use Tauri's ad-hoc identity (`APPLE_SIGNING_IDENTITY=-`); they do
not need an Apple Developer Program membership or Apple secrets. The release
build checks the app inside the DMG and the updater archive for a valid ad-hoc
signature before publishing. Release assets include SHA-256 checksums. Ad-hoc
signing does not establish a trusted developer identity and is not Apple
notarization, so downloaded apps still require the one-time macOS approval
described in the localized Help. Test fresh installation and in-app
updating on separate Intel and Apple Silicon Macs before claiming that those
flows work end to end. Never remove quarantine from the whole system or all
downloads. The `TAURI_SIGNING_PRIVATE_KEY` secret remains required for signed
in-app updates; it is separate from macOS code signing.

### GitHub merge requirements

Maintainers should configure branch protection or rulesets for the branches
that accept changes. Require pull requests, the <code>Release context</code>
status check, an up-to-date branch, and code-owner review for the paths in
<code>.github/CODEOWNERS</code>. Restrict bypass permissions to the intended
maintainers. These files alone do not enable branch protection. A PR author
cannot approve their own PR as a code owner; owner-authored policy changes need
another authorized reviewer or an explicitly managed maintainer exception.

GitHub loads the <code>pull_request_target</code> workflow from the repository's
default branch; this workflow explicitly loads the validator from the PR's base
commit. Bootstrap the workflow in the default branch and the agreement,
template, validator, and tests in each target branch through maintainer review
before making the check required. If an Actions event policy blocks
<code>pull_request_target</code>, allow this reviewed metadata workflow there
before relying on the check. Do not enable unreviewed workflows or broaden token
permissions as a workaround.

A policy change must not use its own proposed wording to authorize itself.
The template links to the agreement on `main`, with no version and no
release-branch pin. Changing the link or the agreement requires the template,
validator, and tests to change together. Open pull requests need fresh consent
after the new text is on `main`.

### Updater changelog for release admins

The updater changelog is read from the GitHub Release body. In the published
Release, put each translation after a <code>relay-notes:&lt;locale&gt;</code>
marker. A section continues until the next marker. A pull request note can stay
in the contributor's own language and can include a screenshot; add the locale
markers when publishing the Release, not in the pull request.

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
