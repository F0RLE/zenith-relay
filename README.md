# Zenith Relay

Desktop app for connecting Codex to Zenith API and, in future releases,
managing personal local AI account pools.

## Features

- Saves your Zenith API key.
- Writes the Zenith connection into Codex config.
- Launches Codex from the app.
- Shows key balance, spending, requests, and usage history.
- Opens the Telegram bot for balance top-ups.

Planned:

- Manage personal local OpenAI/Codex accounts and API keys.
- Add editable provider sources with name, base URL, protocol mode, and API key.
- Show quota, reset, subscription, and health state.
- Run a local gateway for the user's own accounts.
- Generate local API keys so Codex/OpenCode can use the local gateway.
- Add Zenith API as one preset provider source next to custom providers and
  personal accounts.
- Import local `auth.json`, token JSON, and compatible personal account exports.

Telegram bot: [@zenith_service_bot](https://t.me/zenith_service_bot)

Integration docs: [docs.zenithmarket.dev](https://docs.zenithmarket.dev)

## API

The app uses:

```text
https://api.zenithmarket.dev/v1
```

## Architecture

The frontend is intentionally thin: it renders UI, keeps form state, and calls Tauri commands.

Rust/Tauri owns API calls, response normalization, validation, formatting, top-up intent handling, key storage, Codex config writes, and process control.

The app configures Codex to use the project API endpoint and displays API-provided account state.

Future local-pool features are local-first. User-owned accounts stay on the
user's device by default and are not uploaded into Zenith infrastructure.
Internal Zenith backend capacity and routing remain outside this public app.

Start with the [documentation map](docs/README.md). Product scope lives in
[docs/product-direction.md](docs/product-direction.md), the active build order
in [docs/local-pool-final-planning.md](docs/local-pool-final-planning.md), and
the detailed future UI in
[docs/app-ux-flow-spec.md](docs/app-ux-flow-spec.md).

## Platforms

GitHub Actions builds Windows, macOS, and Linux artifacts for x64 and ARM64. Releases use the Tauri updater through GitHub Releases.

## Development

```bash
cd src
bun install
bun run app:dev
```

Source layout:

- `src` - React/Vite frontend package.
- `src-tauri` - Rust/Tauri backend and desktop packaging.
- `src-tauri/src/local_pool` - desktop personal-pool adapters and storage.
- `src/src/features/relay` - target Zenith Relay frontend feature.
- `crates/relay-core` - target shared local/server runtime crate.
- `relay-server` - target standalone user-managed server package.
- `.github/tools` - local and CI build helpers.

The canonical future tree is documented in
[docs/project-structure.md](docs/project-structure.md).

Verify before release:

```bash
cd src
bun run verify
```

Contributor and release workflow lives in [CONTRIBUTING.md](CONTRIBUTING.md).

## License

MIT
