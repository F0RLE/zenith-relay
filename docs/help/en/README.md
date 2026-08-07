<div align="center">
  <img src="../../../src-tauri/icons/128x128.png" width="96" alt="Zenith Relay">
  <h1>Zenith Relay Documentation</h1>
  <p>English · <a href="../ru/README.md">Русский</a> · <a href="../../../README.md">Repository home</a></p>
</div>

Zenith Relay is a local-first desktop application for managing personal
ChatGPT accounts and compatible API sources. It can expose an allowed live
model set through one private OpenAI-compatible endpoint and attach that
endpoint to ChatGPT/Codex through a reversible profile change.

## Start Here

Each operating mode has its own complete guide:

| Mode | Runtime | Guide |
| --- | --- | --- |
| **This computer** | Personal pool and `/v1` endpoint run inside the desktop app. | [Open guide](this-computer.md) |
| **Choose API** | ChatGPT connects directly to a selected compatible hosted API. | [Open guide](choose-api.md) |
| **My server** | Personal pool runs on a Relay Server you operate. | [Open guide](my-server.md) |

Use **This computer** first when testing personal accounts. Use **My server**
only when the endpoint must continue after the desktop app closes. Use
**Choose API** when an existing hosted API key is enough and no personal pool
is required.

## Install

Download the current platform package from
[GitHub Releases](https://github.com/F0RLE/zenith-relay/releases/latest).
Windows users should normally choose **Setup**; the portable EXE does not need
installation. Linux releases include AppImage, DEB, and RPM packages. macOS
releases include Intel and Apple Silicon DMG files.

After the first launch, Quick Setup asks for the operating mode, connection,
and client profile. When it finds an eligible existing ChatGPT sign-in,
**Import current profile** adds it to the local pool and continues to client
selection after a short confirmation. The same wizard can be restarted from
**Help**.

## Application Sections

- **Overview** shows runtime state, active capacity, balance/statistics where
  available, and recent activity.
- **Connections** stores ChatGPT sign-ins or imported sessions, compatible API
  sources, proxies, and server management connection.
- **Pool** controls enabled members, drain state, model rules, order, weight,
  and routing strategy.
- **API & ChatGPT** starts the personal endpoint and attaches the selected
  endpoint to ChatGPT/Codex.
- **Usage** shows request status, selected member, model, tokens, timing,
  speed, and API-equivalent estimate without storing ordinary prompt/response
  bodies.
- **Recovery** restores profile snapshots or removes Relay-managed settings
  without overwriting unrelated user configuration.
- **Help** renders the same localized mode guides stored in this repository.

## Routing And Quota

Monitoring and routing are separate. An enabled account may still be checked
outside the pool, while a routed account must be enabled, in the pool, healthy,
not draining, credential-ready, proxy-ready, allowed for the requested model,
and have usable quota.

Quota windows come from provider evidence. Relay does not invent one fixed
five-hour or weekly limit. A retry may use another eligible participant only
before response bytes reach the client. Response and prompt affinity never
force traffic onto an unavailable account.

## Privacy And Recovery

Desktop secrets use the operating-system credential store. A self-hosted
server stores account secrets and its managed profile credential in the
encrypted vault, while operational state lives in SQLite. The server
management token and profile credential are separate and must never be
exchanged.

Keep the server behind HTTPS, keep its vault key outside the data-directory
backup, and test a restore before depending on it. Relay redacts credentials,
cookies, authorization headers, prompts, and account identities from normal
diagnostics and usage records.

## Screenshots

<p align="center">
  <img src="../../screenshots/overview.png" width="49%" alt="Overview">
  <img src="../../screenshots/connections.png" width="49%" alt="Connections">
</p>
<p align="center">
  <img src="../../screenshots/pool.png" width="49%" alt="Pool">
  <img src="../../screenshots/usage.png" width="49%" alt="Usage">
</p>

## Current Limits

- ChatGPT is the only shipped subscription-account connector.
- **This computer** stops when the desktop process stops.
- **My server** still requires the live production acceptance listed in the
  roadmap before it should be called production-ready.
- Multi-server distributed leases and cross-server prompt affinity are not
  implemented.
- Additional account systems and client integrations require a permitted,
  reversible, tested authentication/configuration path.

For implementation boundaries read [PLANNING.md](../../../PLANNING.md). For
unfinished work read [ROADMAP.md](../../../ROADMAP.md). Development and release
checks are in [CONTRIBUTING.md](../../../CONTRIBUTING.md).

License: [GNU AGPL v3.0 only](../../../LICENSE).
