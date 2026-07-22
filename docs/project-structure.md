# Zenith Relay Project Structure

This document is the canonical source for repository paths, package names,
module placement, test placement, and runtime data directories for Zenith
Relay.

Other documents own behavior:

- `product-direction.md` owns product scope and public/private boundaries;
- `local-pool-final-planning.md` owns implementation order;
- architecture documents own runtime behavior and data contracts;
- `app-ux-flow-spec.md` owns screens and interaction behavior.

If another document shows a different file tree, this document wins. Update
this document before introducing another top-level package or moving an owning
module.

## Naming Decisions

| Item | Name | Notes |
| --- | --- | --- |
| Public product | `Zenith Relay` | Used in UI and public documentation |
| Repository | `zenith-relay` | Local folder and GitHub slug |
| Desktop package/binary | `zenith-relay` | Cargo, Bun, and release artifact base name |
| Tauri bundle identifier | `com.zenith.codex` | Kept for installer and updater compatibility only |
| React product feature | `relay` | Lives under `src/src/features/relay` |
| Desktop personal-pool adapter | `local_pool` | Existing Rust module under `src-tauri/src/local_pool` |
| Shared runtime crate | `zenith-relay-core` | Directory `crates/relay-core` |
| Self-host server package/binary | `zenith-relay-server` | Directory `relay-server` |
| Desktop data folder | `Zenith Relay` | Branded platform-local root; old paths migrate in place |
| Windows current-user install folder | `%LOCALAPPDATA%\\Programs\\Zenith Relay` | Keeps binaries and uninstall state outside the desktop data folder |

The repository, package, executable, updater URL, public labels, and desktop
data directory use the product name. The Tauri bundle identifier stays stable
for installed-app upgrades, but appears on disk only below `cache/` for WebView
compatibility. The keyring adapter reads the old service namespace once,
migrates the secret to `Zenith Relay`, and then removes the old entry.

The Windows NSIS hook relocates the default current-user install directory to
`%LOCALAPPDATA%\\Programs\\Zenith Relay`. This prevents installer binaries from
sharing `%LOCALAPPDATA%\\Zenith Relay` with durable user data. Existing custom
install locations remain unchanged.

## Ownership Boundaries

```text
React UI
  -> typed Tauri commands only

Tauri desktop backend
  -> desktop storage, OS secret store, OAuth browser flow, client profiles
  -> hosts the local endpoint
  -> connects to a user-managed relay server

zenith-relay-core
  -> records, validation, scheduler, request execution, model registry
  -> quota state, quota-wake rules, usage contracts, redaction
  -> no Tauri UI, desktop paths, or private Zenith backend logic

zenith-relay-server
  -> standalone user-managed process
  -> server storage, encrypted vault, background jobs, HTTP listeners
  -> reuses zenith-relay-core
```

Hard rules:

1. React never reads files, secrets, process state, or upstream APIs directly.
2. Tauri commands validate input and delegate; they do not contain scheduler or
   gateway policy.
3. `zenith-relay-core` must compile without Tauri and without desktop OS APIs.
4. `zenith-relay-server` must not depend on `src-tauri`.
5. Desktop and server hosts reuse the same scheduler, request execution,
   model-registry, quota, and wake-automation logic from `zenith-relay-core`.
6. Private `zenith-account-pool`, Zenith provider economy, customer billing,
   provider routing, and internal admin policy never enter this repository.
7. Do not create empty placeholder files. Create a target path when its phase
   first uses it.

## Target Repository Tree

