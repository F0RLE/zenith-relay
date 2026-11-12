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
The assignment terms are in the
[Contributor Agreement](CONTRIBUTOR_LICENSE_AGREEMENT.md). Read that file
before a pull request.

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
These checks are optional in a standalone checkout. Evaluate model-selection
quality separately; live probes, releases and subagents require explicit authorization.

### Design and independent review

Use this workflow for substantial changes with unresolved architectural choices,
especially across routing, protocol, account recovery, or local/Hosted boundaries.
Small, clear fixes keep the normal implementation and focused-check workflow.
No plugin or additional skill is required.

1. Inspect the owning code and affected contracts. Summarize scope, non-goals,
   alternatives, ownership, failure/recovery behavior, affected paths, and
   acceptance checks in chat. Resolve material design choices before coding;
   do not ask for approval at every routine step.
2. When the user explicitly authorizes independent review, use a fresh-context
   reviewer. Supply the proposal, applicable instructions, and relevant source
   and test paths, not the earlier design conversation. Check missing assumptions,
   contract violations, unnecessary complexity, failure cases, and testability
   against the actual code. Same-context self-review is not independent review;
   if isolation is unavailable, report that limitation instead of claiming a pass.
3. Return actionable findings and source references in chat. Validate findings
   before changing the proposal; the user decides material architectural
   trade-offs. Do not silently accept unresolved blockers or expand the scope.
4. Implement the agreed scope and run the owning focused checks. Review approval
   does not replace tests or establish installed-app or live-provider acceptance.
5. Update existing contract documents only when their maintained behavior changes.
   When checking documentation against code, distinguish current contracts from
   planned work in the roadmaps and Hosted designs. Do not report an explicitly
   future feature as a missing implementation of a current contract.

Keep this proportional to the task. Do not create design or review-report files
unless requested; use the existing document owners rather than parallel specs.

## API and protocol references

Use official documentation and SDK types for the exact endpoint/version:
[contract references](.agents/skills/relay-protocol-adapters/references/contracts.md).
Compatible API names do not establish identical features. BPS/account transport
behavior is observed, not specified by the public OpenAI API.

Follow the [protocol skill](.agents/skills/relay-protocol-adapters/SKILL.md) and
the matrix for the affected protocol pair and contract. Third-party comparisons
need an inspected revision, file paths and retained license/attribution. Implement
against official contracts and qualify the complete tool-continuation cycle.

## Account connector references

Use [account connectors](.agents/skills/relay-account-connectors/SKILL.md) for
OAuth, sign-in/import, and reauthentication; use
[pool lifecycle](.agents/skills/relay-account-pool-lifecycle/SKILL.md) for renewal,
quota, admission, execution ownership, and removal. Reuse Relay's authority and
scheduler, and qualify sign-in and pool execution separately.

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
finish active agent requests before restarting. Dev shares your working Relay profile.

Codex's **Relay · Dev** action uses `bun run app:dev` and the default native
profile, sharing the same accounts as terminal development. Set an absolute
`ZENITH_RELAY_DEV_DATA_DIR` to select a separate debug profile.

To opt into automatic Rust rebuilds/restarts, use `bun run app:dev:watch`
or `bun run start --watch`. Do not route the editing agent through that
watch-mode instance: use a separate stable Relay for the agent instead.

Codex desktop actions are provided in `.codex/environments/environment.toml`.
Use **Relay · Dev** from the Run menu to start the normal no-watch debug workflow in
Codex's terminal. After an agent task finishes, stop that terminal command and
run the action again to compile and apply Rust edits. Frontend hot reload stays
enabled. Run Dev, TypeScript, UI tests, or Rust check explicitly from the action
list. Use the commands here for documentation checks and dependency setup.

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
`../scripts/setup/setup-development.ps1` to install missing portable Windows tools
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

Add `--compact` for concise progress, warnings, errors and elapsed time. Complete
build output is saved to `target/desktop-build.log`, replacing the previous log.
The workspace desktop updater uses this mode and retains SHA-256 verification,
safe replacement and startup checks.
It reports `GatewayApiAvailable` separately from `GatewayOwnership`: unavailable
listener metadata does not make a responding API unavailable. The credential-free
API probe rejects redirects and foreign responses and has a total one-second
read deadline. `-RequireGateway` still requires both a responding Relay API and
a listener owned by the newly launched process; unknown or foreign ownership
fails strict startup verification.

