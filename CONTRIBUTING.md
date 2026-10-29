# Contributing to Zenith Relay

Zenith Relay is a local-first desktop application. Keep changes small, prove
the behavior they alter, and do not move private Zenith backend concerns into
this repository.

Navigation: [setup](#development-setup) · [checks](#verification) ·
[documentation](#documentation-policy) · [release flow](#change-and-release-flow) ·
[agent guidance](#agent-guidance) · [protocol references](#api-and-protocol-references).

For suspected vulnerabilities, follow the
[private security-reporting process](.github/SECURITY.md), not a public issue.

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
requirements belong in the [Hosted design](docs/hosted/HOSTED_RELAY_ARCHITECTURE.md).
Keep credentials, cookies, authorization headers, sessions, account identities and
raw prompt/response bodies out of source, fixtures, diagnostics, logs, telemetry,
screenshots and support bundles. Credential exports require explicit user action.

## Safety rules

- Use stable dependencies only.
- Provider credentials and pool request keys have separate owners and purposes.
  Use the intended request key for a `/v1` pool profile, not a provider login
  credential. The retired personal-server runtime is not an integration target.
- Preserve the distinction between quota monitoring and routing eligibility.
  Native Free accounts can join the pool. Do not reject Excel / Basis Points imports
  or requests by plan name; execution requires fresh provider access evidence.
  Do not hard-code quota windows.
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

Keep shared rules in [AGENTS.md](AGENTS.md), specialized workflows in
`.agents/skills`, and product contracts in their owning documents. Skills should
name the operations they cover and load supporting references only as needed;
do not copy contributor guidance into relayed user requests.

In a workspace checkout, the parent `CONTRIBUTING.md#agent-guidance` and
`.agents/skill-selection.cases.json` cover authoring and selection checks.
These are optional for a standalone checkout. Structural checks do not establish
model-selection quality or authorize live probes, releases, or subagents.

## API and protocol references

Use official documentation and SDK types for the exact endpoint/version:
[contract references](.agents/skills/relay-protocol-adapters/references/contracts.md).
Compatible API names do not establish identical features. BPS/account transport
behavior is observed, not specified by the public OpenAI API.

Follow the [protocol skill](.agents/skills/relay-protocol-adapters/SKILL.md) and
the matrix for the affected protocol pair and contract. Third-party comparisons
need an inspected revision, file paths, and retained license/attribution; they are
not the spec or a replacement runtime. A passing first answer does not prove tool
continuation.

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
  Rust 1.99.1 toolchain; both Rust packages use Edition 2024.
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

The repository Cargo configuration enables reqwest's experimental HTTP/3 API
on stable Rust. Custom `RUSTFLAGS` override Cargo configuration, so retain
`--cfg reqwest_unstable` when supplying your own flags, as CI does. Routed direct
HTTPS requests try HTTP/3 only after a background transport probe succeeds;
configured proxies and WebSockets retain their existing transport. Set
`ZENITH_RELAY_HTTP3=0` before starting Relay to disable HTTP/3 for comparison.
Request diagnostics expose the actual upstream HTTP version.

For a prepared checkout, use `bun run app:dev`. Rust auto-restart is disabled
by default, so editing backend code does not terminate pool requests made by
the agent doing the work. Frontend hot reload remains enabled. Rust changes
take effect only after you explicitly stop and restart the dev command;
finish active agent requests before restarting. This is not data or process
isolation from your working Relay profile.

Codex's **Relay · Dev** action uses the same `bun run app:dev` workflow and
native default profile as ordinary terminal development. It does not create a
separate profile or copy accounts. An explicit absolute `ZENITH_RELAY_DEV_DATA_DIR`
can still select another debug profile when deliberately configured.

To opt into automatic Rust rebuilds/restarts, use `bun run app:dev:watch`
or `bun run start --watch`. Do not route the editing agent through that
watch-mode instance: use a separate stable Relay for the agent instead.

Codex desktop actions are provided in `.codex/environments/environment.toml`.
Use **Relay · Dev** from the Run menu to start the normal no-watch debug workflow in
Codex's terminal. After an agent task finishes, stop that terminal command and
run the action again to compile and apply Rust edits. Frontend hot reload stays
enabled. The action list contains only Dev, TypeScript, UI tests, and Rust check;
opening a chat does not run them automatically. Documentation checks and dependency
setup remain available through the commands documented here, outside the action list.

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

### Build the desktop executable

From the repository root or `src`:

~~~sh
bun run app:build --no-bundle
~~~

The wrapper selects the repository's pinned Rust toolchain and native compiler
environment, then runs TypeScript, Vite, the frontend size gate and Cargo with
`--locked`. The result is the optimized executable and its SHA-256 stamp;
the running app stays untouched. Omit `--no-bundle` to build installers too.
Keep Tauri options before `--` and extra Cargo arguments after it.

Identical renderer filenames and bytes retain their previous timestamps, allowing
Cargo to reuse its release output. Changed assets still trigger native work.
Every build runs frontend validation; release LTO, codegen units and the ES2020
WebView target stay unchanged. Gzip measures frontend size, not compiler speed.

For the Windows workspace's build-and-restart shortcut, use the parent
`CONTRIBUTING.md` section **Rebuild the local Relay desktop executable**.

## Documentation policy

Use one owner per subject. The [documentation index](docs/README.md#find-the-right-document)
lists the owning guides; its [repository tree](docs/README.md#repository-tree)
separates source directories from generated local artifacts.

`AGENTS.md` owns repository guidance; legal terms stay in `LICENSE` and the
Contributor Agreement. Keep screenshots under `docs/screenshots/`.
Hosted deployment procedures belong to its future operator guide, not bundled
Desktop Help; the Help document size gate remains unchanged.
Do not add parallel architecture/handoff documents, transcripts, completed TODOs,
or per-model tables copied from generated data. Publishing an export is separate
from generating it. Mark proposals as proposals, not available features.
Give routine check results in chat; do not add audit, benchmark or status reports
unless explicitly requested. Update an existing guide only for a maintained change.

### Changelog and release notes

Record every user-visible change in
[CHANGELOG.md](docs/releases/CHANGELOG.md) under
`Unreleased`, grouped by behavior. Include the relevant
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
modules. Preserve unknown external provider codes as received.

To add a locale, add one sequential guide at
<code>docs/help/&lt;locale&gt;/README.md</code>, register its translation
resources, and update the bundled Markdown registry in
<code>src/src/features/relay/help/HelpCenter.tsx</code>. Keep the guide
accurate for the UI's current labels and section order.

Screenshots are generated from the mocked desktop shell. Update the scenario
when the UI changes, then regenerate:

~~~powershell
cd src
bun run screenshots
~~~

## Verification

Run the narrowest relevant checks while iterating. Before a commit that changes
the frontend, desktop host, shared runtime, or web preview, run the corresponding
commands below.

### One verification command

After [development setup](#development-setup), install the independent web
preview's locked dependencies once, then run verification from the root:

~~~powershell
bun install --cwd web --frozen-lockfile
bun run verify --quick
bun run verify --base origin/main
~~~

`--quick` runs script tests, documentation checks, agent guardrails,
duplicate-code checks, and unit tests plus production builds for both Desktop
UI and the web preview. It deliberately skips native Rust, browser tests, and
dependency audits; it is not a release gate.

Without `--quick`, verification also runs formatting, Clippy and tests for both
Rust packages, both browser suites, both Bun audits, and audits of both Cargo
lockfiles. Script/documentation/guardrail checks run first. Independent Desktop
UI, web and Rust lanes then run with a maximum of two concurrent commands; each
lane stays sequential. Use `--jobs 1` for sequential execution or `--jobs 3` to
allow all three lanes. After a failure, no new command starts, and already-started
commands finish before verification exits unsuccessfully. Desktop Rust tests keep
one test thread. `bun run verify` from `src` calls the same root runner. Each
command reports elapsed time. Both Rust packages reuse `src-tauri/target` by
default; explicit Cargo target-directory overrides still apply.

Full verification needs the pinned Rust toolchain and native build libraries,
locked dependencies in `src` and `web`, Playwright Chromium, `cargo-audit`, and
network access for vulnerability databases. Run `bun run setup:browsers` to
prepare the browser dependencies. Install the audit tool with
`cargo install cargo-audit --locked` when it is missing. On Linux, Desktop Rust
tests also need a running, unlocked Secret Service, normally provided by the
desktop session; the [Build workflow](.github/workflows/build.yml) shows the
headless setup used in CI.

`--base` selects the existing Git reference used by guardrail and duplicate-code
checks. Use the actual PR target and fetch it first; omit the option to use each
check's normal local default. Verification does not package installers, publish
a release, restart Desktop, or establish installed-app acceptance.

### Required CI and dependency maintenance

The Build workflow checks pull requests and pushes to `main` and `release/**`.
A release-branch push omits its duplicate suite only after confirming an open PR
for that exact commit and a separate, non-cancelled PR Build run. Standalone
release pushes, PRs, `main`, tags and manual runs retain their checks; API lookup
failures or an unconfirmed PR run also retain the complete suite. Windows CI
reports test compilation (`--no-run --timings`) separately from execution.
Its `Check` status succeeds only when runtime checks, every Desktop browser
shard, Windows Rust tests, and web-preview checks all succeed; a skipped or
cancelled dependency is not a pass. The `main` ruleset requires `Check` and
`Release context` on an up-to-date branch. CODEOWNERS identifies reviewers;
it does not itself require an additional reviewer or grant a merge bypass.

Third-party Actions are pinned to full commit IDs. Dependabot maintains those
pins, both Cargo packages, and the actual Bun lockfiles in `src` and `web`.
Runtime CI fetches the current RustSec database once, records its commit, and
uses that same snapshot for both Cargo audits. Both frontend packages run
`bun audit`. A new advisory can fail CI without a source change; investigate
and remediate it rather than pinning an old database to obtain a passing check.

Private vulnerability reporting and CodeQL default setup are repository settings,
not local workflow files. Changes to checked-in workflows and maintenance rules
take effect on GitHub only after they reach the relevant branch. Installer
builds still require a manual run or version-tag push; publication remains
restricted to version-tag pushes.

### Focused checks

Changes only to contributor documentation need content/link and diff checks.
After the locked frontend dependencies are installed, run from the repository root:

~~~powershell
bun ./scripts/check/check-docs.mjs
git diff --check -- '*.md' '*.markdown'
~~~

The equivalent is `bun run check:docs` from `src`. It checks local links, anchors,
filename case and required documents, not external URLs or content accuracy.
The bundled Help size gate also applies to user-guide changes.

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
`bun test ./scripts/check ./scripts/lib ./scripts/maintenance ./scripts/build`. The Build workflow
runs this same suite.

### Frontend and desktop

~~~powershell
cd src
bun run check
bun run test:unit
bun run build
bun run test:e2e
~~~

#### Renderer responsiveness checks

Use the existing suite for the changed surface; do not add a new test file or
benchmark diary for each fix.

| Surface | Suites under `src/tests/e2e` |
| --- | --- |
| Startup and navigation | `operations/startup.spec.ts`, `seamless-navigation.spec.ts` |
| Events and background work | `frontend-activity.spec.ts`, `runtime-refresh.spec.ts`, `interface-workload.spec.ts` |
| Usage | `usage-event-refresh.spec.ts`, `usage-loading.spec.ts`, `usage-performance.spec.ts` |
| Saved views | `previous-snapshots.spec.ts`, `overview-persistence.spec.ts` |
| Network settings | `operations/network-refresh.spec.ts`, `operations/hosted-network.spec.ts` |

Run a selected suite from `src`, for example:

~~~powershell
bunx --no-install playwright test tests/e2e/usage-event-refresh.spec.ts
~~~

Cover pending-read navigation, latest-filter wins, failures, cache invalidation and
saved-view clearing. Use the [Desktop contract](docs/project/PLANNING.md#desktop-workspace)
for behavior; check affected locales, compact layouts and dialog focus.

For native telemetry or snapshot changes, run the owning Rust tests from the root:

~~~powershell
bun scripts/build/cargo.mjs test --manifest-path src-tauri/Cargo.toml --locked local_pool::store::telemetry_db::tests -- --test-threads=1
bun scripts/build/cargo.mjs test --manifest-path src-tauri/Cargo.toml --locked local_pool::commands::state:: -- --test-threads=1
~~~

Serialize Cargo runs. Opt-in `measure_*` tests live next to their implementation;
run them with `--ignored --nocapture` on the same fixture and build profile for a
before/after comparison. Keep measured results in review evidence, not this guide.
Mocked IPC counts and local CPU probes do not establish installed-app latency,
memory consumption or provider speed. Builds do not replace or restart Desktop.
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

The root verification command includes these unit, build and browser checks
alongside native tests. Packaging or updater changes also require:

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

Run the affected core contracts, including stream/terminal handling, usage,
continuations and lease isolation. Contract details stay in
[PLANNING.md](docs/project/PLANNING.md) and [BPS](docs/project/BASIS_POINTS.md).

~~~powershell
bun scripts/build/cargo.mjs fmt --manifest-path crates/relay-core/Cargo.toml --all -- --check
bun scripts/build/cargo.mjs check --manifest-path crates/relay-core/Cargo.toml --all-targets --locked
bun scripts/build/cargo.mjs clippy --manifest-path crates/relay-core/Cargo.toml --all-targets --locked -- -D warnings
bun scripts/build/cargo.mjs test --manifest-path crates/relay-core/Cargo.toml --locked
~~~

### Protocol optimization probes

These opt-in probes use synthetic inputs and loopback, not provider quota. Compare
the same machine, build profile and fixtures before/after; preserve unrelated
dirty work. Report results in chat or review, not this guide. Local CPU, bytes and
mock timings do not establish installed-app memory, provider TTFT, cache hits or
billing savings. Keep ordinary correctness/continuation tests passing.

#### Request compression probe

Compare identity/Zstd transfer and copied/shared request bodies:

~~~powershell
bun scripts/build/cargo.mjs test --manifest-path crates/relay-core/Cargo.toml --target-dir src-tauri/target --locked --release --lib payload:: -- --include-ignored --nocapture --test-threads=1
~~~

Fixtures and sampling live with the tests. Windows timer-floor differences are
inconclusive. Do not change the 64 KiB–8 MiB gate, two-job limit or native-Codex
allowlist based only on a mock link; BPS compression needs separate evidence.

#### Schema and cache probes

Schema conversion, with exact-output equivalence:

~~~powershell
bun scripts/build/cargo.mjs test --manifest-path crates/relay-core/Cargo.toml --target-dir src-tauri/target --locked --release --lib measure_canonical_gemini_schema_conversion -- --ignored --nocapture --test-threads=1
~~~

Historical catalog preparation:

~~~powershell
bun scripts/build/cargo.mjs test --manifest-path crates/relay-core/Cargo.toml --locked --lib measure_catalog_history_preparation -- --ignored --nocapture --test-threads=1
bun scripts/build/cargo.mjs test --manifest-path crates/relay-core/Cargo.toml --locked --lib measure_bps_large_history_preparation -- --ignored --nocapture --test-threads=1
~~~

Prepared BPS prefix reuse and opt-in debug fingerprinting:

~~~powershell
bun scripts/build/cargo.mjs test --manifest-path crates/relay-core/Cargo.toml --target-dir src-tauri/target --locked --release --lib measure_bps_turn_prefix_reuse -- --ignored --nocapture --test-threads=1
bun scripts/build/cargo.mjs test --manifest-path crates/relay-core/Cargo.toml --target-dir src-tauri/target --locked --release --lib measure_cache_context_fingerprinting -- --ignored --nocapture --test-threads=1
~~~

Preserve tool order, original schemas, malformed-declaration rejection and
current-call permissions. Debug fingerprint timings do not describe debug-off work.

#### BPS stream and batch probes

Measure parallel stream normalization and passthrough output during batch expansion:

~~~powershell
bun scripts/build/cargo.mjs test --manifest-path crates/relay-core/Cargo.toml --locked --lib -- parallel_stream_normalization_latency measure_bps_batch_passthrough --ignored --nocapture --test-threads=1
~~~

Preserve native call identity, output order, atomic batch validation and terminal
barriers. These synthetic CPU probes do not measure provider generation or caching.

### BPS attachment preparation probe

Compare in-place image resolution with the reference JSON-roundtrip path:

~~~powershell
bun scripts/build/cargo.mjs test --manifest-path crates/relay-core/Cargo.toml --locked --lib measure_bps_attachment_json_roundtrips -- --ignored --nocapture --test-threads=1
~~~

Preserve exact serialized output and discovery continuation state. JSON/SSE,
catalog continuation and a warm request must reuse the same uploaded file reference.
This measures preparation, not real upload or provider generation.

### Pool rotation acceptance

The core suite covers lifecycle churn and mixed loopback transports. CI also
runs the opt-in per-principal queue-limit test:

~~~powershell
bun scripts/build/cargo.mjs test --manifest-path crates/relay-core/Cargo.toml --locked --lib large_mixed_queue_reaches_per_principal_limit_without_starvation -- --ignored --nocapture
~~~

Measure scheduler overhead separately from provider latency:

~~~powershell
bun scripts/build/cargo.mjs test --manifest-path crates/relay-core/Cargo.toml --locked --lib scheduling_latency_under_contention -- --ignored --nocapture
~~~

For an extended mixed-transport loopback run:

~~~powershell
bun scripts/build/cargo.mjs test --manifest-path crates/relay-core/Cargo.toml --locked --test accounts extended_mixed_transport_soak -- --ignored --nocapture
~~~

Check bounded metadata, fairness across principals and release after draining.
Scheduler timings have no hardware-dependent pass threshold or provider I/O.

Live acceptance still requires an explicitly authorized isolated pool and the
installed executable's identity. Include unequal generation durations, quota
resets, cancellations, and account replacement; verify outcomes and ownership,
not only the visible reply. Do not replace or restart the user's app as part of
a synthetic check.

#### Cache-affinity admission comparison

Run the synthetic admission fixture with:

~~~powershell
bun scripts/build/cargo.mjs test --manifest-path crates/relay-core/Cargo.toml --target-dir src-tauri/target --locked --release --lib measure_cache_affinity_under_parallel_load -- --ignored --nocapture --test-threads=1
~~~

Cover both sides of the margin, unequal/saturated capacity, quota/health/order
policies, owner yield and ordinary no-affinity scheduling. This measures route
reuse, not cached tokens; preserve payloads and continuation ownership.

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

Pull requests and pushes to <code>main</code> and <code>release/**</code> run checks only. They do not build
installers or publish artifacts. Pushing a <code>v*</code> tag builds the desktop
installers and, after those checks pass, publishes the GitHub Release and updater
manifest. A manual Build run retains desktop installers as Actions artifacts
without publishing, even when started from a tag.

Browser checks run in four independent Playwright shards, preserving the local
file-level test ordering. The required <code>Check</code> status succeeds only
when runtime checks, every browser shard, Windows Rust tests, and the web preview
all pass. Installer builds run alongside
checks; publication still requires all checks and all six platform builds.

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
