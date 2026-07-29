<div align="center">
  <img src="src-tauri/icons/128x128.png" width="112" alt="Zenith Relay">
  <h1>Zenith Relay</h1>
  <p>Personal ChatGPT accounts and compatible API sources behind one private endpoint.</p>
  <p>
    <a href="docs/help/en/README.md">English documentation</a> ·
    <a href="docs/help/ru/README.md">Русская документация</a>
  </p>
  <p>
    <a href="https://github.com/F0RLE/zenith-relay/releases/latest">Download latest release</a> ·
    <a href="LICENSE">AGPL-3.0-only</a>
  </p>
</div>

## Documentation

Choose a language first. Each overview links directly to a separate guide, so
the Help Center and GitHub navigation do not depend on one long page.

| Mode | English | Русский |
| --- | --- | --- |
| Overview | [Read](docs/help/en/README.md) | [Открыть](docs/help/ru/README.md) |
| This computer | [Guide](docs/help/en/this-computer.md) | [Инструкция](docs/help/ru/this-computer.md) |
| Choose API | [Guide](docs/help/en/choose-api.md) | [Инструкция](docs/help/ru/choose-api.md) |
| My server | [Guide](docs/help/en/my-server.md) | [Инструкция](docs/help/ru/my-server.md) |

## What Is Shipped

- Local-first Tauri desktop app with a React/Vite UI.
- ChatGPT OAuth, existing-profile import, and compatible API sources.
- Local personal pool with quota/health checks, model rules, proxies, routing,
  response affinity, and redacted usage history.
- Optional user-managed Relay Server with encrypted vault, SQLite state,
  management API, scoped client keys, backup/restore, and append-only migrations.
- Reversible ChatGPT/Codex profile attachment with automatic snapshots.
- Signed in-app updates, including in-place replacement and rollback for the
  Windows portable EXE.

Relay is a personal deployment. It is not Zenith customer billing, a public
account marketplace, or the internal Zenith account pool.

## Screenshots

<p align="center">
  <img src="docs/screenshots/overview.png" width="49%" alt="Overview">
  <img src="docs/screenshots/connections.png" width="49%" alt="Connections">
</p>
<p align="center">
  <img src="docs/screenshots/pool.png" width="49%" alt="Pool">
  <img src="docs/screenshots/usage.png" width="49%" alt="Usage">
</p>

## Development

```powershell
cd src
bun install
bun run verify
bun run test:e2e
```

For the desktop bundle use `bun run app:build`. Shared runtime and server
checks are listed in [CONTRIBUTING.md](CONTRIBUTING.md).

Current implementation boundaries are in [PLANNING.md](PLANNING.md); unfinished
acceptance work is in [ROADMAP.md](ROADMAP.md).
