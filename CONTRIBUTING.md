# Contributing to Zenith Relay

Use `main` as the stable branch and open reviewed work into `main`.

## Boundaries

- React renders typed snapshots and invokes commands. It does not read files,
  secrets, provider APIs, or client configuration directly.
- `src-tauri` owns desktop storage, native secret services, OAuth callbacks,
  local endpoint lifecycle, and reversible client profile changes.
- `crates/relay-core` owns behavior shared by desktop and server.
- `relay-server` owns the standalone user-managed runtime and encrypted vault.
- Private Zenith billing, inventory, provider economy, customer debit, and
  selling-pool routing never enter this repository.
- Migrations are append-only and secret material must not appear in fixtures,
  logs, frontend state, telemetry, screenshots, or support output.

Read [AGENTS.md](AGENTS.md) and the owning document from [docs/README.md](docs/README.md)
before changing behavior.

## Verification

Frontend, localization, desktop Rust, and browser flows:

```bash
cd src
bun run verify
bun run test:e2e
```

Shared runtime and server strict gates:

```bash
cargo fmt --manifest-path crates/relay-core/Cargo.toml --all -- --check
cargo clippy --manifest-path crates/relay-core/Cargo.toml --all-targets -- -D warnings
cargo test --manifest-path crates/relay-core/Cargo.toml

cargo fmt --manifest-path relay-server/Cargo.toml --all -- --check
cargo clippy --manifest-path relay-server/Cargo.toml --all-targets --locked -- -D warnings
cargo test --manifest-path relay-server/Cargo.toml --locked
```

Before a release, audit every committed Rust lockfile and the frontend
dependency graph:

```bash
cargo audit --file src-tauri/Cargo.lock
cargo audit --file relay-server/Cargo.lock
cd src
bun audit
```

Packaging changes also require:

```bash
cd src
bun run app:build
```

The GitHub matrix must pass on all six desktop OS/architecture targets before a
release is considered cross-platform verified.

## Release Process

After all release gates and real-provider tests pass, merge into `main`. Create
a stable tag only when explicitly approved:

```bash
git tag vX.Y.Z
git push origin vX.Y.Z
```

The tagged workflow publishes signed desktop artifacts and updater metadata.
