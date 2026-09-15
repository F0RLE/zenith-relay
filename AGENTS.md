# Relay instructions

Relay is a separate local-first desktop/personal-pool product. Read the workspace
[AGENTS.md](../AGENTS.md) when present; its authorization and secret-handling
rules apply. Production Zenith catalog and route-evidence policies do not
define the personal Relay catalog.

## Ownership and contracts

- `src/src`: React rendering, local UI state, i18n, typed Tauri wrappers.
- `src-tauri/src`: desktop I/O, credentials, OAuth, profiles, process lifecycle.
- `crates/relay-core`: shared discovery, scheduling, protocol, gateway, usage.
- `relay-server`: user-managed runtime, encrypted vault, persistence, management.

Keep side effects and validation in Rust, not React. Keep hosted API, local
pool, and user-managed remote pool distinct in UI, configuration, and storage.

- Never import production Zenith credentials, customer inventory, or internal
  business/routing logic. A user's own Zenith API key is an ordinary personal
  provider source, not access to production internals.
- Accounts stay on the user's device by default. Secret transfer requires an
  explicit confirmed operation to that user's own server. Use existing desktop
  credential storage/server encryption and keep snapshots/exports redacted.
- Management tokens and pool request keys are not interchangeable.
- Profile changes use inspect, snapshot, attach, verify, and restore. Preserve
  newer user logins; do not overwrite them during recovery.
- Account/source inventory preserves all models actually provided, subject to
  explicit user filters, regardless of current endpoint/client support.
  Compatibility belongs at client/admission/routing boundaries, not discovery
  filtering.
  Do not replace discovery with a hardcoded model allowlist.
- Preserve model metadata and its provenance. Price/cache-write accounting must
  follow actual upstream protocol evidence; adapters must not invent counters,
  zero costs, or unsupported cache semantics.
- Keep quota monitoring distinct from routing eligibility. No inferred
  Free-account policy or hardcoded quota window. Retry only before visible
  response bytes; preserve response ownership affinity.
- Migrations are append-only. Use stable dependencies and existing i18n paths.

## Verification and docs

Use the affected frontend, desktop, core, or server checks in `CONTRIBUTING.md`.
CI guardrail and duplicate-code gates remain required; packaging/updater
changes also require `bun run app:build` from `src`.

Current architecture: `docs/project/PLANNING.md`. Open work/live acceptance:
`docs/project/ROADMAP.md`. User steps: `docs/help/<locale>/README.md`.
Local desktop rebuild entry point: `../scripts/rebuild-relay-desktop.cmd`.
