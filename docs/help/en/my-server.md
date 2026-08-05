# My server

> Your pool and OpenAI-compatible API run on Relay Server. The desktop
> application manages that server but does not need to remain open for requests.

**Use it for:** continuous operation, remote access, and one managed pool shared
by multiple clients.

**Do not use it for:** a server that has not been deployed yet. Prove the pool
in **This computer** mode first.

## Quick navigation

- [How this mode works](#how-this-mode-works)
- [Prepare the server](#prepare-the-server)
- [Connect the application](#connect-the-application)
- [Move accounts](#move-accounts)
- [Connect a client](#connect-a-client)
- [Verify the setup](#verify-the-setup)
- [If the server fails](#if-the-server-fails)
- [Security and recovery](#security-and-recovery)

## How this mode works

There are two separate access levels:

- the **management token** connects Zenith Relay to the server administration
  API;
- a **client key** lets ChatGPT or another client send inference requests to
  the public `/v1` endpoint.

Never use the management token as a client API key. It has a different purpose
and must not appear in end-user configuration.

The server owns participants, quota state, routing, usage logs, and client keys.
The desktop application renders server snapshots and sends confirmed management
operations.

Relay lets routed models receive image attachments without guessing support
from their names. The selected model makes the final decision; native ChatGPT
model capabilities remain unchanged.

## Prepare the server

- A compatible Relay Server version runs as a persistent service.
- Its public endpoint uses HTTPS with a valid certificate.
- The management token and vault encryption key come from server secrets.
- The data directory and database survive container or service restarts.
- Server time is synchronized.
- Management API access is restricted to a trusted network or separate access
  control.

Before moving real accounts, verify the health endpoint and one synthetic
request through a client key.

## Connect the application

1. Switch to **My server**.
2. Open **Connections** → **Server**.
3. Enter the HTTPS endpoint and management token.
4. Confirm the server identity after validation.
5. Save the connection and refresh its snapshot.

Server identity prevents silently connecting to a different deployment at the
same URL. Do not approve an identity change until its cause is understood.

## Move accounts

1. Refresh quota and models for the local account first.
2. Start the move from **Connections**.
3. Confirm the selected accounts. Relay validates their protected credentials,
   server compatibility, proxy state, models, and quota before committing.
4. Wait for the ownership operation to finish.
5. Verify that the card shows **On server** and that the account appears in the
   server **Pool**. A successful move adds it to that pool automatically.

> Do not import the same session again while a move is incomplete. Reconnect
> the recorded server and let Relay recover the existing ownership operation.

## Connect a client

1. Open **API & ChatGPT** → **Client access**.
2. Create a client key with the required models and budget.
3. Store the secret immediately; it is not shown in full after the dialog closes.
4. Connect ChatGPT from the application, or configure another OpenAI-compatible
   client with the server `/v1` URL and this key.
5. Start the server API if process management is enabled by the deployment.

## Verify the setup

1. Send a request and find it in **Usage**.
2. Check its model, participant, speed, tokens, and HTTP status.
3. Close the desktop application.
4. Send a second request with the same client key.
5. Reopen Zenith Relay and confirm that the second request was stored.

> **Ready means:** both requests succeed, the second works without the desktop
> app, and its statistics appear after reconnecting.

## If the server fails

| Symptom | Check | Action |
| --- | --- | --- |
| Server is unreachable | DNS, HTTPS, certificate, and Relay Server process | Restore network or service, then refresh the connection |
| Server identity changed | Endpoint, database, and server data directory | Do not auto-approve it; compare the deployment and backup |
| Management API returns `401` | Management token | Replace the management token without changing client keys |
| `/v1` returns `401` | Client key and its state | Create or re-enable a client key |
| No eligible participant | Pool state, quota, models, and proxies | Repair the specific participant or enable a fallback |
| Move was interrupted | The recorded server connection | Reconnect the same server and let Relay recover the operation; do not create a second copy manually |
| Usage does not refresh | Server `usage` capability and version | Install a compatible server version and fetch a new snapshot |

## Security and recovery

Account sessions are encrypted with the server vault key. The management token
is stored in this computer's credential store. Client keys should expose only
the required protocols, sources or accounts, models, and budget.

Use the standalone server `--backup <dir>` and `--restore <dir>` commands so the
database and encrypted vault are validated together. Preserve the stable vault
key separately; a backup without its original key cannot restore account
sessions. Desktop **Recovery** protects the local ChatGPT profile and does not
replace a server backup.
