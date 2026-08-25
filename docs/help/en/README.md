<div align="center">
  <img src="../../../src-tauri/icons/128x128.png" width="96" alt="Zenith Relay">
  <h1>Zenith Relay Help</h1>
  <p>English · <a href="../ru/README.md">Русский</a> · <a href="../../../README.md">Repository home</a></p>
</div>

Zenith Relay is a desktop app for your own ChatGPT accounts and compatible API
connections. It can place selected connections behind one private
OpenAI-compatible endpoint and connect that endpoint to ChatGPT or Codex with
a reversible profile change.

## Start here

| Mode | Choose it when | Guide |
| --- | --- | --- |
| **This computer** | The pool should run on this PC. | [Open guide](this-computer.md) |
| **Choose API** | You already have a hosted API key. | [Open guide](choose-api.md) |
| **My server** | You operate a Relay Server for continuous access. | [Open guide](my-server.md) |

Use **This computer** for the first personal-pool test. Use **My server** only
when the server is yours and the local flow is understood. **Choose API** is a
direct provider connection; it does not create a pool.

## Install and start

Download the current package from
[GitHub Releases](https://github.com/F0RLE/zenith-relay/releases/latest).
Windows normally uses **Setup**; the portable EXE needs a writable folder for
in-place updates. Linux packages are AppImage, DEB, and RPM. macOS packages
are DMGs for Intel and Apple Silicon.

Quick Setup asks for a mode, a connection, and a client. **Import current
profile** reuses an existing ChatGPT sign-in on this computer. You can run the
wizard again from **Help**.

## Sections

- **Overview**: current runtime, healthy capacity, provider statistics when
  available, usage charts, and recent activity.
- **Connections**: ChatGPT sign-ins, imported sessions, API sources, proxies,
  quota automations, and the server management connection.
- **Pool**: which connections may receive requests, model visibility, order,
  weights, and routing policy.
- **API & ChatGPT**: start or stop the private endpoint and attach ChatGPT or
  Codex to it.
- **Usage**: request status, model, selected connection, timing, token totals,
  output speed, and the source of an error. Prompt and response text are not
  stored.
- **Recovery**: create and restore local ChatGPT profile snapshots.
- **Settings**: language, theme, update check, data folder, backup reminder,
  and local data reset.

## Availability and quota

Relay only treats an automatically discovered model as provider/account
availability. A manual source catalog is an explicit local assertion for a
provider that does not expose `/models`; it is not independent proof that the
provider accepts the model. A failed model check remains visible in the
source/account status and, when there is no valid account catalog fallback, in
Model Rules. It does not silently publish a guessed automatic model list.
Quota windows and reset times also come from the provider; there is no
universal five-hour or weekly formula.

During an active local session, model catalogs are checked at startup and every
eight hours. Quota refreshes are scheduled from provider reset times. Relay
does not send separate requests to probe reasoning modes: the modes in Model
Rules are catalog metadata or a manual allow-list.

If a provider exposes a weekly reset credit, the account card shows
**Reset weekly quota** and asks for Yes/No confirmation. Local
**Connections → Automations** can run that reset automatically when the weekly
window reaches zero. The provider must still report the credit as available.

Account cards show **API equiv. used** for priced Relay usage and optional
purchase cost payback. If Relay has complete priced usage from the start of the
current provider quota window, the card also shows **API equiv. left**: an
approximate remaining amount based on that window's Relay usage and the
provider-reported percentage. It excludes activity outside Relay and is hidden
when the window, pricing, or usage record is incomplete. Provider quota itself
is still a percentage and reset time, not money or a billing value.

**Pool → Request speed** controls the provider service tier. **Standard** keeps
the client/provider choice. **Fast** is shown only when the current upstream
catalog explicitly confirms `fast` or `priority` for that model. It requests
the `priority` tier on that concrete route; a dash means that the tier is not
confirmed, not that the model is unavailable. Fast does not select a different
model or change pool order. The provider may still apply the standard tier.
Fast is a request-speed mode, not a second user-facing quota, so account cards
show the primary windows and feature-specific limits rather than a separate
Fast meter.

### Protocols and adapters

`Native` keeps the selected client and provider wire contracts unchanged.
Responses clients can use the explicit `Responses → Messages` or
`Responses → Gemini` bridges when the source is configured for those routes.
Messages uses the provider's `/v1/messages` contract; Gemini uses its native
`generateContent`/`streamGenerateContent` contract. A bridge converts the
request and response, while a native route passes them through. The source's
model-to-format assignments decide which route can receive a model; a
`/models` response alone does not prove every protocol.

## Privacy

This is a personal product, separate from Zenith Gateway and Control API. Relay
does not receive production credentials, customer keys, backend tokens,
production account inventory, or internal billing/routing logic.

Desktop secrets stay in the operating system credential store. A secret moves
to a server only after you explicitly select and confirm a server you operate.
Operational diagnostics, snapshots, screenshots, support bundles, and usage
records are redacted: they do not contain raw credentials, cookies,
authorization headers, prompts, or provider response bodies.

**Account export is intentionally different.** It is a credential-bearing file
for an explicit account transfer and may contain OAuth access, refresh, and
identity tokens. Treat it as a secret, use it only for the intended import, and
delete it afterwards.

## Before asking for help

Open the affected card or request and read the status and **Error source**.
**Provider** means the upstream service rejected the request; **Account** means
the sign-in or route needs attention; **Relay** means local configuration or
protocol handling needs attention. Copy only the sanitized error details from
the error dialog.

The complete product boundary and unfinished acceptance work are in
[PLANNING.md](../../../PLANNING.md) and [ROADMAP.md](../../../ROADMAP.md).
