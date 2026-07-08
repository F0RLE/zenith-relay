# AGENTS.md

## Rules

- Default branch is `main`; open PRs into `main` when review is needed.
- Use stable dependencies only. No beta, alpha, nightly, or prerelease packages unless explicitly approved.
- Keep UI text in `src/src/i18n`.
- Keep React in `src/src` display-only: local state, components, and Tauri command wrappers.
- Keep API calls, key storage, Codex config writes, validation, formatting, top-up intents, and process control in `src-tauri/src`.
- Do not add Zenith business logic, backend routing, fallback, pricing, balance math, or internal service topology to this app.
- Do not expose private Zenith infrastructure, internal service URLs, tokens, workspace paths, or internal error text in UI, logs, docs, updater metadata, or release assets.
- Treat the API key as a secret. Use the existing key storage path and sanitize user-visible errors.
- Top-up links must never contain raw API keys.

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

- The app uses the public Zenith API gateway and public helper endpoints only.
- The app must not call admin/internal Zenith APIs directly.
- Backend infrastructure changes must remain invisible here unless exposed through stable public API behavior.