```text
zenith-relay/
  AGENTS.md
  CONTRIBUTING.md
  LICENSE
  README.md
  .gitignore
  .coderabbit.yaml

  docs/
    README.md
    project-structure.md
    product-direction.md
    local-pool-final-planning.md
    local-pool-runtime-contract.md
    local-account-auth-architecture.md
    local-gateway-architecture.md
    app-ux-flow-spec.md
    local-pool-ui-notes.md
    help/
      en/
        zenith-api.md
        this-computer.md
        my-server.md
      ru/
        zenith-api.md
        this-computer.md
        my-server.md
    screenshots/
      overview.png

  src/                              React/Vite desktop frontend package
    package.json
    bun.lock
    tsconfig.json
    vite.config.ts
    playwright.config.ts            created when browser tests are added
    index.html
    public/
    src/
      main.tsx
      styles.css                     import root only
      styles/
        tokens.css
        base.css
        shell.css
        controls.css
        tables.css
        dialogs.css
        pages/
          connections.css
          pool.css
          usage.css
          settings.css
      vite-env.d.ts

      app/
        App.tsx                      application root only
        RelayApp.tsx                 onboarding or operational shell switch
        routes.ts                    seven top-level page ids
        providers.tsx                i18n and app-level React providers

      features/
        relay/
          api/
            commands.ts             typed Tauri invoke wrappers only
            types.ts                frontend DTOs matching Rust responses

          state/
            RelayStateProvider.tsx   one app snapshot and refresh lifecycle
            useRelayState.ts

          shell/
            RelayShell.tsx
            Sidebar.tsx
            PageHeader.tsx
            ModeSelector.tsx
            StatusStrip.tsx

          onboarding/
            ProductIntro.tsx
            QuickSetupWizard.tsx
            steps/
              UsageModeStep.tsx
              ConnectionStep.tsx
              EndpointStep.tsx
              ClientStep.tsx
              FinishStep.tsx

          help/
            HelpCenter.tsx           full-page non-sidebar guide for three modes

          pages/
            overview/
              OverviewPage.tsx

            connections/
              ConnectionsPage.tsx
              SourcesView.tsx
              AccountsView.tsx
              ProxyStorageView.tsx
              AutomationsView.tsx
              RemoteServerView.tsx
              ImportDialog.tsx

            pool/
              PoolPage.tsx
              MembersView.tsx
              MemberEditor.tsx
              RoutingSettingsDialog.tsx
              ModelRulesView.tsx

            gateway/
              GatewayPage.tsx
              EndpointView.tsx
              ClientSetupView.tsx

            usage/
              UsagePage.tsx
              RequestsView.tsx
              AggregatesView.tsx
              ErrorsView.tsx
              RequestDetails.tsx
              useTableLayout.ts

            profiles/
              ProfilesPage.tsx
              RecoveryView.tsx

            settings/
              SettingsPage.tsx

          components/
            ConnectionStatus.tsx
            HealthBadge.tsx
            QuotaMeter.tsx
            SecretField.tsx
            EmptyState.tsx
            ConfirmDialog.tsx

      components/                    reusable product-neutral UI only
        Button.tsx
        IconButton.tsx
        Tabs.tsx
        Table.tsx
        Dialog.tsx
        Drawer.tsx
        FormField.tsx
        Toast.tsx

      i18n/
        index.ts
        resources.ts
        locales/
          ru.ts
          en.ts

    tests/
      e2e/
        onboarding.spec.ts
        local-endpoint.spec.ts
        remote-server.spec.ts
        remote-usage.spec.ts
        ready-api.spec.ts
        profiles.spec.ts
        accessibility.spec.ts

  src-tauri/                         Tauri desktop package and desktop adapters
    Cargo.toml
    Cargo.lock
    build.rs
    tauri.conf.json
    capabilities/
    icons/
    src/
      main.rs                        boot, state construction, command registry
      files.rs                       small atomic/canonical file helpers
      tray.rs

      relay/
        mod.rs
        commands.rs                  app-level mode/onboarding/state commands
        state.rs                     combined Ready API/local/remote snapshot

      ready_api/
        mod.rs
        commands.rs
        client.rs                    public Zenith/customer API client
        models.rs
        top_up.rs                    Zenith-only top-up intent bridge

      local_pool/
        mod.rs
        error.rs
        models.rs                    desktop command DTOs only

        commands/
          mod.rs
          state.rs
          connections.rs
          pool.rs
          gateway.rs
          usage.rs
          profiles.rs
          remote_server.rs
          settings.rs

        store/
          mod.rs
          secret_store.rs            encrypted vault with OS-held master key
          telemetry_db.rs            SQLite state, usage, affinity, and migrations
          vault.rs                   authenticated encrypted secret file

        host/
          mod.rs
          local_server.rs            localhost listener lifecycle
          runtime_sync.rs            update running core state after mutations
          watchers.rs                profile/config change watchers

        accounts/
          mod.rs
          oauth_browser.rs           desktop browser/callback integration
          imports.rs                 preview and confirm local imports
          import_orchestrator.rs      host persistence/rollback around core parse
          quota_refresh.rs            bounded manual/automatic host refresh
          secret_adapter.rs          core credential interface -> OS store

        sources/
          mod.rs
          secret_adapter.rs

        profiles/
          mod.rs
          codex.rs
          opencode.rs
          backups.rs
          launcher.rs

        remote/
          mod.rs
          client.rs                  relay management-protocol client
          origin.rs                  URL pinning and redirect protection
          deployment.rs              config/instruction generation only

      platform/
        mod.rs
        paths.rs
        secrets.rs
        process.rs
        browser.rs
        folders.rs
        autostart.rs

  crates/
    relay-core/                      reusable local and server runtime
      Cargo.toml                     package name: zenith-relay-core
      src/
        lib.rs
        error.rs
        ids.rs
        state.rs
        redaction.rs

        sources/
          mod.rs
          record.rs
          openai_compatible.rs
          model_discovery.rs

        accounts/
          mod.rs
          record.rs
          import.rs
          status.rs
          token_authority.rs
          codex_executor.rs

        catalog/
          mod.rs
          registry.rs
          aliases.rs
          rules.rs

        scheduler/
          mod.rs
          candidate.rs
          selection.rs
          capacity.rs
          cooldown.rs
          affinity.rs

        gateway/
          mod.rs
          auth.rs
          request.rs
          translation.rs
          response.rs
          streaming.rs
          execution.rs
          errors.rs

        quota/
          mod.rs
          windows.rs
          refresh.rs

        automations/
          mod.rs
          quota_wake.rs

        usage/
          mod.rs
          event.rs
          aggregate.rs

        protocol/
          mod.rs
          capabilities.rs
          management.rs
          version.rs

      tests/
        scheduler.rs
        gateway.rs
        quota_wake.rs
        protocol.rs

  relay-server/                      standalone user-managed server package
    Cargo.toml                       package/binary: zenith-relay-server
    Cargo.lock
    .env.example
    Dockerfile
    compose.yaml
    README.md
    migrations/
      001_init.sql
    deploy/
      install.sh
      zenith-relay-server.service
    src/
      main.rs
      app.rs                          state construction and runtime rebuild
      config.rs
      state.rs
      runtime_mapping.rs
      usage_persistence.rs

      accounts/
        mod.rs
        token_refresh.rs

      http/
        mod.rs
        public_api.rs                /v1 customer-facing local-pool routes
        middleware.rs
        management/                  desktop management protocol resources
          mod.rs
          sources.rs
          accounts.rs
          imports.rs
          proxies.rs
          keys.rs
          quota.rs
          routing.rs
          models.rs
          usage.rs
          gateway.rs
          automations.rs

      store/
        mod.rs
        sqlite.rs                     Store facade and connection ownership
        records.rs
        imports.rs
        automations.rs
        usage.rs
        affinity.rs
        migrations.rs
        vault.rs
        backups.rs

      jobs/
        mod.rs
        quota_refresh.rs
        health_probe.rs
        wake_automation.rs

    tests/
      public_api.rs
      management_api.rs
      usage_parity.rs
      restart_persistence.rs
      backup_restore.rs

  tests/
    fixtures/                        synthetic data only, never real sessions
      accounts/
      sources/
      quota/
      profiles/
      upstream/

  .github/
    workflows/
      build.yml                      desktop checks and six desktop targets
      relay-server.yml               server checks, binaries, container image
    tools/
      clean.mjs
      tauri-build.mjs
      tauri-dev.mjs
      tauri-env.mjs
      publish-updater-manifest.ps1
```

