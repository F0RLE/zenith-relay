# Release Flow

## v1.0.3

Patch release for the current Zenith desktop build.

Changes:

- Promotes the current nightly desktop app state to the stable `main` release line.
- Keeps the current Zenith API contract, billing precision, sanitized API errors, and request history fixes from `v1.0.2`.
- Aligns the frontend package, Tauri config, Cargo package, and lockfile version to `1.0.3`.

No user action is required beyond updating to the latest release.

No breaking changes from `v1.0.2`.

## v1.0.2

Patch release for the current Zenith API contract and billing display fixes.

Changes:

- Shows usage, key totals, and daily/weekly/monthly spending with exact micro-USD precision instead of rounded cents.
- Fixes request-history scrolling and load-more state.
- Requires the current Zenith key stats contract and removes the old stats fallback path.
- Removes old plaintext saved-key migration; saved app keys are loaded from the OS keyring or Codex config only.
- Keeps user-facing API errors sanitized while preserving the public Zenith support link when it is safe.

No user action is required beyond updating to the latest release.

Breaking change for old gateway builds: this app version expects `/v1/zenith/key/stats`, `/v1/zenith/key/usage`, and `/v1/zenith/key/usage-version`.

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
git tag v1.0.3
git push origin v1.0.3
```

The `Build` workflow runs on `main`, `nightly`, pull requests, and tags matching `v*`.

The frontend is built with React, TypeScript, and Vite before Tauri packaging.

Updates are served from the latest GitHub Release through `latest.json`. Tauri signs update artifacts with `TAURI_SIGNING_PRIVATE_KEY` from GitHub Secrets and verifies them with the public key stored in `tauri.conf.json`.

See [UPDATES.md](UPDATES.md) for detailed information about the update system and troubleshooting.

For `v*` tags it creates or updates the public GitHub Release and uploads release artifacts for:

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
