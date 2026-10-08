<div align="center">
  <img src="src-tauri/icons/zenith-relay.svg" width="128" alt="Zenith Relay">
  <h1>Zenith Relay</h1>
  <p>Personal desktop relay for ChatGPT, OpenCode, and compatible APIs.</p>
  <p>
    <a href="https://github.com/F0RLE/zenith-relay/releases/latest"><img src="https://img.shields.io/github/v/release/F0RLE/zenith-relay?display_name=tag&style=for-the-badge" alt="Latest release"></a>&nbsp;
    <a href="docs/help/en/README.md"><img src="https://img.shields.io/badge/docs-English-2ea44f?style=for-the-badge" alt="English documentation"></a>&nbsp;
    <a href="docs/help/ru/README.md"><img src="https://img.shields.io/badge/docs-Russian-2ea44f?style=for-the-badge" alt="Russian documentation"></a>&nbsp;
    <a href="LICENSE"><img src="https://img.shields.io/badge/license-AGPL--3.0--only-6b7280?style=for-the-badge" alt="AGPL-3.0-only license"></a>
  </p>
</div>

<p align="center">
  Relay keeps user-owned accounts and API sources in one place, selects an
  eligible connection for each request, and exposes a private compatible API.
</p>

## What it does

- Combines ChatGPT accounts and compatible API sources in a local pool.
- Selects a member by model, protocol, availability, quota, load, and the
  selected rotation mode.
- Preserves a provider-native route when the request can use it and converts
  only supported protocol shapes.
- Provides a local API for Responses, Chat Completions, Anthropic Messages, and
  Gemini requests.
- Can manage a user-operated Relay Server. Hosted multi-user accounts, billing,
  and customer wallets are not part of Relay.

Relay is a separate personal-pool product. It must not contain Zenith production
credentials, customer data, or production routing and billing logic.

## Download

Download a package for your platform from
[GitHub Releases](https://github.com/F0RLE/zenith-relay/releases/latest).

- **Windows:** use the Setup installer or the portable EXE. The portable folder
  must be writable for in-place updates.
- **Linux:** choose AppImage, DEB, or RPM.
- **macOS:** choose the DMG for Intel or Apple Silicon. It is ad-hoc signed and
  not notarized; follow the [English](docs/help/en/README.md#install-on-macos)
  or [Russian](docs/help/ru/README.md#установка-на-macos) first-launch steps.

## Modes

| Mode | Use it when | What remains running |
| --- | --- | --- |
| **Computer** | The pool should run on this device. | Relay and its local API. Closing the window leaves the process in the tray. |
| **Choose API** | One saved API source should receive requests directly. | The selected provider. Pool rotation and pooled usage are not used. |
| **On your server** | A user-operated Relay Server should run the pool. | The server; closing the desktop does not stop it. |

## Quick start

1. Open **Connections** and add a ChatGPT account or API source.
2. In **Pool**, add the connections and allow the required models.
3. Start **API**, then use **Pool → Connect** for ChatGPT or OpenCode. Other
   clients use the displayed address and request key.
4. Check **Overview** for state, **Usage** for request history, and **Recovery**
   for Relay-managed client settings.

The full user guide is available in the application and in
[English](docs/help/en/README.md) and [Russian](docs/help/ru/README.md).
Quick Setup can be opened again from **Help**.

## Screenshots

<table>
  <tr>
    <td align="center" width="50%"><img src="docs/screenshots/overview.png" height="360" alt="Overview"></td>
    <td align="center" width="50%"><img src="docs/screenshots/connections.png" height="360" alt="Connections"></td>
  </tr>
  <tr>
    <td align="center" width="50%"><img src="docs/screenshots/pool.png" height="360" alt="Pool"></td>
    <td align="center" width="50%"><img src="docs/screenshots/usage.png" height="360" alt="Usage"></td>
  </tr>
</table>

## Development

Read [CONTRIBUTING.md](CONTRIBUTING.md) before changing the project.
Implemented contracts are in [PLANNING.md](docs/project/PLANNING.md); open work
is in the [roadmap](docs/project/ROADMAP.md). The project uses
[AGPL-3.0-only](LICENSE). Before submitting a pull request, read the
[Contributor Agreement](CONTRIBUTOR_LICENSE_AGREEMENT.md).

```sh
bun scripts/setup/start-dev.mjs
```

The same command is available as `bun run start` from `src`. It prepares locked
frontend and Rust dependencies, then starts the desktop app. Install Bun,
rustup, and the platform's native desktop libraries first. Use
`bun run setup:browsers` only when Playwright browsers are needed.
