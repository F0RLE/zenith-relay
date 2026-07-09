# AGENTS.md

## Rules

- Default branch is `main`; open PRs into `main` when review is needed.
- Use stable dependencies only. No beta, alpha, nightly, or prerelease packages unless explicitly approved.
- Keep UI text in `src/src/i18n`.
- Keep React in `src/src` display-only: local state, components, and Tauri command wrappers.
- Keep API calls, key storage, Codex config writes, validation, formatting, top-up intents, and process control in `src-tauri/src`.
- Configure Codex to use `https://api.zenithmarket.dev/v1`.
- Render state returned by the Zenith API.
- Do not hardcode provider, model, price, routing, or admin assumptions in the desktop UI.
- Keep public local-pool features local-first. User-owned accounts must stay on
  the user's device by default and must not be uploaded into Zenith server
  account-pool.
- Local gateway mode should expose a generated local API key and local
  OpenAI-compatible base URL, then configure Codex/OpenCode through reversible
  config attach/restore.
- Local provider sources are generic user-owned records: display name, base URL,
  API key, protocol mode, model filters, priority, and weight.
- A user's Zenith API key is one preset personal local-pool source. Treat it as
  user-owned local configuration, not internal Zenith provider routing. Do not
  hardcode local pool behavior around Zenith API only.
- Treat server operator upload as a private/admin capability for Zenith-owned
  accounts only.
- Keep Zenith API mode, personal local pool mode, and private operator upload
  mode separate in UI, storage, and docs.
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
- Model and balance displays come from Zenith API responses.
- UI copy should describe Zenith Codex behavior.
- Product direction for the future open app lives in
  `docs/product-direction.md`.
- Personal local-pool account data is local user data. It is not Zenith
  customer billing state and not server account-pool inventory.