Identical renderer filenames and bytes retain their previous timestamps, allowing
Cargo to reuse its release output. Changed assets still trigger native work.
Every build validates the frontend and uses the configured release optimizations
and ES2020 WebView target. Gzip is used for frontend size budgets.

For the Windows workspace's build-and-restart shortcut, use the parent
`CONTRIBUTING.md` section **Rebuild the local Relay desktop executable**.

## Documentation policy

Use one owner per subject. The [documentation index](docs/README.md#find-the-right-document)
lists the owning guides; its [repository tree](docs/README.md#repository-tree)
separates source directories from generated local artifacts.

Maintain architecture, design and contributor documentation in English. Keep
translated guides and UI copy in separate locale-specific files.

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

Keep English notes in `CHANGELOG.md` and Russian notes in
[CHANGELOG.ru.md](docs/releases/CHANGELOG.ru.md). Additional translations use
`docs/releases/CHANGELOG.<locale>.md`. Keep each translated version heading and
date identical to English. Use `### Changed` and, when needed, `### Fixed`, with
translated headings in each locale file. Group longer lists with `####` topic
headings; align translations by topic, changes and PR/commit references.

Source changelogs contain plain Markdown, without locale markers. The release
workflow runs `bun scripts/release/build-release-notes.mjs <version-or-tag>` to
assemble the requested version from these files. It adds `relay-notes:<locale>`
markers only to the generated release body, with English first. The updater's
exact-locale, base-language and English fallback behavior stays unchanged.

Keep the audience boundary explicit: changelog entries and updater notes are
for users. Do not put test counts, CI job status, formatter or audit output,
review history, or unfinished acceptance evidence in them. Keep contributor
commands in this file, current implementation facts in `docs/project/PLANNING.md`,
and unfinished or live-provider acceptance gates in `docs/project/ROADMAP.md`.

Group fixes by the behavior a user observes, rather than adding a bullet for
each patch. Buffer allocation, file moves and dependency-version inventories
belong in code or contributor guidance, not release notes. Preserve release
headings and references when condensing an existing section.

Stable release tags must have a matching dated section in
each maintained changelog. The release workflow fails on missing or empty
translations, duplicate versions or inconsistent dates instead of publishing an
automatically generated pull-request list as user-facing notes.

Define stable error codes in `crates/relay-core/src/error_codes.rs` and reuse its
constants in emitters and classifiers. Add causes and recovery steps to both
localized Help error tables, including public aliases. The catalog test rejects
undocumented codes; classification and recovery policy remain in their owning
modules. Preserve unknown external provider codes as received.

### Desktop localization

English in `src/src/i18n/locales/en.ts` is the canonical message/key source.
Keep runtime code and diagnostics in English; additional UI languages belong
only in locale dictionaries. Do not add per-language branches to components,
startup, tray or OAuth windows. Hosted has its own localization owner.

To add a language:

1. From `src`, run `bun run locale:template <BCP-47-code>` to print an English
   starter with the target language's plural categories. Save and translate it
   as `src/src/i18n/locales/<code>.json` (path relative to the repository root).
2. Add one record in `src/src/i18n/registry.json`: `code`, `nativeName`,
   `englishName`, `direction`, ordered `fallbacks`, `helpLocale` and `format: "json"`.
   Optional `aliases` map explicit OS/region spellings; never infer a different
   script from the same base language. English must remain the final fallback.
3. Preserve keys, placeholders and arrays. Review the translation, long text,
   font coverage and RTL layout before enabling it. `bun run check:locales`
   checks every registered dictionary, interpolation, plural forms and Help path.
   Ordinary UI tests run the same gate in CI; no extra CI language list is needed.

Settings, setup, resource loading and native language validation use that registry.
Only the selected dictionary and fallback chain load. UI switching is latest-wins;
failed loading retains the current language. A small saved public-text snapshot
localizes pre-bundle startup and the native tray; corrupt optional preferences
fall back to English without blocking startup. No credentials or user labels enter it.
Native labels update before preferences are saved; a failed save leaves the active
language applied and is recorded through the existing IPC diagnostics.

UI and Help coverage are independent. Set `helpLocale: "en"` until a translated
guide is ready. A new guide needs only `docs/help/<locale>/README.md` and the
registry value; keep topic/error-reference markers and current UI steps intact.
Release notes already use exact-locale, base-language and English fallback.
Resource budgets separately constrain shared application code, each language
pack, all packs and Help. Do not raise startup limits merely to add dictionaries.

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

`--quick` covers scripts, documentation, guardrails, duplicate-code checks, and
Desktop UI/web unit tests and production builds. Full verification adds Rust
formatting, Clippy, native and browser tests, and audits of both Bun and Cargo
lockfiles. Release gates are listed separately below.

Script, documentation and guardrail checks run first. Desktop UI, web and Rust
then run in independent, sequential lanes, two at a time. Use `--jobs 1` for
sequential execution or `--jobs 3` for all three lanes. A failure stops scheduling;
started commands finish before the runner exits unsuccessfully. Each command
reports elapsed time. Desktop Rust tests use one thread. Both Rust packages
reuse `src-tauri/target`; explicit Cargo target-directory overrides take priority.
The root and `src` verification commands use the same runner.

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
check's normal local default. Packaging, publication, Desktop restarts and
installed-app acceptance are separate steps.

### Public metadata contract check

Deterministic metadata regressions run with the core tests. To validate current
public models.dev, OpenRouter and LiteLLM responses through the actual loader:

~~~powershell
bun scripts/build/cargo.mjs test --manifest-path crates/relay-core/Cargo.toml --locked --lib public_reference_sources_pass_atomic_refresh_validation -- --ignored --nocapture
~~~

This explicit network check uses isolated temporary state and no accounts or
credentials. It qualifies catalog loading, not provider generation or route access.

### Required CI and dependency maintenance

The Build workflow checks pull requests and pushes to `main` and `release/**`.
A release push with an open PR and a separate, non-cancelled Build run for that
exact commit delegates checks to the PR run. All other cases, including API
lookup failures, run the complete suite. Windows CI reports test compilation
(`--no-run --timings`) separately from execution.
`Check` requires successful runtime checks, every Desktop browser shard, Windows
Rust tests and web-preview checks. Failed, skipped or cancelled dependencies fail
the gate. The `main` ruleset requires `Check` and `Release context` on an up-to-date
branch. CODEOWNERS identifies reviewers; the ruleset controls review requirements
and merge bypasses.

Third-party Actions are pinned to full commit IDs. Dependabot maintains those
pins, both Cargo packages, and the actual Bun lockfiles in `src` and `web`.
Runtime CI fetches the current RustSec database once, records its commit, and
uses that same snapshot for both Cargo audits. Both frontend packages run
`bun audit`. A new advisory can fail CI without a source change; investigate
and remediate it rather than pinning an old database to obtain a passing check.

Desktop pins two reviewed upstream changes pending published releases:

- `proc-macro-error2`: [PR #14](https://github.com/GnomedDev/proc-macro-error-2/pull/14),
  commit `53ff94b61c678593158db1a3d50269c6f0de0db7`, fixes the private `proc_macro`
  re-export. Its attribute macro uses the same revision.
- `i18n-config`: merged [PR #176](https://github.com/kellpossible/cargo-i18n/pull/176),
  commit `2ce7b1dfe9a8cc224288bf50cbc2a67a6de863c4`, replaces unmaintained
  `basic-toml` with `toml` for localization build configuration.

Script checks enforce immutable, credential-free Git sources, matching manifest
and lockfile revisions, and the maintained parser. `age` and the Codex authorization
encryption format stay unchanged. Replace each pin when a reviewed published
release includes its fix; keep dependency audits and compiler checks enabled.

The current Linux dependency families retain these upstream constraints:

| Advisory | Dependency owner and required migration |
| --- | --- |
| [RUSTSEC-2024-0370](https://rustsec.org/advisories/RUSTSEC-2024-0370.html) | Tauri's GTK 0.18 family uses `proc-macro-error` 1 through `glib-macros` and `gtk3-macros`. The separate `proc-macro-error2` override does not replace it. |
| [RUSTSEC-2024-0429](https://rustsec.org/advisories/RUSTSEC-2024-0429.html) | GTK 0.18 retains `glib` 0.18. The published fix requires `glib` 0.20 or newer, so qualify a coordinated Tauri/GTK migration rather than mixing incompatible GTK types. |
| [RUSTSEC-2026-0370](https://rustsec.org/advisories/RUSTSEC-2026-0370.html), [0371](https://rustsec.org/advisories/RUSTSEC-2026-0371.html), [0372](https://rustsec.org/advisories/RUSTSEC-2026-0372.html) | Linux keyring/Secret Service and Tauri plugins require zbus 5. Its standalone `zvariant`, `zbus_names` and derive crates moved into zbus 6; migrate their parent integrations together and verify credential-store compatibility. |

These paths are absent from the Windows dependency graph but remain relevant to
Linux delivery. They are unresolved upstream constraints, not false positives or
audit suppressions. Recheck published parent constraints before a platform
upgrade; qualify Linux credential access with an unlocked Secret Service.

Private vulnerability reporting and CodeQL default setup are repository settings,
not local workflow files. Changes to checked-in workflows and maintenance rules
take effect on GitHub only after they reach the relevant branch. Installer
builds still require a manual run or version-tag push; publication remains
restricted to version-tag pushes.

### Offline HTTP/2 transport check

On Windows, build the executable without replacing the running app, then run
the bounded diagnostic from the repository root:

~~~powershell
bun run app:build --no-bundle
$exe = (Resolve-Path .\src-tauri\target\release\zenith-relay.exe).Path
Get-FileHash -LiteralPath $exe -Algorithm SHA256
& $exe --diagnose-http2-retries | Out-Host
~~~

The mode exits before Desktop, single-instance, credential-store or database
initialization. It uses fresh in-memory sources and synthetic keys with fixed
loopback HTTP/2 prior-knowledge peers. A default reqwest client is the positive
retry control. Normal gateway dispatch then checks refused and uncertain resets,
HTTP 429 failover, candidate limits, one logical identity and one usage attempt
per physical send. No saved accounts, sources or proxies are loaded or changed.
All work has a 20-second deadline; listeners and child tasks are cancelled on exit.

Output is one JSON line with aggregate observations and fixed error codes.
Exit 0 requires every invariant to pass; invalid arguments, setup, timeout,
accounting/retry mismatches or output failures return 2. For installed-copy
qualification, repeat with that exact executable after updating to a build that
supports this flag and retain its SHA-256 with the result. Scope is the compiled
HTTP/2 client and gateway accounting; qualify TLS/ALPN, configured proxies, live
pool load and provider behavior separately.

Source-level transport regressions also exercise certificate-verified TLS with
negotiated `h2` ALPN, directly and through source-specific HTTP CONNECT proxies:

~~~powershell
bun scripts/build/cargo.mjs test --manifest-path crates/relay-core/Cargo.toml --locked --lib runtime::http2_diagnostic::tests
~~~

The TLS suite uses freshly generated in-memory certificates and bounded loopback
peers, without prior knowledge or certificate-verification bypasses. It counts
HTTP/2 streams and CONNECT tunnels, compares real gateway dispatches with usage
attempts, and checks refused/uncertain resets, HTTP 429 failover, candidate limits
and an unavailable proxy with no direct bypass. A default-client positive control
must expose protocol-level resends. The TLS matrix has a 30-second deadline;
individual fixture connections and gateway calls are bounded separately.
These tests do not qualify an installed binary or a real provider/proxy combination.

### Subscription proxy diagnostics

For a bounded, anonymous comparison of the first two saved proxies, build the
debug executable with the standard Rust wrapper, then run from the repository root:

~~~powershell
bun scripts/build/cargo.mjs build --manifest-path src-tauri/Cargo.toml --locked
& .\src-tauri\target\debug\zenith-relay.exe --diagnose-subscription-proxies "$env:LOCALAPPDATA\Zenith Relay"
~~~

This mode exits before desktop, gateway, or single-instance initialization. It
reads the existing proxy vault without key creation, migration, or file recovery;
it never changes assignments or passes account tokens/cookies. It issues at most
two header-only GET observations per proxy, through the selected proxy only,
using the subscription client's public headers and management HTTP admission.
Challenges stop that route; rate limits or positive Retry-After stop the comparison.

JSON labels A/B follow saved pool order and omit proxy URLs, credentials, labels,
and account inventory. `authentication: none` and `generationTested: false` are
explicit: a 401 proves neither account access nor model eligibility. `complete`
means both routes were observed, not that authentication succeeded; `deferred`
means a provider pause prevented the second route. Exit 0 returns observations;
exit 2 indicates a setup, argument, or output failure. Setup and argument errors
use fixed codes. The running Relay is not restarted.

To compare the exact subscription query with and without a bearer, use
`--diagnose-subscription-authorization` with the same absolute data root.
It selects one enabled, non-draining Codex account eligible for quota monitoring,
with an existing explicit proxy assignment. Pool membership is not required for
this metadata-only read. It validates the account through the normal account
check, then uses the same canonical `account_id` for two subscription requests:
anonymous first, bearer second. A challenge, rate limit, or Retry-After stops
further requests. The bounded diagnostic never refreshes tokens, retries, changes
assignments, launches the desktop, or stores provider responses. It rechecks
account registration, credentials, and proxy assignment before each dispatch.
The account-check body is parsed only in memory; subscription observations retain
safe headers only. Account ids, credentials, URLs, and bodies are omitted from JSON.
`accountCheckAccepted` proves that check only, not subscription or model access.
Both subscription requests must complete for `status: complete`; inspect their
individual statuses rather than interpreting completion as authorization success.

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
Browser scripts use the installed Playwright CLI through `bunx --no-install`,
preserving its Node interpreter on Windows.

The root verification command includes unit, build, browser and native checks.
Packaging or updater changes also require `bun run app:build`; see
[Build the desktop executable](#build-the-desktop-executable) for installer options
and compiler setup. On Windows, the executable is
`src-tauri/target/release/zenith-relay.exe`. Building does not replace or restart
an installed Relay.

The Desktop release profile uses 16 codegen units for `zenith-relay-core` to
avoid the observed Windows compiler crash with a single large unit. Release
optimization and cross-crate LTO remain enabled; other packages retain their
existing profile settings.

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
checks this package separately before packaging. Qualify the Hosted backend and
real WebView integration after connecting them.

### Build artifacts

Stop builds and browser tests before cleanup. From the repository root:

~~~powershell
bun scripts/maintenance/clean.mjs
~~~

The default removes only `src/.build`, `src/dist`, and `web/dist`. Add `--all`
to also remove test reports, Desktop browser captures, Rust targets, and Tauri
permission schemas. **This includes the locally built release executable.**
From `src`, the equivalent commands are `bun run clean` and `bun run clean:all`.

Use `bun scripts/maintenance/clean.mjs --incremental` to remove only Rust debug
incremental caches. Compiled dependencies, release executables and frontend builds
stay available; the next debug build may take longer.

Paths are anchored to this checkout, not the caller's working directory. All
targets and their parents inside the checkout are checked before removal;
links, non-directories, or unreadable metadata abort cleanup. Removal errors
stop the command, but already removed artifacts are not restored.
Root-level `.build` worktrees, other `gen` contents, committed `docs/screenshots`,
user data, and installed applications are not targets.

Cleanup paths and their validation live next to the command in
`scripts/maintenance/artifact-paths.mjs`; `scripts/lib` is for shared tooling.
Run `bun test ./scripts/maintenance/clean.test.mjs` after cleanup-tooling changes.
These synthetic cleanup checks also run in CI and preserve build artifacts.

### Windows private sign-in check

Run the opt-in native regression on a Windows desktop with WebView2 installed:

~~~powershell
bun scripts/build/cargo.mjs test --manifest-path src-tauri/Cargo.toml --locked --bin zenith-relay private_webviews_start_clean_while_another_private_window_stays_open -- --ignored --test-threads=1
~~~

It opens hidden WebViews against a synthetic loopback page, checks cookies and
local storage across consecutive sign-in windows while another private window
stays open, and verifies cache cleanup after browser shutdown. It uses temporary
profiles, not saved accounts, provider requests or the installed Relay process.

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

The local probes use synthetic inputs and loopback, not provider quota. Compare
the same machine, build profile and fixtures before/after; preserve unrelated
dirty work. Report results in chat or review, not this guide. Local CPU, bytes and
mock timings do not establish installed-app memory, provider TTFT, cache hits or
billing savings. Keep ordinary correctness/continuation tests passing.

#### SIWC live cache comparison

`bun scripts/maintenance/siwc-cache-probe.mjs --help` describes a separate live
probe. It requires explicit authorization and `--allow-requests MODEL EFFORT`;
it consumes provider quota for at most six synthetic generations with no retries.
Supply a current SIWC `accessToken` and optional account `proxyUrl` as JSON through
stdin from an authorized in-memory credential source, never command arguments,
checked-in fixtures or a plaintext file. Preserve the account's configured proxy.
HTTP and a persistent WebSocket use isolated cache cohorts with the same model,
reasoning and tool cycle. The probe uses HTTP only for this explicitly authorized
comparison, independently of the runtime's WebSocket-only SIWC generation policy.
Output contains bounded timing/byte/usage observations,
not credentials, prompts or response IDs. Cache hits do not prove billing savings.
Use `bun test ./scripts/maintenance/siwc-cache-probe.test.mjs` for offline checks.

#### BPS discovery continuation probe

Check retained-size accounting and exact continuation-budget boundaries, then
measure local lookup advancement for small, large-schema, and fragmented-output
fixtures. Preparation stays outside the timing window; use the same build profile
for both runs. These timings do not measure provider latency or process memory.

~~~powershell
bun scripts/build/cargo.mjs test --manifest-path crates/relay-core/Cargo.toml --locked --lib discovery::tests::continuation -- --include-ignored --nocapture --test-threads=1
~~~

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

Claude catalog preparation with 200 synthetic models and the installed client's
publisher metadata (Desktop 4.29.0+ on Windows or macOS):

~~~powershell
bun scripts/build/cargo.mjs test --manifest-path src-tauri/Cargo.toml --locked --bin zenith-relay measure_catalog_preparation_and_publisher_probe -- --ignored --nocapture --test-threads=1
~~~

This reads version metadata without opening the client, changing its profiles,
or sending provider requests. Compare preparation time and publisher-probe count;
these measurements do not establish live response latency.

Installed Claude Code capability-to-wire qualification, using model rows produced
by Relay's actual profile generator:

~~~powershell
$env:ZENITH_CLAUDE_CODE_BIN = 'C:\path\to\installed\claude.exe'
bun scripts/build/cargo.mjs test --manifest-path src-tauri/Cargo.toml --locked --bin zenith-relay installed_claude_code_preserves_generated_identity_and_native_effort -- --ignored --nocapture --test-threads=1
~~~

Select the exact executable for the intended Desktop/Code version, not an
unrelated newer installation. The test starts an isolated CLI process with
`--bare`, no tools or permission bypass, an empty temporary profile and a synthetic
key. It sends only to its loopback fixture and checks unchanged model IDs,
exact effort, one generation request per case, and a complete synthetic SSE
answer. Each child has time/output limits; temporary profiles are removed after
the run. It does not close Desktop, read working accounts, or establish actual
Desktop picker/settings handoff or real-provider acceptance. Keep unsupported
effort controls separate: Code may downgrade a native model's unsupported level
even without a Relay alias.

To qualify the same installed executable through Relay's actual in-process
gateway, generated profile rows, and a synthetic Messages upstream:

~~~powershell
bun scripts/build/cargo.mjs test --manifest-path src-tauri/Cargo.toml --locked --bin zenith-relay installed_claude_code_roundtrips_generated_identity_through_gateway -- --ignored --nocapture --test-threads=1
~~~

Use the same explicit `ZENITH_CLAUDE_CODE_BIN`. The fixture owns both loopback
servers and synthetic keys; it never attaches to the running Relay or a real
provider. It checks the client alias, canonical upstream model, exact effort,
one physical request and one successful usage record per case. The bounded
observer buffers the synthetic SSE reply; this is not a TTFT/latency benchmark.
Client token-count calls remain local fixture replies, not token-count endpoint
qualification. Desktop UI handoff, sparse menus and real-provider acceptance
remain separate.

For the installed client's complete synthetic tool cycle through native Messages
and explicitly declared Responses, Chat Completions and Gemini upstreams:

~~~powershell
bun scripts/build/cargo.mjs test --manifest-path src-tauri/Cargo.toml --locked --bin zenith-relay installed_claude_code_completes_gateway_tool_cycle -- --ignored --nocapture --test-threads=1
~~~

This opts in only the fixture's deterministic `mcp__relay_contract__lookup` tool.
Built-in tools stay disabled and the strict MCP configuration contains only that
local stdio server. `--allowedTools` authorizes that exact synthetic tool, not a
general permission bypass. Its loopback audit callback uses a fresh per-case
capability and must occur between the two generations. A successful case needs
one audited execution, the original call ID/name/arguments and result in order,
two physical sends, matching effort and terminal usage on both sends. The fixture
server negotiates MCP 2025-11-25 and exits on EOF or its deadline; it is not a
general MCP implementation. Each synthetic endpoint declares its own protocol
support, without relaxing native-protocol admission. Messages, Responses and Chat
cases use the declared Max/XHigh levels; the Gemini case declares and requests
High. This is an independent supported-level case, not a substituted Max/XHigh.
The Chat case checks reasoning replay, and Gemini checks original thought
signatures and function-call/result association. A non-opt-in unit check also
validates the foreign fixtures and fragmented streams without launching Code.
Other tool types, real provider behavior, streaming latency and Desktop UI remain
separate checks.

#### Installed Codex tool cycle

Qualify an explicitly selected Codex executable through a generated Relay catalog,
the in-process gateway and a synthetic native Responses upstream:

~~~powershell
$env:ZENITH_CODEX_BIN = 'C:\path\to\installed\codex.exe'
bun scripts/build/cargo.mjs test --manifest-path src-tauri/Cargo.toml --locked --bin zenith-relay installed_codex_completes_gateway_tool_cycle -- --ignored --nocapture --test-threads=1
~~~

The driver uses an empty temporary `CODEX_HOME`, strict inline config, ephemeral
sessions and a read-only sandbox. User config, project instructions, remote
discovery, plugins, telemetry and updates stay outside the fixture. Its only MCP
server exposes the deterministic lookup tool; all generation and audit traffic
stays on loopback with synthetic keys. Any unexpected connection fails the check.
The test requires positive adoption of the generated instructions, unchanged model
and declared Low effort, one audited execution, two physical requests and two
terminal usage records. A content/type/order digest checks every text part of the
client's tool result at the physical upstream, including client-added metadata.
Codex 0.167.0-alpha.3.2 passed this native Responses SSE case. The observer forwards
SSE bytes incrementally with backpressure and reports first-byte, first-text,
tool-argument and terminal timing separately. A synthetic upstream is not a
provider-latency benchmark. This mode leaves working accounts and the running
Relay untouched.

For an explicitly authorized real-provider check, select an existing Codex profile,
one model from its generated catalog and an exact declared effort:

~~~powershell
$cli = (Get-Command codex.exe -CommandType Application | Select-Object -First 1).Source
$profile = Join-Path $HOME '.codex\config.toml'
bun scripts/check/check-codex-wire.mjs --cli "$cli" --profile "$profile" --model "<generated catalog ID>" --effort high --allow-provider-requests
~~~

This mode makes real generation requests through the selected profile's
authenticated loopback `/v1` provider. Its local key stays in the parent process;
the isolated client receives a synthetic observer key. Only the selected catalog
row is copied into temporary state. The check requires two generation requests,
one audited MCP execution, the original dynamic call identity and complete result,
two completed SSE responses, and the exact final acknowledgement. Unknown
connections, extra generations and unrelated built-in execution fail qualification.
The output distinguishes `live_gateway` from synthetic modes and reports client
cycle and between-request time. Match its Relay request IDs against retained
usage/attempt records before claiming physical-send counts or accounting acceptance.

Add `--scenario parallel` to qualify two independent MCP functions in the first
response and both distinct results in the next request. Each tool has its own audit
and expected result. The check preserves namespaces, dynamic call IDs and argument
strings, and accepts independently returned result order. Duplicate executions,
swapped/missing results, a serial-only first response, explicit disabled parallel
policy and unrelated executable output fail qualification. A rejected audit blocks
the next provider generation. The ordinary single-function scenario remains the
default. Effort comes from the selected catalog row and is checked unchanged on
the wire; the installed client's strict configuration must also accept it.

Qualification remains specific to the selected client version, model, effort and
route. JSON clients, other tool kinds, larger batches, media, foreign protocols,
Desktop UI handoff and provider failure/recovery need their own checks. A cache hit
on the next turn is an observation, not a matched cold/warm performance comparison.

Prepared BPS prefix reuse and opt-in debug fingerprinting:

~~~powershell
bun scripts/build/cargo.mjs test --manifest-path crates/relay-core/Cargo.toml --target-dir src-tauri/target --locked --release --lib measure_bps_turn_prefix_reuse -- --ignored --nocapture --test-threads=1
bun scripts/build/cargo.mjs test --manifest-path crates/relay-core/Cargo.toml --target-dir src-tauri/target --locked --release --lib measure_cache_context_fingerprinting -- --ignored --nocapture --test-threads=1
~~~

Preserve tool order, original schemas, malformed-declaration rejection and
current-call permissions. Debug fingerprint timings do not describe debug-off work.

#### BPS stream and batch probes

Measure parallel stream normalization, passthrough output during batch expansion,
and single-call batch screening:

~~~powershell
bun scripts/build/cargo.mjs test --manifest-path crates/relay-core/Cargo.toml --locked --lib -- parallel_stream_normalization_latency measure_bps_batch_passthrough measure_bps_single_call_batch_probe --ignored --nocapture --test-threads=1
~~~

Preserve native call identity, output order, atomic batch validation and terminal
barriers. These probes measure local CPU work; qualify provider timing/cache separately.

#### BPS tool-envelope probe

Compare referenced and legacy envelope decoding on identical synthetic inputs:

~~~powershell
bun scripts/build/cargo.mjs test --manifest-path crates/relay-core/Cargo.toml --locked --lib performance::envelope -- --include-ignored --nocapture --test-threads=1
~~~

Run the same build profile before and after a change. The probe checks exact
payload serialization and unchanged inputs, then reports local decode medians
and p90 for object and JSON-string envelopes. It does not measure provider
latency or token savings; small timing differences may be noise.

Discovery classification and cleanup, including the client call/result/next-response
regression and malformed-argument checks:

~~~powershell
bun scripts/build/cargo.mjs test --manifest-path crates/relay-core/Cargo.toml --locked --lib discovery::tests::inspection -- --include-ignored --nocapture --test-threads=1
~~~

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
6. Update the English changelog and its separate translations for user-visible behavior, or state in the
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
manifest. The release remains a draft until every asset upload and the stable
updater manifest succeed. Failed uploads stop publication and can be retried.
A manual Build run retains desktop installers as Actions artifacts
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
markers when publishing the Release, not in the pull request or source changelogs.
The release workflow generates these blocks from the separate changelog files.

~~~markdown
<!-- relay-notes:en -->
- English changes

<!-- relay-notes:ru -->
- [Russian translation of the changes]
~~~

Use lowercase locale codes. For a new language, add another section such as
<code>&lt;!-- relay-notes:zh --&gt;</code>; no updater code change is required. The
app selects the exact locale, then its base language, then English, and finally
the first available section.

After editing the Release, rerun the <code>Publish updater manifest</code> job.
It copies the current Release body into <code>latest.json</code>; editing the
Release without rerunning that job does not update the in-app changelog.
