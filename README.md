<div align="center">
  <img src="src-tauri/icons/128x128.png" width="112" alt="Zenith Relay">
  <h1>Zenith Relay</h1>
  <p>Personal desktop relay for ChatGPT accounts and compatible APIs.</p>
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

Zenith Relay lets you keep user-owned ChatGPT accounts and compatible API
sources in one place, choose which connections may receive requests, and use
one private OpenAI-compatible endpoint. ChatGPT/Codex profile changes are
reversible and protected by a recovery point.

## Download

Download the package for your platform from
[GitHub Releases](https://github.com/F0RLE/zenith-relay/releases/latest).

- **Windows:** use the Setup installer. The portable EXE runs without
  installation, but its folder must be writable for in-place updates.
- **Linux:** choose AppImage, DEB, or RPM.
- **macOS:** choose the DMG for Intel or Apple Silicon.

The first launch opens Quick Setup. It asks where Relay should run, what
connection to add, and which client should use the endpoint. You can restart
Quick Setup from **Help** at any time.

## Choose a mode

| Mode | Use it when | What remains running |
| --- | --- | --- |
| **This computer** | You want to combine personal accounts without deploying a server. | Relay and the local endpoint must stay open. |
| **Choose API** | You already have a compatible hosted API and its key. | The provider runs the requests; Relay may be closed after profile setup. |
| **My server** | You operate a Relay Server for continuous or remote access. | The server runs the pool; the desktop app is only the manager. |

Start with **This computer** if you are testing a personal pool. Choose
**My server** only after the local flow works and you have a server you
control. **Choose API** does not create a pool and does not keep Relay request
history.

## Everyday workflow

1. Open **Connections** and add a ChatGPT account, an API source, or a proxy.
2. In **Pool**, include only connections that may receive traffic. Use
   **Model Rules** to enable or disable the models visible to clients.
3. Start the endpoint in **API & ChatGPT** and connect the selected ChatGPT or
   Codex profile. Relay creates a protected return point before changing it.
4. Use **Overview** for health and speed, **Usage** for request details, and
   **Recovery** for profile snapshots.

Account and automatically discovered model status comes from the selected
provider or server. A manual source catalog is an explicit local assertion for
providers that do not expose `/models`; it is not independent proof that the
provider accepts the model. Relay does not replace a provider's quota rules
with a fixed five-hour or weekly formula. A failed check stays visible in the
source/account status, and in **Pool → Model Rules** when there is no valid
account catalog fallback; a failed check is never presented as confirmed
automatic availability.

## Checks, quota, and background work

While the local session is active, model catalogs are checked after startup and
again every eight hours. Quota refreshes follow the reset times reported by
the provider. The visible Overview, Pool, and Connections state may refresh
while those pages are open.

Relay does not send separate probe requests to test reasoning modes. Reasoning
levels shown in **Model Rules** are catalog metadata or a manual rule. Codex
background activity summaries and task titles are a separate setting in
**API & ChatGPT** and can be blocked without disabling ordinary requests.

When a provider reports a weekly reset credit, an account card shows
**Reset weekly quota** and opens a simple Yes/No confirmation. In local mode,
**Connections → Automations** can run a weekly reset automatically when the
weekly window reaches zero. The provider must still report the reset as
available.

Account cards show **API equiv. used** for priced Relay usage and optional
purchase cost payback. When Relay has complete priced usage recorded from the
start of the current provider quota window, it also shows **API equiv. left**:
an approximate remaining amount derived from that window's Relay usage and the
provider-reported percentage. It excludes activity outside Relay and is hidden
when the window, pricing, or usage record is incomplete. Provider quota itself
remains a percentage and reset time, not a monetary balance or billing value.

In **Pool**, **Request speed** selects the service tier for routed requests.
**Standard** leaves the client/provider choice unchanged. **Fast** is shown
only when the selected upstream catalog explicitly confirms `fast` or
`priority` for that model; it then asks that concrete route for the
provider's `priority` tier. A dash means that no current route has confirmed
the tier, not that the model is unavailable. Fast does not change model
selection, reasoning, account order, or routing priority, and the provider may
still apply the standard tier. Fast is a request-speed mode, not a second
user-facing quota: account cards show the primary provider windows and
feature-specific limits such as Code Review, but do not display a separate
Fast-tier meter.

## Privacy boundary

Relay is a personal deployment, separate from the production Zenith Gateway
and Control API. It does not receive Zenith production credentials, customer
keys, backend tokens, account inventory, or internal billing and routing logic.

Desktop credentials stay in the operating system's protected credential store.
When you explicitly move your own connection to a server you operate, that
server keeps it in its encrypted vault. Nothing is uploaded to Zenith
implicitly. Operational diagnostics, snapshots, screenshots, support bundles,
and usage records contain redacted data, not raw credentials, cookies,
authorization headers, prompts, or provider response bodies.

**Account export is different.** It is an explicit credential-bearing transfer
file and may contain OAuth access, refresh, and identity tokens. Use it only
for an intended import, keep it private, and delete it after the transfer.

## Current limits

- ChatGPT is the only shipped subscription-account connector.
- **This computer** stops serving requests when Relay or the computer stops.
- **My server** is a user-managed path and is not production-certified until
  the live acceptance gates in [ROADMAP.md](ROADMAP.md) are complete.
- There is no multi-server pool or distributed scheduling.
- A provider may reject an account, model, region, tool, image, or quota
  window even when the connection itself is saved. The error source in
  **Usage** identifies whether the failure came from the provider, account, or
  Relay.

## Screenshots

<p align="center">
  <img src="docs/screenshots/overview.png" width="49%" alt="Overview">
  <img src="docs/screenshots/connections.png" width="49%" alt="Connections">
</p>
<p align="center">
  <img src="docs/screenshots/pool.png" width="49%" alt="Pool">
  <img src="docs/screenshots/usage.png" width="49%" alt="Usage">
</p>

## Help

The complete mode guides are available in the repository and inside the
application:

| Mode | English | Русский |
| --- | --- | --- |
| Overview | [Read](docs/help/en/README.md) | [Открыть](docs/help/ru/README.md) |
| This computer | [Guide](docs/help/en/this-computer.md) | [Инструкция](docs/help/ru/this-computer.md) |
| Choose API | [Guide](docs/help/en/choose-api.md) | [Инструкция](docs/help/ru/choose-api.md) |
| My server | [Guide](docs/help/en/my-server.md) | [Инструкция](docs/help/ru/my-server.md) |

## For contributors

The product boundary and unfinished acceptance work live in
[PLANNING.md](PLANNING.md) and [ROADMAP.md](ROADMAP.md). Development and
release checks are in [CONTRIBUTING.md](CONTRIBUTING.md).

```powershell
cd src
bun install
bun run verify
bun run test:e2e
bun run screenshots
```

The screenshot command regenerates only the committed documentation images.
See [CHANGELOG.md](CHANGELOG.md) for the user-facing changes from 1.0.5 to
1.1.0.