## Directory Contracts

### `src/src/features/relay`

Owns the Zenith Relay user experience. It may:

- render typed backend state;
- hold temporary form and selection state;
- invoke typed commands;
- format already-authorized values for display.

It may not:

- open local files;
- read OS credentials;
- call upstream model providers;
- implement scheduler or retry policy;
- calculate Zenith prices or balances;
- infer backend state from file paths.

The seven operational sidebar pages are exactly:

```text
overview
connections
pool
gateway
usage
profiles
settings
```

Help is a separate full-page non-sidebar view opened from the shell. It does
not reset onboarding and does not become an eighth operational sidebar item.

Sources, Accounts, Proxy Storage, Automations, Remote Server, Members, Model
Rules, Endpoint, Client Setup, and Recovery remain nested views. Internal pool
keys, command-line diagnostics, and removed history-repair UI do not return as
pages during refactoring. Nested views do not become additional sidebar pages.

### `src-tauri/src/relay`

Owns desktop application orchestration only:

- selected public mode;
- onboarding completion;
- one combined application snapshot;
- delegation to Ready API, local runtime, or remote runtime commands.

It must not contain gateway execution or scheduler policy.

### `src-tauri/src/ready_api`

Owns the existing ready API flow, including the recommended Zenith preset and
compatible ready APIs. Zenith-specific balance, history, and top-up behavior
stays here and is not copied into personal-pool core logic.

