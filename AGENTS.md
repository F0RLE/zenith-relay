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
- `src-tauri/src`: Rust/Tauri desktop logic, API requests, validation, formatting, tray, Codex config writes, process launch.
- `src-tauri/capabilities`: Tauri permissions.
- `src-tauri/icons`: app and installer icons.
- `.github/tools`: local clean and Tauri dev/build environment helpers.
- `docs`: release and contributor documentation.
