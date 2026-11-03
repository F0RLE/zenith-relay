<div align="center">
  <img src="src-tauri/icons/zenith-relay.svg" width="128" alt="Zenith Relay">
  <h1>Zenith Relay</h1>
  <p>Personal desktop relay for ChatGPT, Claude Desktop, OpenCode, and API clients.</p>
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

- Combines ChatGPT/SIWC, Excel / Basis Points, and compatible API sources in a local pool; Codex connections support direct account switching.
- Connects installed ChatGPT, Claude Desktop, and OpenCode, or another client through
  a compatible API.
- Selects an available pool member for each request and shows its status,
  quota, and request history.
- Launches a client with one saved account or API source directly.
- Keeps the local pool running in the tray when the window is closed; optionally
  shares its API with other devices on a trusted LAN.

Bring your own account or provider API key. Conversations stay in your chosen
client; Relay manages connections and routing. Usage costs are estimated at API prices.

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
| **Computer** | Run the pool on this device, optionally sharing its API on a trusted LAN. | Relay and its local API. Closing the window leaves the process in the tray. |
| **Relay Hosted** | Placeholder for the future hosted service. | The local API and automations keep running. |

Launch a saved API source from **Connections → Sources → Launch**.
For LAN access, use Computer mode.

## Quick start

1. Select **Computer**.
2. In **Connections → Accounts → Sign in**, choose **Codex**, **ChatGPT (SIWC)**
   or **Excel / Basis Points**, or add an API source under **Connections → Sources**.
3. In **Pool → Members**, add the saved connections. Check **Model Rules**
   for the model you want to use.
4. Start the API under **Access → Local API**, then use **Pool → Connect**
   for ChatGPT, Claude Desktop, or OpenCode. Other clients use the API address and request key
   from **Access**.
5. Send a request from the client and find it in **Usage → Requests**.

### Choose a sign-in method

| Method | Main use in Relay | How requests run |
| --- | --- | --- |
| **Codex** | Switch the account used by the installed Codex client, manually or with account automations. | The client uses its selected account directly. |
| **Excel / Basis Points (BPS)** | Add an account to Relay's pool. | Relay adapts requests and client tools to the Excel/BPS transport, then converts the results back for the client. |
| **ChatGPT (SIWC)** | Experimental pool access with permission to use the ChatGPT plan. | Relay sends Responses requests directly to the public OpenAI API, without the Excel/BPS tool bridge. |

SIWC is the more experimental integration in Relay. Its shorter protocol path
describes how it works, not a reliability ranking; access, limits and supported
features depend on the selected authorization.

The account-card badge selects the route. **Account connections** groups matching
authorizations under one pool member; each keeps its own quota. Codex also supports
linked SIWC quota display and explicit image-only pool membership. Codex text
pool access requires separate consent in Settings; see the
[experimental-access warning](docs/help/en/README.md#connect-an-application-through-the-pool).
BPS access, including Free plans, is checked with the provider before use.
All three support [Zenith export/import](docs/help/en/README.md#import-export-and-account-state).
SIWC imports use Zenith format and require a fresh sign-in; existing connections
are preserved. Exported files contain secrets: store them like passwords.
The guide covers [direct use](docs/help/en/README.md#work-without-the-pool),
[manual API setup and LAN access](docs/help/en/README.md#local-api), and
[saved account recovery](docs/help/en/README.md#saved-account-recovery).

The full user guide is available in the application and in
[English](docs/help/en/README.md) and [Russian](docs/help/ru/README.md).
Quick Setup guides the first launch and can be opened again from **Help**.
Use **Recovery** to restore saved client settings.

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

Use the [documentation index and repository tree](docs/README.md) to find the
owning guide or source directory.

Read [CONTRIBUTING.md](CONTRIBUTING.md) before changing the project.
Implemented contracts are mapped in [PLANNING.md](docs/project/PLANNING.md);
open Desktop work is in the [roadmap](docs/project/ROADMAP.md), and future Hosted
delivery has a [separate plan](docs/hosted/HOSTED_RELAY_ROADMAP.md). The project uses
[AGPL-3.0-only](LICENSE). Before submitting a pull request, read the
[Contributor Agreement](CONTRIBUTOR_LICENSE_AGREEMENT.md).

The [model catalog](catalog/README.md) documents the shared display-order
policy and the JSON export for other applications.

```sh
bun scripts/setup/start-dev.mjs
```

The same command is available as `bun run start` from `src`. It prepares locked
frontend and Rust dependencies, then starts the desktop app. Install Bun,
rustup, and the platform's native desktop libraries first. Use
`bun run setup:browsers` only when Playwright browsers are needed.

Run `bun run verify --quick` from the repository root for scripts, documentation,
and both frontend packages. `bun run verify` adds native Rust checks, browser
tests, and dependency audits. See the [verification guide](CONTRIBUTING.md#verification)
for prerequisites and targeted checks.

Report suspected vulnerabilities through the
[private security-reporting process](.github/SECURITY.md).
