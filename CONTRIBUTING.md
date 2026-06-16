# Contributing to Zenith Codex

Use `main` as the active development and stable release branch. Open pull requests into `main` when review is needed; small operational fixes may be committed directly after checks pass.

## Codex-Specific Rules

- This is a Tauri desktop app for local Codex setup
- Stores user key locally (OS keyring)
- Configures Codex to use Zenith API
- Must build for Windows, macOS, Linux, x64, and ARM64
- Frontend: React + TypeScript + Vite
- Backend: Tauri + Rust
- Keep the app pointed at the Zenith gateway. Do not add old upstream provider URLs to the public desktop app

## Verification

```bash
cd src
bun run check
bun run build
```

Full Tauri build is usually verified through GitHub Actions.

## Development

```bash
cd src
bun install
bun run dev  # starts Vite dev server + Tauri
```

## Scope

- Frontend code lives in `src/src`
- Focused React components live in `src/src/components`
- Tauri/Rust app code lives in `src-tauri/src`
- Tauri capabilities live in `src-tauri/capabilities`
- App and installer icons live in `src-tauri/icons`
- Build helpers live in `.github/tools`
- Release notes and packaging rules live in `docs`

## Platform Support

See [PLATFORM-SUPPORT.md](PLATFORM-SUPPORT.md) for platform details and signing setup.

## Release Process

Tag stable releases from the repository root after CI is green:

```bash
git checkout main
git pull origin main
git tag v1.0.3
git push origin v1.0.3
```

The release workflow creates GitHub Release artifacts for Windows, macOS, and Linux on x64 and ARM64.

## Updates

See [docs/UPDATES.md](docs/UPDATES.md) for auto-update system details.

## Boundaries & Architecture

See [AGENTS.md](AGENTS.md) for detailed ownership boundaries and architecture.
