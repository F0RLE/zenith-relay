# This computer

> The personal pool and private /v1 endpoint run on this computer. Keep
> Zenith Relay open while ChatGPT, Codex, or another client sends requests.

Use this mode for personal work and pool setup without a server. Use **My
server** when the endpoint must continue after Relay or the computer closes.

## Quick navigation

- [Before you start](#before-you-start)
- [First setup](#first-setup)
- [Daily operation](#daily-operation)
- [Verify the setup](#verify-the-setup)
- [If a request fails](#if-a-request-fails)
- [Recovery and data](#recovery-and-data)

## Before you start

- Install Zenith Relay and ChatGPT/Codex on the same computer.
- Have at least one valid ChatGPT account or compatible API source.
- Check any proxy before assigning it to an account.
- Allow Relay to create a profile recovery point before connecting a client.

## First setup

1. Open **Connections** and choose **Sign in**, **Import current profile**, or
   **Import**. Wait until the account says **Updated**. **Pending check** and
   **Checking** are normal immediately after an import.
2. Open **Pool**, add the accounts or API sources that may receive requests,
   and check that they are enabled, healthy, model-compatible, and within
   quota. An account outside the pool can still be checked but cannot receive
   traffic.
3. Open **API & ChatGPT** and choose **Start API**. The default address is:

       http://127.0.0.1:14998/v1

   Change the port in the app, not in a configuration file.
4. In the same section choose the ChatGPT/Codex account binding and select
   **Connect ChatGPT**. Relay saves a protected return point before changing
   the profile.

## Daily operation

- Use **Connections → Accounts** to refresh one quota or **Refresh all quotas**
  for a bulk check.
- Use **Pool → Model Rules** to hide a model from clients or to set its
  reasoning allow-list. Reasoning is configured from catalog data; Relay does
  not probe it with an extra request.
- Use **Connections → Automations** for quota wake tasks. A weekly-reset task
  is always automatic and runs only after the provider reports the weekly
  window at zero and the reset credit available.
- Keep the endpoint running before changing its port or attaching a profile.
- The model catalog refreshes at startup and every eight hours while the local
  background session is active. This is separate from the visible page refresh.
- In **API & ChatGPT**, Codex background activity summaries and task titles can
  be allowed or blocked independently of ordinary requests.

## Verify the setup

1. Confirm **API is running**.
2. Send a short request from ChatGPT or another OpenAI-compatible client.
3. Open **Usage → Requests** and inspect the model, pool member, status,
   First / total timing, and token totals.
4. If it failed, open the request and read **Error source**. Do not treat a
   provider error as a local Relay error.

## If a request fails

| Symptom | What it means | What to do |
| --- | --- | --- |
| **Sign-in required** | The provider session expired, changed, or was revoked. | Sign in to that account again. |
| **Unavailable** | The latest health, model, proxy, or quota check failed. | Open the account status and fix the reported cause. |
| **429 / quota exhausted** | The selected account or model is limited. | Wait for its provider reset or enable an eligible fallback. |
| Model is missing | No eligible participant reported that model. | In Automatic mode, refresh models and check Pool rules. If the provider has no `/models`, use the source's Manual model mode and verify the entered ID with the provider. |
| Request is absent from Usage | The client is not using the local endpoint. | Reconnect ChatGPT and verify the base URL. |
| Model refresh shows an error | The provider rejected discovery or returned unusable metadata. | Fix the account/API source or proxy, then refresh. The old verified catalog remains visible; a Manual catalog is not re-probed in the background. |
| ChatGPT profile is wrong | A profile change needs to be reversed. | Open **Recovery** and restore the automatic or named snapshot. |

## Recovery and data

Sessions and API keys are stored in the operating system credential store.
Local pool records, quota state, settings, and redacted request statistics are
stored in Relay's data folder.

**Recovery** creates a protected automatic return point before a managed
profile change. A named snapshot restores the full config.toml and auth.json,
including sign-in, MCP connections, and plugins, after confirmation. Relay
refuses a managed restore when it detects a newer manual profile change.
**Settings → Reset local pool data** restores the previous profile first and
then removes local pool data; named snapshots are kept.

This mode never uploads account or provider secrets to Zenith production
systems. Moving an owned secret to **My server** is a separate, explicit,
confirmed action.
