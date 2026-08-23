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
    <a href="CHANGELOG.md">Changelog</a> ·
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
- Provider-neutral source discovery with explicit protocol bindings, confirmed
  reasoning capabilities, API-reported model prices, and optional per-source
  price overrides.
- Usage diagnostics that distinguish Relay, account, and provider failures
  without recording prompts, response bodies, or secrets.
- Runtime snapshots, telemetry, exports, diagnostics, and screenshots are
  redacted; they are not a transport for credentials or provider payloads.
- Account views keep provider-reported quota windows separate from direct
  token-based API-equivalent and optional purchase-cost payback; Relay does
  not turn a quota percentage into a monetary entitlement.
- Optional user-managed Relay Server with encrypted vault, SQLite state,
  management API, managed ChatGPT/Codex profile attachment, backup/restore, and
  append-only migrations.
- Live model catalogs and reversible ChatGPT/Codex profile attachment with
  automatic snapshots.
- Signed in-app updates, including in-place replacement and rollback for the
  Windows portable EXE.

Relay is a personal deployment. It is not Zenith customer billing, a public
account marketplace, or the production Zenith account pool. It is also separate
from the production Zenith Gateway and Control API: production credentials,
customer keys, backend tokens, account-pool inventory, and internal business
or routing logic do not enter or leave this repository.

## Privacy Boundary

Desktop secrets stay in the operating-system credential store. A server that
the user owns can keep transferred user-owned secrets in its encrypted vault.
The transfer is possible only after the user explicitly selects that server and
confirms the management operation; it is never an implicit upload to Zenith
systems.

Raw secrets, cookies, authorization headers, prompts, response bodies, and
provider session material must not appear in UI snapshots, SQLite telemetry,
logs, exports, diagnostics, screenshots, or ordinary server API snapshots.
Management tokens and `/v1` profile credentials are separate credentials and
are never interchangeable. Documentation and examples use placeholders only.

## Current Direction

The next work prioritizes reliable, provider-neutral operation over adding
vendor-specific shortcuts: prove the existing personal-pool and server paths
with real permitted accounts, measure user-visible latency, and keep model,
price, and error behavior covered by regression tests. New account connectors,
client integrations, and multi-server scale remain demand-gated. The exact
acceptance gates and their order are in [ROADMAP.md](ROADMAP.md).

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
acceptance work is in [ROADMAP.md](ROADMAP.md). Release history is in
[CHANGELOG.md](CHANGELOG.md).
