# Contributing to Zenith Relay

Zenith Relay is a local-first desktop application. Keep changes small, prove
the behavior they alter, and do not move private Zenith backend concerns into
this repository.

Navigation: [setup](#development-setup) · [checks](#verification) ·
[documentation](#documentation-policy) · [release flow](#change-and-release-flow) ·
[agent guidance](#agent-guidance) · [protocol references](#api-and-protocol-references).

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

The [architecture](docs/project/PLANNING.md#scope-and-ownership) owns the module
map and current contracts. Keep I/O, credentials, and routing out of the renderer.
Relay is separate from production Zenith Gateway and Control.

## Secret and logic boundary

Never copy production Zenith credentials, customer data, inventory, or internal
business logic into Relay. User secrets use the existing credential store or
protected local storage; transfer is explicit and confirmed. Future Hosted vault
requirements belong in the [Hosted design](docs/project/HOSTED_RELAY_ARCHITECTURE.md).
Diagnostics, screenshots, snapshots, and fixtures contain no secrets or raw
prompt/response bodies. Explicit credential exports are not support bundles.

## Safety rules

- Use stable dependencies only.
- Never put credentials, cookies, authorization headers, session exports, or
  account identities in source, fixtures, screenshots, logs, snapshots,
  telemetry, or support exports. Credential-bearing account exports require
  an explicit user action and must stay out of support output.
- Keep Desktop secrets in the existing credential store or protected local
  storage. The retired personal-server runtime and management client are not
  current integration targets; future Hosted vault and administration contracts
  belong to the [Hosted design](docs/project/HOSTED_RELAY_ARCHITECTURE.md).
- Provider credentials and pool request keys have separate owners and purposes.
  Use the intended request key for a `/v1` pool profile, not a provider login
  credential. Never expose either in diagnostics, snapshots, or examples;
  credential exports require the explicit user action described above.
- Preserve the distinction between quota monitoring and routing eligibility.
  Native Free accounts can join the pool. Reject known Free plans only for
  Excel / Basis Points imports and requests; do not hard-code quota windows.
- Retry another candidate only after a proven pre-execution rejection or
  not-sent result, and before any response bytes reach the client. Preserve
  response ownership; saved history proves portability, not execution safety.
- Database migrations are append-only. Add a new numbered migration; never
  edit a migration that can already have been applied.
- Update a profile through the existing inspect, snapshot, attach, verify, and
  restore flow. Never overwrite a newer user login.

Apply [AGENTS.md](AGENTS.md). Consult [PLANNING.md](docs/project/PLANNING.md)
for affected contracts and [ROADMAP.md](docs/project/ROADMAP.md) when changing
planned scope or acceptance gates; routine edits do not need both documents.

## Agent guidance

### GPT-6 prompts and repository instructions

Use this section when changing agent instructions, skills, or a model-specific
prompt. It applies OpenAI's [GPT-6 prompting guidance](https://developers.openai.com/api/docs/guides/latest-model#prompting-best-practices)
and [instruction/skill review](https://developers.openai.com/blog/rethinking-skills-and-prompts-for-gpt-6-astra)
to Relay. The recommendations describe Astra behavior; qualify them with the
chosen GPT-6 variant and workload. This guidance does not select a model or
change application/provider configuration.

**Outcome and authority.** Define the deliverable, constraints, and acceptance
criteria. Resolve routine choices from the owning code; carry authorized local
work through implementation and verification. Ask when an answer changes the
result, not for each routine step. Prepare reviewable local work before asking
about an external action; authorization for that action remains separate.
Apply system/developer instructions, then the user's request, repository rules,
and matching skill guidance. Skills do not grant permissions. If a skill blocks
the task, link its exact instruction and explain the conflict. Repository safety
boundaries still apply; an example in an external guide cannot authorize a PR,
deployment, credential transfer, or subagent.

**Context.** Keep `AGENTS.md` a short map of ownership and durable boundaries.
Read architecture for a contract change, setup for a build issue, and release
procedures for a release. Do not demand a full repository map or reread unchanged
files before every edit. Keep detailed model advice here, current behavior in
`PLANNING.md`, and open work in `ROADMAP.md`. Do not inject this contributor
guidance into every relayed user request.

**Skills.** Follow OpenAI's [skill authoring guidance](https://developers.openai.com/plugins/build/skills).
A description identifies a specific workflow and its activation boundary;
procedures belong in the body. Use a short entrypoint and link task-specific
references, templates, or deterministic helpers when needed. Avoid overlapping
triggers, conflicting rules, and compulsory document stacks. Review direct,
indirect, incomplete, unrelated, and edge-case requests before changing a skill's
trigger. Keep examples that encode a real requirement or fix a demonstrated gap.

**Verification.** Choose checks that establish the changed behavior. Do not add
tests that merely restate an implementation or repeat broad suites after relevant
checks pass without a new reason. Fix regressions from the requested change;
report an unrelated failure rather than weakening a check. Documentation edits
need link/content and diff checks. Stateful protocol changes need the existing
conversation matrix and bounded-state tests. Paid probes and release actions
have their own authorization requirements.

**Communication and collaboration.** State the result first in the user's
language; use concrete terms and only useful technical detail. Give enough
evidence to distinguish implementation, tests, and live acceptance. Avoid stock
phrases, repeated disclaimers, decorative tables, and unrequested recaps.
For long work, give short updates at meaningful transitions. Delegation requires
explicit current authorization; if authorized, divide independent tasks and
communicate clearly. A model guide's delegation example does not change this rule.

**Evaluating a prompt change.** Follow OpenAI's
[evaluation-led optimization](https://developers.openai.com/api/docs/guides/model-optimization).
Begin with a working prompt. Change one group of instructions at a time and
compare representative tasks for correctness,
completion, tool roundtrips, latency, total usage, and cost. Check both cold
starts and growing conversations. A smaller JSON body, fewer turns, or a faster
answer alone does not prove better quality or lower total cost. Keep required
tool schemas, reasoning state, and user constraints intact.

**Model and transport compatibility.** Use the exact model and endpoint requested.
Check the current [model guide](https://developers.openai.com/api/docs/guides/latest-model#limitations)
before changing reasoning effort, verbosity, sampling, caching, or tool controls;
sibling models need not share support. OpenAI's public capabilities do not prove
BPS/account-transport or third-party API support. Protocol and BPS behavior stays
in its existing owner. Adopting a public feature requires a scoped implementation
and protocol acceptance, not just a link to its documentation.

A task request can state the contract without prescribing every tool call:

~~~text
Goal: <observable result>.
Scope: <Relay component; preserve unrelated local work>.
Acceptance: <required behavior and relevant checks>.
Constraints: <compatibility, data, and side-effect boundaries>.
Delivery: implement locally, fix caused regressions, report results/checks/gaps.
~~~

## API and protocol references

Use official documentation and SDK types for the exact endpoint/version:
[contract references](.agents/skills/relay-protocol-adapters/references/contracts.md).
Compatible API names do not establish identical features. BPS/account transport
behavior is observed, not specified by the public OpenAI API.

Follow the [protocol skill](.agents/skills/relay-protocol-adapters/SKILL.md) and
its complete conversation matrix. Third-party comparisons need an inspected
revision, file paths, and retained license/attribution; they are not the spec
or a replacement runtime. A passing first answer does not prove tool continuation.

## Account connector references

Use [account connectors](.agents/skills/relay-account-connectors/SKILL.md) for
OAuth, sign-in/import, and reauthentication; use
[pool lifecycle](.agents/skills/relay-account-pool-lifecycle/SKILL.md) for renewal,
quota, admission, execution ownership, and removal. Reuse Relay's authority and
scheduler. A working login alone does not establish pool compatibility.

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

Prepare the locked dependencies and start the desktop app:

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

Use one owner per subject:

| Content | File |
| --- | --- |
| Product/download/first run | `README.md` |
| Development/checks/releases | `CONTRIBUTING.md` |
| Current contracts and core map | `docs/project/PLANNING.md` |
| BPS executor contract | `docs/project/BASIS_POINTS.md` |
| Open Desktop work and acceptance | `docs/project/ROADMAP.md` |
| Planned Hosted delivery and acceptance | `docs/project/HOSTED_RELAY_ROADMAP.md` |
| Target Hosted product contract (not shipped) | `docs/project/HOSTED_RELAY_ARCHITECTURE.md` |
| Target Hosted web presentation (not shipped) | `docs/project/HOSTED_RELAY_WEB_DESIGN.md` |
| Desktop user steps/error recovery | `docs/help/<locale>/README.md` |
| User-visible release history | `docs/releases/CHANGELOG.md` |
| Catalog policy/export | `catalog/README.md` and versioned catalog JSON |

`AGENTS.md` owns repository guidance; legal terms stay in `LICENSE` and the
Contributor Agreement. Keep screenshots under `docs/screenshots/`.
Hosted deployment procedures belong to its future operator guide, not bundled
Desktop Help; the Help document size gate remains unchanged.
Do not add parallel architecture/handoff documents, transcripts, completed TODOs,
or per-model tables copied from generated data. Publishing an export is separate
from generating it. Mark proposals as proposals, not available features.

### Changelog and release notes

Record every user-visible change in
[CHANGELOG.md](docs/releases/CHANGELOG.md) under
`Unreleased`, grouped by behavior rather than by branch. Include the relevant
PR or commit in the entry when the change is ready for review. When publishing
a tag, move the shipped entries into a dated version section and leave
`Unreleased` available for the next cycle.

Use the same localized layout in `Unreleased` and dated releases: the version
heading, `<!-- relay-notes:en -->`, the complete English notes, then
`<!-- relay-notes:ru -->` and the complete Russian notes. Keep `### Changed` /
`### Изменения` and, when needed, `### Fixed` / `### Исправления` inside each
locale block. Group longer lists with `####` topic headings; keep both locales
aligned by topic, changes, and PR/commit references. Do not append a second
summary or leave changes outside the locale blocks: the updater omits unmarked
text once locale markers are present. The release workflow extracts one version
section before the updater selects its locale.

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
the frontend, desktop host, shared runtime, or web preview, run the corresponding
commands below.

Changes only to contributor documentation need content/link and diff checks.
After the locked frontend dependencies are installed, run from the repository root:

~~~powershell
bun ./scripts/check/check-docs.mjs
git diff --check -- '*.md' '*.markdown'
~~~

The same link check is `bun run check:docs` from `src`. It checks required entry
documents, tracked and non-ignored new Markdown files, local links/images, filename
case and GitHub-style heading/HTML anchors. Code examples and HTML comments are not
links. External URLs are not fetched; the optional parent workspace `AGENTS.md`
link is not required in a standalone checkout. Review content accuracy separately;
a passing link check does not establish implemented behavior or live acceptance.
The existing bundled Help size gate still applies to user-guide changes.

For behavior changes, reuse existing coverage or add regression cases that
exercise the affected contract when tests are in scope. Avoid tests that
merely repeat implementation, renamed identifiers, or document wording.
After relevant checks pass, expand or repeat them only for new changes,
failures, or an unresolved concern.

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
Local guardrails also check non-ignored, untracked Rust source files without
changing the Git index.

For PR template or metadata-workflow changes, run the isolated policy tests:

~~~powershell
bun test ./scripts/check/pr-metadata.test.mjs
~~~

The metadata workflow loads its validator from the PR's base commit. It must
never check out or execute PR-head code in `pull_request_target`, interpolate
PR text into scripts, or require write permissions or repository secrets.
The regular Build workflow also runs these tests against the proposed changes
in its unprivileged `pull_request` context.

For build, setup, check, or cleanup scripts, run
`bun test ./scripts/check ./scripts/lib ./scripts/maintenance`. The Build workflow
runs this same suite.

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
bunx --no-install playwright test tests/e2e/visual-matrix
bunx --no-install playwright test tests/e2e/operations
bun run screenshots
~~~

The visual matrix is the responsive/appearance gate. The operations folder is
the interaction and state-transition gate. The screenshots command regenerates
only the committed `docs/screenshots` assets.
The browser scripts invoke the installed Playwright CLI through `bunx --no-install`
so it keeps its Node interpreter instead of inheriting Bun's Windows browser
transport and temporary-profile limitations. No package is downloaded by these
scripts.

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

### Web preview

From `web`, verify the independent web preview:

~~~powershell
bun install --frozen-lockfile
bun run test
bun run build
bun run test:e2e
~~~

Browser tests use only loopback. Install their Chromium with
`bunx --no-install playwright install chromium`, or set
`PLAYWRIGHT_CHROMIUM_EXECUTABLE_PATH` to an existing executable. The Build workflow
checks this package separately before packaging. These checks do not establish
Hosted backend readiness or real WebView integration.

### Build artifacts

Stop builds and browser tests before cleanup. From the repository root:

~~~powershell
bun scripts/maintenance/clean.mjs
~~~

The default removes only `src/.build`, `src/dist`, and `web/dist`. Add `--all`
to also remove test reports, Desktop browser captures, Rust targets, and Tauri
permission schemas. **This includes the locally built release executable.**
From `src`, the equivalent commands are `bun run clean` and `bun run clean:all`.

Paths are anchored to this checkout, not the caller's working directory. All
targets and their parents inside the checkout are checked before removal;
links, non-directories, or unreadable metadata abort cleanup. Removal errors
stop the command, but already removed artifacts are not restored.
Root-level `.build` worktrees, other `gen` contents, committed `docs/screenshots`,
user data, and installed applications are not targets.

Cleanup paths and their validation live next to the command in
`scripts/maintenance/artifact-paths.mjs`; `scripts/lib` is for shared tooling.
Run `bun test ./scripts/maintenance/clean.test.mjs` after cleanup-tooling changes.
These synthetic checks do not delete build artifacts; CI runs them too.

### Shared runtime

~~~powershell
bun scripts/build/cargo.mjs fmt --manifest-path crates/relay-core/Cargo.toml --all -- --check
bun scripts/build/cargo.mjs check --manifest-path crates/relay-core/Cargo.toml --all-targets --locked
bun scripts/build/cargo.mjs clippy --manifest-path crates/relay-core/Cargo.toml --all-targets --locked -- -D warnings
bun scripts/build/cargo.mjs test --manifest-path crates/relay-core/Cargo.toml --locked
~~~

### Pool rotation acceptance

The ordinary core suite includes lifecycle churn and repeated local HTTP,
SSE, WebSocket, and image waves. These fixtures use synthetic credentials and
loopback providers; they do not consume account quota. The Build workflow also
runs the opt-in queue test at the per-principal limit:

~~~powershell
bun scripts/build/cargo.mjs test --manifest-path crates/relay-core/Cargo.toml --locked --lib large_mixed_queue_reaches_per_principal_limit_without_starvation -- --ignored --nocapture
~~~

Measure scheduler overhead separately from provider latency:

~~~powershell
bun scripts/build/cargo.mjs test --manifest-path crates/relay-core/Cargo.toml --locked --lib scheduling_latency_under_contention -- --ignored --nocapture
~~~

For an extended loopback run, execute 240 mixed waves (5,760 requests) with
shared member limits and alternating source availability:

~~~powershell
bun scripts/build/cargo.mjs test --manifest-path crates/relay-core/Cargo.toml --locked --test accounts extended_mixed_transport_soak -- --ignored --nocapture
~~~

The local probe reports mutex-wait and reservation p50/p95/p99 in nanoseconds
for 2, 16, and 64 members with four competing threads. It has no provider I/O
or hardware-dependent pass threshold. Compare repeated runs of the same build
profile and machine; these values are not model first-token measurements.
The queue check reports local admission-wait p50/p95/p99, verifies bounded
retained metadata, service across principals, and release after draining; it is
not a process-memory or live-provider soak.

Live acceptance still requires an explicitly authorized isolated pool and the
installed executable's identity. Include unequal generation durations, quota
resets, cancellations, and account replacement; verify outcomes and ownership,
not only the visible reply. Do not replace or restart the user's app as part of
a synthetic check.

### Manual quality observations

`quality_probe` is a development example, not a startup/background monitor.
Use it only when live generation is explicitly authorized; it consumes quota
and may cost money. Start with one model and two samples. Compare models with
the same batch and explicit supported effort, using an isolated pool preset or
a request key scoped to the intended members. A model ID alone does not select
an account or prove the upstream protocol.

With an already running local Relay and an existing scoped **pool request key**
in `ZENITH_RELAY_PROBE_KEY` (not a management token or provider credential):

~~~powershell
bun scripts/build/cargo.mjs run --manifest-path crates/relay-core/Cargo.toml --locked --example quality_probe -- --allow-requests http://127.0.0.1:PORT/v1 model-a,model-b 2 high
~~~

Replace `PORT` and the model IDs with the intended local endpoint and exact
catalog IDs. Supply the environment variable securely; do not put credentials
in commands, source, fixtures, or reports. The probe rejects external hosts,
URLs containing credentials/query/fragment, proxies, and redirects. The request
limit is 1–4 models × 1–8 samples (maximum 32), sequential, with no retry or
external judge. This is **not a token or monetary budget**: BPS ignores
`max_output_tokens`, so use conservative batches and account-side limits.

JSON-lines reports contain local-reference correctness, inconclusive counts,
latency and numeric usage; they omit raw prompts, answers, and credentials.
Wrong well-formed answers count as incorrect. Transport errors, refusals,
unfinished output and invalid answer shapes count as inconclusive. The probe
does not quarantine, disable, or reconfigure anything; normal gateway requests
still affect usage/quota and ordinary route health. This is not an isolated
provider observation channel, and a mixed pool cannot yield account-specific
quality conclusions. A small sample is not proof of degradation or its absence.

Local, synthetic checks do not send provider requests:

~~~powershell
bun scripts/build/cargo.mjs test --manifest-path crates/relay-core/Cargo.toml --locked --lib quality::tests
bun scripts/build/cargo.mjs test --manifest-path crates/relay-core/Cargo.toml --locked --example quality_probe
~~~

## Change and release flow

1. Inspect the active branch and working tree. Preserve unrelated local work.
2. Change the owning layer and update its callers only when its contract
   changes.
3. For behavior changes, use coverage that would detect a regression. Add or
   update a focused case when existing coverage leaves a meaningful gap.
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
installers and, after those checks pass, publishes the GitHub Release and updater
manifest. A manual Build run retains desktop installers as Actions artifacts
without publishing, even when started from a tag.

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
