# Contributing

Zenith Codex is the public desktop app for configuring Codex to use Zenith.

## Branches

- `nightly` is the default development branch.
- `main` is the stable release branch.
- Open normal pull requests into `nightly`.
- Promote `nightly` into `main` only through a pull request after CI is green.

## Pull Request Flow

```text
feature/<name> -> PR into nightly
nightly -> PR into main
main -> vX.Y.Z tag -> GitHub Release
```

Do not push product changes directly to `main`.

## Local Checks

Run these before opening a pull request:

```bash
cd src
bun run verify
```

For packaging changes, also run:

```bash
cd src
bun run app:build
```

Full release builds are verified by GitHub Actions on Windows, macOS, Linux, x64, and ARM64.

## Scope

- Frontend code lives in `src/src`.
- Focused React components live in `src/src/components`.
- Tauri/Rust app code lives in `src-tauri/src`.
- Tauri capabilities live in `src-tauri/capabilities`.
- App and installer icons live in `src-tauri/icons`.
- Build helpers live in `.github/tools`.
- Release notes and packaging rules live in `docs`.

Keep the app pointed at the Zenith gateway. Do not add old upstream provider URLs to the public desktop app.
