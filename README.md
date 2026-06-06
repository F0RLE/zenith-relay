# Zenith Codex

Desktop app for connecting Codex to Zenith API.

## What It Does

- Saves your Zenith API key.
- Writes the Zenith connection into Codex config.
- Launches Codex from the app.
- Shows key balance, spending, requests, and token usage.
- Opens the Telegram bot for balance top-ups.

Telegram bot: [@zenith_service_bot](https://t.me/zenith_service_bot)

## API Gateway

The app uses:

```text
https://api.zenithmarket.dev/v1
```

## Architecture

The frontend is intentionally thin: it renders UI, keeps form state, and calls Tauri commands. Request handling, API calls, response normalization, validation, formatting, top-up intent handling, key storage, Codex config writes, and process control belong in the Rust backend under `src-tauri/src`.

### Platform Support

Builds are automatically created for:
- **Windows**: x64 and ARM64 (EXE portable, Setup installer, MSI)
- **macOS**: Apple Silicon (ARM64) and Intel (x64) (DMG, .app.tar.gz)
- **Linux**: x64 and ARM64 (AppImage, DEB, RPM)

All platforms support automatic signed updates through GitHub Releases.

## Updates

Zenith Codex checks GitHub Releases on startup. When a new version is available, it downloads and installs signed updates silently in the background, then relaunches automatically.

Updates are verified using signed artifacts from GitHub Releases. The public key is embedded in the app; releases are signed with `TAURI_SIGNING_PRIVATE_KEY` during CI builds.

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

Clean local frontend artifacts:

```bash
cd src
bun run clean
```

Release and contributor workflow lives in [CONTRIBUTING.md](CONTRIBUTING.md) and [docs/RELEASE.md](docs/RELEASE.md).

## License

MIT