### `src-tauri/src/local_pool`

Owns desktop-only personal-pool adapters:

- Tauri command surface;
- transactional local SQLite storage;
- OS keychain integration;
- desktop OAuth browser callback;
- local listener lifecycle;
- Codex/OpenCode attach, backup, restore, and launch;
- management client for a user-owned server.

Generic records and runtime decisions belong in `crates/relay-core`, not here.

### `crates/relay-core`

Owns behavior that must be identical on this computer and on a user-managed
server:

- normalized sources and accounts;
- shared account/source import parsing and redacted previews;
- model discovery and visibility;
- local-key scopes;
- candidate eligibility and ordering;
- cooldown and affinity;
- bounded pre-stream fallback;
- request/stream execution state;
- quota windows and reset state;
- quota wake cycle dedupe and natural-use suppression;
- usage event and aggregate contracts;
- management protocol DTOs and capability/version negotiation;
- redaction rules.

The crate exposes Rust APIs, not Tauri commands or HTTP listeners.

### `relay-server`

Owns the always-on self-host process:

- config and startup;
- public `/v1` listener;
- authenticated management listener;
- SQLite persistence;
- encrypted secret vault;
- background quota, health, and wake jobs;
- backup and restore;
- Docker/systemd packaging.

It does not know Codex desktop profile paths and does not import Tauri.

## Dependency Direction

Allowed:

```text
src React -> Tauri invoke contract
src-tauri -> zenith-relay-core
relay-server -> zenith-relay-core
zenith-relay-core -> standard/runtime HTTP and serialization libraries
```

Forbidden:

```text
zenith-relay-core -> src-tauri
relay-server -> src-tauri
React -> filesystem/keyring/upstream provider
public Relay code -> zenith-account-pool
public Relay code -> zenith-control-api business internals
```

Do not add a root Cargo workspace until build and artifact paths are updated and
verified. `src-tauri` and `relay-server` keep their own lockfiles and depend on
`crates/relay-core` by local path.

## Desktop Runtime Data Tree

The desktop uses one branded platform-local root. On Windows this is
`%LOCALAPPDATA%\\Zenith Relay`; macOS uses
`~/Library/Application Support/Zenith Relay`; Linux uses
`$XDG_DATA_HOME/Zenith Relay`.

