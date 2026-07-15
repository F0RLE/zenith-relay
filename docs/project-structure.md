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
    ui-schematic.html
    full-implementation-agent-prompt.md
    p0-baseline.md

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
      styles.css                     global tokens, reset, shell primitives
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

          pages/
            overview/
              OverviewPage.tsx

            connections/
              ConnectionsPage.tsx
              SourcesView.tsx
              AccountsView.tsx
              AutomationsView.tsx
              RemoteServerView.tsx

            pool/
              PoolPage.tsx
              MembersView.tsx
              KeysView.tsx
              ModelRulesView.tsx

            gateway/
              GatewayPage.tsx
              EndpointView.tsx
              ClientSetupView.tsx
              DiagnosticsView.tsx

            usage/
              UsagePage.tsx
              RequestDetails.tsx

            profiles/
              ProfilesPage.tsx
              ProfilesView.tsx
              BackupsView.tsx
              RepairView.tsx

            settings/
              SettingsPage.tsx
              GeneralSettings.tsx
              AppearanceSettings.tsx
              StorageSettings.tsx
              UpdateSettings.tsx
              SecuritySettings.tsx
              RecoverySettings.tsx

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
          settings_store.rs          non-secret JSON records
          secret_store.rs            OS secret references/encrypted fallback
          telemetry_db.rs            SQLite usage and request metadata
          migrations.rs              versioned local-data migrations
          profile_backups.rs
          quarantine.rs

        host/
          mod.rs
          local_server.rs            localhost listener lifecycle
          runtime_sync.rs            update running core state after mutations
          watchers.rs                profile/config change watchers

        accounts/
          mod.rs
          oauth_browser.rs           desktop browser/callback integration
          imports.rs                 preview and confirm local imports
          secret_adapter.rs          core credential interface -> OS store

        sources/
          mod.rs
          secret_adapter.rs

        profiles/
          mod.rs
          codex.rs
          opencode.rs
          backups.rs
          repair.rs
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
          response.rs
          streaming.rs
          execution.rs

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
      app.rs
      config.rs
      state.rs

      http/
        mod.rs
        public_api.rs                /v1 customer-facing local-pool routes
        management_api.rs            desktop management protocol
        middleware.rs

      store/
        mod.rs
        sqlite.rs
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

The seven top-level pages are exactly:

```text
overview
connections
pool
gateway
usage
profiles
settings
```

Sources, Accounts, Automations, Remote Server, Members, Keys, Model Rules,
Endpoint, Client Setup, Diagnostics, Backups, and Repair remain nested views.
They do not become additional sidebar pages.

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
- local JSON/SQLite storage;
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
`$XDG_DATA_HOME/Zenith Relay`. Existing `com.zenith.codex/local-pool` data is
moved into this layout before the WebView or local gateway starts.

```text
Zenith Relay/
  data/                              required to restore Relay state
    metadata.json                    schema version and migration state
    settings.json                    non-secret local gateway settings
    remote-target.json               active user-managed server connection
    connections.json                 non-secret provider/source records
    accounts.json                    non-secret account and quota records
    pool-keys.json                   local key policy and secret references
    automations.json
    usage.sqlite                     no prompt or response bodies
    secrets.enc                      encrypted values; master key stays in OS storage
    secrets.enc.bak

  recovery/                          durable rollback material
    migrations/
    profiles/                        reversible client profiles and user snapshots
    history-repair/                  one current automatic ChatGPT history rollback
    client-config/                   redacted Ready API/config rollback files
    exports/                         exports made by older Relay versions only
    quarantine/                      invalid store files preserved for recovery
    legacy/                          unrecognized legacy files preserved during migration

  cache/                             safe to clear while Relay is stopped
    com.zenith.codex/
      EBWebView/                     Tauri/WebView compatibility cache
    imports/                         short-lived import sessions
    oauth_pending/                   resumable OAuth flow metadata
    repair_previews/                 expiring history-repair previews
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
sessions or databases. Legacy Zenith config backups from `.codex` and
`.codex/zenith-backups` are migrated into `recovery/client-config`.

Storage rules:

1. JSON files store non-secret records and `secretRef` identifiers only.
2. Native OS secret storage is the default.
3. The encrypted vault is an explicit fallback, never plaintext fallback.
4. OAuth access tokens, refresh tokens, API keys, local generated key values,
   and previous auth files never appear in normal JSON or SQLite rows.
5. Prompt bodies and generated responses are not stored in telemetry, wake
   history, support bundles, or logs.
6. Durable record migrations create a backup first and quarantine corrupt
   input instead of deleting it. Directory-layout migrations use same-volume
   renames, are restart-safe, and never overwrite a name that already exists.
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

### P0: Baseline And Store

Use:

```text
docs/p0-baseline.md
src-tauri/src/local_pool/error.rs
src-tauri/src/local_pool/models.rs
src-tauri/src/local_pool/store/
src-tauri/src/platform.rs or the later platform/ split
```

Do not create the server or full UI during P0.

### P1: One Source And One Local Endpoint

Create the first used parts of:

```text
crates/relay-core/src/sources/
crates/relay-core/src/catalog/
crates/relay-core/src/gateway/
crates/relay-core/src/usage/
src-tauri/src/local_pool/commands/
src-tauri/src/local_pool/host/
src-tauri/src/local_pool/profiles/
src/src/features/relay/onboarding/
src/src/features/relay/pages/overview/
src/src/features/relay/pages/connections/
src/src/features/relay/pages/gateway/
```

### P2: Multi-Source Pool

Create:

```text
crates/relay-core/src/scheduler/
crates/relay-core/src/catalog/rules.rs
src/src/features/relay/pages/pool/
src/src/features/relay/pages/usage/
```

### P3: OAuth Accounts, Quota, And Wake Automation

Create:

```text
crates/relay-core/src/accounts/
crates/relay-core/src/quota/
crates/relay-core/src/automations/
src-tauri/src/local_pool/accounts/
src/src/features/relay/pages/connections/AccountsView.tsx
src/src/features/relay/pages/connections/AutomationsView.tsx
src/src/features/relay/pages/profiles/
```

### P4: User-Managed Server

Create:

```text
crates/relay-core/src/protocol/
relay-server/
src-tauri/src/local_pool/remote/
src/src/features/relay/pages/connections/RemoteServerView.tsx
.github/workflows/relay-server.yml
```

The desktop must manage the server through the public management protocol. It
must not read server files directly.

### P5: Complete UI

Finish the seven page trees under `src/src/features/relay/pages`, replace old
single-screen components, and keep one command wrapper layer under
`features/relay/api`.

### P6: Release Verification

Finish Playwright flows, cross-platform desktop builds, server binaries,
container smoke, backup/restore tests, documentation links, and artifact names.

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
