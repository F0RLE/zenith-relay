# Contributing to Zenith Relay

Use `main` as the stable branch. Open pull requests into `main` when review is needed.

## Rules

- This is a Tauri desktop app for local Codex setup.
- Frontend: React + TypeScript + Vite in `src`.
- Backend: Tauri + Rust in `src-tauri`.
- Store user API keys through the existing local key storage path.
- Keep the app pointed at the Zenith API endpoint defined by the project.
- Keep service behavior in the API; the desktop app configures Codex and displays API responses.
- Keep user-facing text short and product-focused.

## Verification

```bash
cd src
bun run verify
```

For packaging/updater changes, also verify:

```bash
cd src
bun run app:build
```

## Development

```bash
cd src
bun install
bun run dev  # starts Vite dev server + Tauri
```

## Layout

- `src/src` - React UI, components, i18n, Tauri wrappers.
- `src-tauri/src` - API client, config writes, key storage, launcher, updater hooks.
- `src-tauri/capabilities` - Tauri permissions.
- `src-tauri/icons` - app and installer icons.
- `.github/workflows` - CI builds and releases.
- `.github/tools` - local/CI helper scripts.

## Release Process

Tag stable releases from the repository root after CI is green:

```bash
git checkout main
git pull origin main
git tag vX.Y.Z
git push origin vX.Y.Z
```

The release workflow creates signed GitHub Release artifacts for Windows, macOS, and Linux on x64 and ARM64.

See [AGENTS.md](AGENTS.md) for ownership boundaries.
