# Zenith Codex

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
- Show quota, reset, subscription, and health state.
- Run a local gateway for the user's own accounts.
- Import local `auth.json`, token JSON, and Sub2API-style personal exports.
- Private operator upload mode for Zenith-owned accounts only.

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
user's device by default and are not uploaded into Zenith's server account-pool.
Zenith's server account-pool is for Zenith-owned capacity only.

See [docs/product-direction.md](docs/product-direction.md) for the planned
product boundary.

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
- `.github/tools` - local and CI build helpers.

Verify before release:

```bash
cd src
bun run verify
```

Contributor and release workflow lives in [CONTRIBUTING.md](CONTRIBUTING.md).

## License

MIT