```text
Zenith Relay/
  data/                              required to restore Relay state
    relay.sqlite                     settings, records, usage, affinity, schema
    secrets.enc                      encrypted values; master key stays in OS storage
    secrets.enc.bak                  present only during atomic replacement

  recovery/                          durable rollback material
    profiles/                        reversible client profiles and user snapshots
    client-config/                   redacted Ready API/config rollback files

  cache/                             safe to clear while Relay is stopped
    com.zenith.codex/
      EBWebView/                     Tauri/WebView compatibility cache
    imports/                         short-lived import sessions
    oauth_pending/                   resumable OAuth flow metadata
    locks/                           cross-process token refresh locks
    deployments/                     regenerable self-host deployment files

  logs/                              redacted bounded logs when file logging is enabled
```

Account, usage, and support exports use the native save dialog and are written
only to the path selected by the user. They are not silently retained in app
data. WebView data is rebuildable interface state and is never part of backups
or account state.

The ChatGPT client profile remains external at `<user_home>/.codex`. Zenith
Relay edits `config.toml`, `auth.json`, and compatible desktop state only for
explicit attach, restore, or service-tier actions. It does not move ChatGPT
sessions or databases. Redacted config backups are written directly into
`recovery/client-config`.

Storage rules:

1. `relay.sqlite` stores non-secret records, usage metadata, and `secretRef`
   identifiers in transactions.
2. The OS secret store contains only the vault master key.
3. Tokens and API keys stay in the authenticated encrypted vault.
4. OAuth access tokens, refresh tokens, API keys, local generated key values,
   and previous auth files never appear in SQLite rows.
5. Prompt bodies and generated responses are not stored in telemetry, wake
   history, support bundles, or logs.
6. The current flat JSON schema `v14` is imported transactionally once and
   removed only after all SQLite state rows are verified. Older pre-release
   layouts are unsupported.
7. Temporary state, generated output, recovery files, and durable records stay
   in separate directories so clearing one category cannot remove another.
8. Keep `com.zenith.codex` as the bundle identifier for upgrade compatibility,
   but keep its filesystem state below `cache/` rather than using it as the
   product data root.

## Server Runtime Data Tree

Default Linux layout:

```text
/etc/zenith-relay/
  config.toml
  environment                      optional root-readable env file

/var/lib/zenith-relay/
  relay.sqlite
  vault/
    metadata.json
    secrets.enc
  backups/
  quarantine/

/var/log/zenith-relay/
  server.log                       redacted, retention-bounded
```

Container deployments mount the same logical locations as named volumes. The
management token and vault key come from environment/file secrets, not from
`config.toml` committed to source control.

## Tests And Fixtures

| Test | Location |
| --- | --- |
| Rust unit test | next to the owning module in `#[cfg(test)]` |
| Shared runtime integration | `crates/relay-core/tests` |
| Desktop/Tauri integration | `src-tauri/tests` when process-level coverage is needed |
| Server HTTP/persistence | `relay-server/tests` |
| React pure helper | colocated `*.test.ts` using Bun test |
| Desktop flow/visual/accessibility | `src/tests/e2e` using Playwright |
| Synthetic reusable input | `tests/fixtures` |

Fixtures must be synthetic and obviously fake. Real OAuth exports, cookies,
refresh tokens, API keys, user profile backups, prompts, and request logs never
belong in Git.

Generated build output remains ignored:

```text
src/dist/
src/node_modules/
src-tauri/target/
relay-server/target/
crates/relay-core/target/
playwright-report/
test-results/
```

## Phase-To-Path Map

The active phase definitions come from
[local-pool-final-planning.md](local-pool-final-planning.md). This map only
states which paths those phases are allowed to introduce or reorganize.

### P0: Complete Remote Pool

Finish working behavior in the existing `relay-core`, `relay-server`, desktop
`local_pool/remote`, Connections Remote Server view, Help center, server
workflow, and synthetic fixtures. P0 may add only paths required for account
transfer, autonomous server runtime, management protocol, backup/restore, and
monitoring. It does not perform cosmetic source moves.

### P1: Stabilize Product Logic

Work in the existing account, quota, scheduler, gateway, import, store, and
snapshot modules. Add focused tests next to the current owner. Do not rename
paths until local/server parity, transactions, concurrency, streaming, and
failure classification are accepted.

### P2: Source Cleanup

