# Zenith Relay

Zenith Relay is a Tauri desktop application for routing a user's own accounts
and compatible APIs through one OpenAI-compatible endpoint.

## Runtime Modes

- **This computer** runs a private loopback endpoint from the desktop app.
- **My server** manages the same personal pool on a server controlled by the
  user, so requests continue while the desktop is closed.
- **Ready API** connects Codex to Zenith API and shows balance, usage, and
  top-up actions without exposing the saved key to the frontend.

Personal pool features include OAuth accounts, compatible API sources, quota
windows and reset times, a shared scheduler, local client keys, model rules,
usage diagnostics, quota-wake automation, profile backup/restore, and RU/EN UI.

The public app never contains Zenith private selling-pool inventory, billing,
provider economy, or internal routing policy. User credentials stay in the
device secret store or encrypted vault on the server selected by that user.

## Components

- `src` - React/Vite frontend and Playwright tests.
- `src-tauri` - Rust/Tauri desktop host, OS secret storage, OAuth, local
  endpoint, client profiles, and remote management client.
- `crates/relay-core` - shared scheduler, gateway, quota, automation, protocol,
  usage, and redaction logic.
- `relay-server` - standalone encrypted user-managed runtime.
- `docs` - product, architecture, runtime, UX, and active release gates.

Start with the [documentation map](docs/README.md). Exact paths and ownership
live in [project-structure.md](docs/project-structure.md); unfinished release
work lives in [local-pool-final-planning.md](docs/local-pool-final-planning.md).

## Development

```bash
cd src
bun install
bun run app:dev
```

Verification:

```bash
cd src
bun run verify
bun run test:e2e
bun run app:build
```

Run the user-managed server:

```bash
cargo run --manifest-path relay-server/Cargo.toml --release
```

See [relay-server/README.md](relay-server/README.md) for encrypted deployment,
backup, and restore instructions.

## Platforms

The release workflow builds Windows x64/ARM64, macOS Intel/Apple Silicon, and
Linux x64/ARM64. Release artifacts include portable/setup/MSI packages on
Windows, app/DMG packages on macOS, and AppImage/DEB/RPM packages on Linux.

## Zenith API

The recommended Ready API preset is:

```text
https://api.zenithmarket.dev/v1
```

Telegram top-up bot: [@zenith_service_bot](https://t.me/zenith_service_bot)

Public API documentation: [docs.zenithmarket.dev](https://docs.zenithmarket.dev)

## License

MIT
