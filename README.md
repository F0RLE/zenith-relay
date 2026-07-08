# Zenith Codex

Desktop app for connecting Codex to Zenith API.

## Features

- Saves your Zenith API key.
- Writes the Zenith connection into Codex config.
- Launches Codex from the app.
- Shows key balance, spending, requests, and token usage.
- Opens the Telegram bot for balance top-ups without putting raw API keys in links.

Telegram bot: [@zenith_service_bot](https://t.me/zenith_service_bot)

Integration docs: [docs.zenithmarket.dev](https://docs.zenithmarket.dev)

## API

The app uses:

```text
https://api.zenithmarket.dev/v1
```

It does not call Zenith admin/internal APIs.

## Architecture

The frontend is intentionally thin: it renders UI, keeps form state, and calls Tauri commands.

Rust/Tauri owns API calls, response normalization, validation, formatting, top-up intent handling, key storage, Codex config writes, and process control.

Do not add backend routing, fallback, pricing, or balance rules to this app.

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
