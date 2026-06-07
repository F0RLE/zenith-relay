# Release Flow

## v1.0.1

Patch release for the first public Zenith Codex build.

Changes:

- Rebuilds the desktop app with version `1.0.1` so signed updater artifacts can supersede `v1.0.0`.
- Keeps the existing Zenith API endpoint and local Codex configuration flow unchanged.
- No user action is required beyond installing or updating to the latest release.

No breaking changes.

GitHub default branch: `main`.

Use this flow:

1. Open development pull requests into `nightly`.
2. Promote `nightly` into `main` through a pull request after CI is green.
3. Create a tag on the `main` commit:

```bash
git checkout main
git pull origin main
git tag v1.0.2
git push origin v1.0.2
```

The `Build` workflow runs on `main`, `nightly`, pull requests, and tags matching `v*`.

The frontend is built with React, TypeScript, and Vite before Tauri packaging.

Updates are served from the latest GitHub Release through `latest.json`. Tauri signs update artifacts with `TAURI_SIGNING_PRIVATE_KEY` from GitHub Secrets and verifies them with the public key stored in `tauri.conf.json`.

See [UPDATES.md](UPDATES.md) for detailed information about the update system and troubleshooting.

For `v*` tags it creates a draft GitHub Release and uploads release artifacts for:

- Windows x64
- Windows arm64
- macOS Apple Silicon arm64
- macOS Intel
- Linux x64
- Linux arm64

The app writes the Zenith connection into the local Codex config and uses:

```text
https://api.zenithmarket.dev/v1
```
