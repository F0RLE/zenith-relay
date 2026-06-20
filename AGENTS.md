# AGENTS.md

## Rules

- Default branch is `main`.
- Open feature work into `main` when review is needed.
- Small operational fixes may be pushed to `main` after checks pass.
- Keep the public app free of old upstream provider URLs and private workspace paths.
- Use stable dependencies only. No beta, alpha, nightly, or prerelease packages unless explicitly approved.
- Keep UI text localized through `src/src/i18n`.
- Keep the frontend dumb. React/TypeScript in `src/src` should render UI, hold local form state, and call Tauri commands only.
- Put request handling, API calls, response normalization, validation, formatting, top-up intent handling, key storage, Codex config writes, and process control in the Rust backend under `src-tauri/src`.
- Do not implement Zenith business logic, provider fallback, model pricing, or balance math in the desktop app. Fetch and display backend-provided state.
- Never expose internal provider URLs, account-pool internals, upstream tokens, or private workspace paths in UI, logs, updater metadata, or release assets.
- Treat the API key as a secret. Store it through the existing key storage path and sanitize error messages before showing them.
- App auto-update behavior is release-critical. If changing updater config, tags, signing, or version metadata, verify the GitHub release flow/manual build path.
- Keep top-up intent links safe: the app may create intents through Zenith API, but must not put raw API keys into Telegram links.

## Checks

Run before committing:

```bash
cd src
bun run verify
```

For packaging changes:

```bash
cd src
bun run app:build
```

## Map

- `src`: Vite frontend package (`package.json`, `index.html`, `src`, `public`, TypeScript/Vite config).
- `src/src`: React + TypeScript frontend; display-only UI and Tauri command wrappers.
- `src/src/components`: focused UI components.
- `src/src/i18n`: localized UI strings.
- `src-tauri/src`: Rust/Tauri desktop logic, API requests, validation, formatting, tray, Codex config writes, process launch.
- `src-tauri/src/main.rs`: Tauri commands, API calls, stats/usage polling, top-up intents, error sanitization.
- `src-tauri/src/codex_config.rs`: Codex config/auth writes and backups.
- `src-tauri/src/key_storage.rs`: local API key storage.
- `src-tauri/src/launcher.rs`: Codex process launch/restart.
- `src-tauri/capabilities`: Tauri permissions.
- `src-tauri/icons`: app and installer icons.
- `.github/tools`: local clean and Tauri dev/build environment helpers.
- `docs`: release and contributor documentation.

## Cross-Repo Contracts

- Uses `zenith-gateway` public `/v1` base URL for Codex.
- Uses Zenith helper endpoints for key stats, usage history, usage version, and desktop top-up intents.
- Does not call `zenith-control-api` admin/internal endpoints directly.
- Future `zenith-account-pool` changes should be invisible to this app except through normal gateway behavior and backend-provided status text.
