# My server

> A Relay Server you operate runs the pool and /v1 endpoint. The desktop app
> manages it; it does not need to remain open for requests.

Use this mode for continuous or remote access. Prove the flow in **This
computer** first. This is a user-managed deployment, separate from Zenith
production services.

## Quick navigation

- [Two credentials](#two-credentials)
- [Prepare the server](#prepare-the-server)
- [Connect the application](#connect-the-application)
- [Move accounts](#move-accounts)
- [Connect ChatGPT/Codex](#connect-chatgptcodex)
- [Server refresh and automation](#server-refresh-and-automation)
- [If the server fails](#if-the-server-fails)
- [Security and backup](#security-and-backup)

## Two credentials

- The **management token** lets the desktop app administer the server.
- The **profile credential** is created and rotated by Relay for the managed
  ChatGPT/Codex profile.

They are different credentials. Never enter the management token as a client
API key and never expose the profile credential as a management token.

## Prepare the server

- Run a compatible Relay Server as a persistent service.
- Put it behind HTTPS with a valid certificate.
- Keep the vault encryption key outside the data-directory backup.
- Make the data directory and database survive restarts.
- Restrict management API access to a trusted network or access layer.
- Synchronize server time.

Before moving real accounts, verify the health endpoint, connect the desktop
app, attach ChatGPT/Codex, and send a test request.

## Connect the application

1. Switch to **My server**.
2. Open **Connections → Remote Server** and choose **Connect existing server**
   or **Deploy new server**.
3. Enter the HTTPS address and management token.
4. Confirm the server identity only when you understand an identity change.
5. Save and choose **Refresh capabilities**.

## Move accounts

1. Refresh the account's quota and models locally.
2. Start **Move to server** from **Connections → Accounts**.
3. Confirm the selected accounts. Relay validates their protected credentials,
   models, quota, proxy, and server capabilities before committing.
4. Wait for the operation to finish. The card then says **On server** and the
   account appears in the server Pool.

Only your own permitted secrets may move, and only after this explicit
confirmation. Do not import the same session again while a move is running.

## Connect ChatGPT/Codex

1. Start the server API from **API & ChatGPT** if it is stopped.
2. Choose **Connect ChatGPT**.
3. Relay creates or rotates the managed profile credential and saves a
   reversible local profile snapshot.
4. Send a request, close the desktop app, and send a second request. The
   server should continue serving the second request.

**Usage** on the server shows request status, pool member, model, timing,
tokens, and error source. It is loaded from the server after reconnecting.

## Server refresh and automation

Server capability and runtime snapshots refresh on demand. A server may refresh
account quota in its own background jobs according to provider reset times.
Model catalogs are not reasoning probes. A weekly reset automation is
automatic; it runs only when the provider reports the weekly window at zero
and a reset credit available.

## If the server fails

| Symptom | What to check | Action |
| --- | --- | --- |
| Server unreachable | DNS, HTTPS, certificate, and service process. | Restore the service and refresh the connection. |
| Identity changed | Endpoint, database, and deployment. | Do not approve automatically; compare the server and backup. |
| Management API 401 | Management token. | Replace it and reconnect. |
| /v1 401 | Managed profile credential or binding. | Attach ChatGPT/Codex again from the desktop app. |
| No eligible participant | Pool membership, models, proxies, and quota. | Repair that participant or enable a fallback. |
| Request failed | **Error source** in Usage. | Provider, Account, and Relay identify different owners of the failure. |
| Move interrupted | The same saved server connection. | Reconnect it and let Relay recover the operation. |

## Security and backup

Server account sessions and the managed profile credential are encrypted with
the vault key. The management token stays in this computer's credential store.
Use the server's --backup <dir> and --restore <dir> commands and keep the
original vault key separately. A desktop profile snapshot does not replace a
server backup.

Server snapshots, usage, diagnostics, and support bundles are redacted. They
contain operational metadata and aggregates, never raw credentials, cookies,
prompts, authorization headers, or provider response bodies. An explicit
account export is a separate credential-bearing transfer file and may contain
OAuth tokens; keep it private and delete it after import.