Delete generated output, empty directories, superseded planning artifacts,
dead wrappers, and obsolete Help-to-onboarding behavior. P2 creates no new
runtime layer.

### P3: Restore Ownership Boundaries

Create or finish:

```text
src-tauri/src/relay/
src-tauri/src/ready_api/
crates/relay-core/src/accounts/import.rs
crates/relay-core/src/accounts/status.rs
src-tauri/src/local_pool/accounts/import_orchestrator.rs
src-tauri/src/local_pool/accounts/quota_refresh.rs
src-tauri/src/local_pool/host/runtime_sync.rs
```

P3 moves Ready API behavior out of bootstrap, gives local/server import one
parser, and gives every screen one Rust runtime projection.

### P4: Frontend Structure

Split only the current independent workflows under Connections, Pool, Usage,
Help, and `styles/`. Page entries retain data wiring; React does not gain
backend policy.

### P5: Rust Structure

Create the accepted gateway submodules, management resource modules, concrete
server Store aggregate modules, runtime mapping, usage persistence, and token
refresh paths shown in the target tree. Each path is introduced in the same
batch that moves its tested owner; no empty scaffolding is allowed.

### P6: Release Verification

Finish behavior-focused test placement, Playwright flows, six desktop targets,
server binaries/container smoke, restart and backup/restore, Help/README,
redaction/confidentiality scans, signing, updater verification, and final tree
reconciliation.

## Current-To-Target Migration

Current code is moved only when the owning feature is implemented:

| Current path | Target owner |
| --- | --- |
| `src/src/App.tsx` | `src/src/app/App.tsx` and `features/relay` shell |
| `src/src/tauri.ts` | `src/src/features/relay/api/commands.ts` |
| existing `src/src/components/*` | keep if product-neutral; otherwise move to owning Relay page |
| `src-tauri/src/main.rs` business functions | `ready_api`, `relay`, or `local_pool` command modules |
| `src-tauri/src/key_storage.rs` | generic desktop secret adapter; do not duplicate per feature |
| `src-tauri/src/codex_config.rs` | `local_pool/profiles/codex.rs` plus shared client backup helpers |
| `src-tauri/src/launcher.rs` | `local_pool/profiles/launcher.rs` |
| `src-tauri/src/platform.rs` | keep as one file until multiple platform concerns require `platform/` |
| existing `src-tauri/src/local_pool/*` | keep; split only when the next phase needs the owner boundary |
| desktop `local_pool/accounts/imports.rs` plus server import parser | `relay-core/accounts/import.rs`; hosts keep I/O, secrets, and transactions |
| `local_pool/commands/accounts.rs` business helpers | core record/quota logic plus desktop import/quota orchestration; commands delegate |
| `relay-core/gateway/mod.rs` | gateway request, translation, execution, errors, streaming, and response modules |
| `relay-server/http/management_api.rs` | authenticated resource modules under `http/management/` |
| `relay-server/app.rs` mapping/usage/token adapters | `runtime_mapping.rs`, `usage_persistence.rs`, and `accounts/token_refresh.rs` |
| `relay-server/store/sqlite.rs` aggregate implementations | concrete `Store` facade plus records/imports/automations/usage/affinity/migrations modules |
| `ConnectionsPage.tsx`, `PoolPage.tsx`, and `UsagePage.tsx` child workflows | the matching page view files; page entries keep wiring |
| monolithic `styles.css` | stable import root plus tokens/base/shell/controls/tables/dialogs/page styles |

Do not perform a mass file move before behavior requires it. Git history and a
small working diff are more valuable than making empty folders match the target
tree early.

## Structure Acceptance Checklist

- one canonical file owns path and package naming;
- each behavior has one owning module;
- desktop-only code does not enter the shared core;
- server-only code does not enter Tauri;
- local and server runtime policy is shared, not copied;
- exactly seven top-level UI pages exist;
- secrets and prompt/response bodies have no normal file location;
- runtime data is outside the repository;
- no private Zenith selling-pool logic is present;
- new top-level paths require this document to be updated first;
- no empty scaffolding is committed merely to match this tree.
