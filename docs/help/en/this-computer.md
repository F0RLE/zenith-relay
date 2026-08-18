# This computer

> Your personal pool runs on this computer. Zenith Relay must remain open while
> ChatGPT or another client sends requests through the local API.

**Use it for:** personal work, account checks, and pool setup without a server.

**Do not use it for:** an API that must survive closing Zenith Relay or shutting
down the computer. Use **My server** instead.

## Quick navigation

- [How this mode works](#how-this-mode-works)
- [Before you start](#before-you-start)
- [First setup](#first-setup)
- [Verify the setup](#verify-the-setup)
- [Daily operation](#daily-operation)
- [If requests fail](#if-requests-fail)
- [Recovery and data](#recovery-and-data)

## How this mode works

1. **Connections** stores ChatGPT accounts, API sources, and proxies.
2. **Pool** selects the participants allowed to receive requests.
3. **API & ChatGPT** starts a local OpenAI-compatible endpoint and connects
   ChatGPT to it.
4. **Usage** shows requests, models, the selected participant, speed, tokens,
   and estimated API equivalent.

An account outside the pool is still monitored but cannot receive traffic. An
account in the pool is routed only while its session, requested model, and
quota are available. A **Free** plan is not excluded automatically.

Relay lets routed models receive image attachments without guessing support
from their names. The selected model makes the final decision; native ChatGPT
model capabilities remain unchanged.

## Application tabs

- **Overview** shows runtime state and the **Stream (E2E)** chart. It divides
  all reported output tokens by the Relay-to-provider request time, so first-output
  wait and network time are included.
- **Connections** stores accounts, API sources, and proxies.
- **Pool** controls who may receive requests, plus model rules, order, and
  routing weight.
- **API & ChatGPT** starts the local OpenAI-compatible endpoint and reversibly
  attaches it to the ChatGPT/Codex profile. The desktop app must stay open in
  this mode.
- **Usage** never stores prompt or response text and has four views: **Requests**,
  **Models**, **Pool members**, and **Errors**. Its **Generation speed** uses the
  remaining tokens after the first output and excludes separately reported
  reasoning tokens. **First / total** shows time to first output and the full
  request duration.
- **Recovery** restores saved profiles; **Help** explains the selected mode and
  starts Quick Setup again.

## Before you start

- Zenith Relay and ChatGPT are installed on the same computer.
- At least one valid ChatGPT account or compatible API source is available.
- Any assigned proxy has already been checked.
- A profile backup may be created before switching ChatGPT.

## First setup

### 1. Add an account

Open **Connections** and choose one method:

- **Sign in to ChatGPT** opens the OAuth flow in your browser;
- **Import current profile** uses the existing ChatGPT sign-in;
- **Import** reads a supported JSON or session file.

After adding it, wait for **Updated**. **Pending check** and **Checking** are
normal immediately after import. If the account shows **Sign-in required**,
authenticate again before relying on it in the pool.

### 2. Build the pool

Open **Pool** and add the required accounts and API sources. Check that:

- the participant is enabled;
- the requested model is available;
- its proxy has no error;
- quota is not exhausted;
- routing order and rules match your intended behavior.

Do not add the same account twice. Import updates an existing record when its
identity matches.

### 3. Start the local API

Open **API & ChatGPT** and select **Start API**. The endpoint appears in the
same section. With the default port it looks like this:

```text
http://127.0.0.1:14998/v1
```

Change the port in this section instead of editing configuration files by hand.

### 4. Connect ChatGPT

Choose the account used by the ChatGPT interface, or keep automatic selection,
then select **Connect ChatGPT**. Relay creates a return point before changing
the profile unless the backup reminder was disabled in Settings.

## Verify the setup

1. Confirm that **API & ChatGPT** shows **API is running**.
2. Send a short request from ChatGPT.
3. Open **Usage** and find the new request.
4. Check its model, pool participant, HTTP status, response time, and **Error
   source** when it failed.
5. Return to **Pool** and confirm that the account did not turn red.

> **Ready means:** the request succeeds, Usage names the actual participant,
> and quota refreshes without restarting the application manually.

## Daily operation

- Keep only participants allowed to receive traffic in the pool.
- Use the per-account refresh button for a targeted check.
- A bulk refresh checks every enabled local account, not only pool members.
- Open request details to inspect why a participant was selected.
- Stop the local API before changing its port or maintaining local data.

## If requests fail

| Symptom | Check | Action |
| --- | --- | --- |
| **Sign-in required** | The session may be revoked or invalid | Sign in to that account again |
| **Unavailable** | The latest quota, model, or proxy error | Open account status and correct the reported cause |
| **429** | Account quota, model limit, and other participants | Wait for reset or enable an eligible fallback; one 429 must not block the whole pool |
| Request fails after a participant was selected | **Error source** in Usage | **Provider** rejected the upstream request, **Account** identifies its credential or route, and **Relay** identifies local configuration or protocol translation |
| Model cannot be selected | Model availability for every participant | Refresh models; do not force-enable a model the account does not support |
| Request is absent from Usage | Client endpoint and API state | Reconnect ChatGPT and verify the local endpoint |
| ChatGPT profile changed incorrectly | The latest automatic backup | Open **Recovery** and restore the previous profile |

## Recovery and data

**Sessions and keys** live in the operating system credential store. SQLite
contains non-secret records, quota state, settings, and request statistics.

**Recovery** restores the ChatGPT configuration and available profile files.
Restoring a named snapshot requires confirmation. By default, Relay offers to
save the current profile first; you can decline it for one restore or turn off
that default in Settings. During Relay-managed automatic detach or restore, a
reasoning effort added or changed by Codex or you is kept. An explicitly
selected full snapshot restore may restore the snapshot as a whole and is not
covered by that preservation guarantee. Changes to the managed provider,
endpoint, credentials, or model catalog block managed recovery for review
instead of being overwritten.
Deleting an account from the application also removes its owned local secrets
and related service data.
