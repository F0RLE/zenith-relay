# AGENTS.md

## Rules

- Default branch is `main`; open PRs into `main` when review is needed.
- Use stable dependencies only. No beta, alpha, nightly, or prerelease packages unless explicitly approved.
- Keep UI text in `src/src/i18n`.
- Keep React in `src/src` display-only: local state, components, and Tauri command wrappers.
- Keep API calls, key storage, Codex config writes, validation, formatting, top-up intents, and process control in `src-tauri/src`.
- Configure Codex to use `https://api.zenithmarket.dev/v1`.
- Render state returned by the Zenith API.
- Use the existing key storage path.
- Create top-up links through the project helper endpoint.

## Checks

Run before committing:

```bash
cd src
bun run verify
```

For packaging/updater changes, also run or verify the Tauri build path:

```bash
cd src
bun run app:build
```

## Map

- `src`: Vite frontend package.
- `src/src`: React UI, components, Tauri wrappers, and i18n strings.
- `src-tauri/src`: Rust/Tauri backend, API client, config writes, key storage, launcher, updater hooks.
- `src-tauri/capabilities`: Tauri permissions.
- `src-tauri/icons`: app and installer icons.
- `.github/workflows`: CI builds and releases.
- `.github/tools`: local/CI build helpers.

## Contracts

- The app writes Codex config for `https://api.zenithmarket.dev/v1`.
- The app uses project-owned helper endpoints for stats, usage history, usage version, and top-up intents.
- UI copy should describe Zenith Codex behavior.
