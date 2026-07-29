# Contributing to Zenith Relay

Zenith Relay is a local-first desktop application. Keep changes small, prove
the behavior they alter, and do not move private Zenith backend concerns into
this repository.

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

## Safety rules

- Use stable dependencies only.
- Never put credentials, cookies, authorization headers, session exports, or
  account identities in source, fixtures, screenshots, logs, or support output.
- Keep desktop secrets in the existing credential-store path and server secrets
  in the existing encrypted vault.
- Management tokens and pool request keys are different credentials. Do not
  accept either one in the other's boundary.
- Preserve the distinction between quota monitoring and routing eligibility.
  Do not reinstate a Free-account routing policy or a hard-coded quota window.
- Retry another candidate only before any response bytes have reached the
  client. Keep response ownership affinity intact.
- Database migrations are append-only. Add a new numbered migration; never
  edit a migration that can already have been applied.
- Update a profile through the existing inspect, snapshot, attach, verify, and
  restore flow. Never overwrite a newer user login.

Read [AGENTS.md](AGENTS.md), [PLANNING.md](PLANNING.md), and
[ROADMAP.md](ROADMAP.md) before changing a cross-cutting behavior.

## Documentation policy

The tracked human-facing documentation is deliberately small:

~~~text
README.md
CONTRIBUTING.md
PLANNING.md
ROADMAP.md
docs/help/<locale>/README.md
docs/help/<locale>/this-computer.md
docs/help/<locale>/zenith-api.md
docs/help/<locale>/my-server.md
docs/screenshots/*.png
~~~

<code>AGENTS.md</code> is repository guidance, <code>LICENSE</code> is legal
metadata, and <code>relay-server/openapi.yaml</code> is the machine-readable
server contract. Do not add parallel architecture, design, handoff, or
historical planning documents. Fold current behavior into
<code>PLANNING.md</code>, future work into <code>ROADMAP.md</code>, and user
steps into localized Help files.

To add a locale, add its overview and all three Help files, register its
translation resources, and update the raw Markdown registry in
<code>src/src/features/relay/help/HelpCenter.tsx</code>. Keep Help documents
accurate for the UI's current labels and mode order.

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

### Frontend and desktop

~~~powershell
cd src
bun run check
bun run test:unit
bun run build
bun run test:e2e
~~~

<code>bun run verify</code> runs the type check, unit tests, frontend build,
and desktop Rust tests. Packaging or updater changes also require:

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

Use the real server acceptance gate in [ROADMAP.md](ROADMAP.md) before
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
6. Merge reviewed work into <code>main</code>. Tag and publish only after the
   release checks and live acceptance pass.
