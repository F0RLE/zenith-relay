# Zenith API

> ChatGPT connects directly to a hosted OpenAI-compatible API. The account pool
> and Zenith Relay local API are not involved in this mode.

**Use it for:** an existing API endpoint and client key that must work without
keeping Zenith Relay open.

**Do not use it for:** combining your own ChatGPT accounts into a pool. Use
**This computer** or **My server** instead.

## Quick navigation

- [How this mode works](#how-this-mode-works)
- [Before you start](#before-you-start)
- [Connect an API](#connect-an-api)
- [Verify the setup](#verify-the-setup)
- [Pricing and statistics](#pricing-and-statistics)
- [If the connection fails](#if-the-connection-fails)
- [Keys and recovery](#keys-and-recovery)

## How this mode works

**Connections** stores hosted API sources. The selected source is written
directly to the ChatGPT profile, so the desktop application may be closed after
setup.

This mode has no **Pool**, **API & ChatGPT**, or **Usage** pages. The provider
owns routing, availability, and request logs. **Overview** displays only the
balance and counters exposed by the selected provider.

## Before you start

- A base HTTPS URL for an OpenAI-compatible API.
- A client API key issued by that service.
- The required model list, or an endpoint capable of returning it.
- Current provider prices when they differ from the official catalog.

A Zenith Relay server management token is not valid here. This mode requires a
**client key for inference requests**.

## Connect an API

1. Switch to **Zenith API**.
2. Open **Connections** and select **Add API**.
3. Choose a known provider profile or **Custom API**.
4. Enter a clear name, base URL, and client key.
5. Save the source and wait for model discovery.
6. Open the saved source menu and select **Connect ChatGPT**.
7. Confirm the profile backup before switching.

> Do not enter a model-specific path as the base URL. Clients normally expect
> the compatible API root ending in `/v1`.

## Verify the setup

1. The source in **Connections** must not show a validation error.
2. Select it in **Overview** and inspect the discovered models.
3. Send a short request from ChatGPT.
4. If supported by the provider, refresh **Overview** and compare request count
   or spend.

> **Ready means:** ChatGPT responds with the selected model after Zenith Relay
> has been closed.

## Pricing and statistics

- Balance and spend come from the provider account or API when supported.
- A missing balance with successful requests can mean the provider has no
  statistics endpoint; it does not automatically mean the key is invalid.
- Set custom prices to your actual purchase cost. They affect local economics,
  not the provider invoice.
- OpenAI models use input, output, and cache-read prices. Cache-write fields
  apply only to model families that bill them.

## If the connection fails

| Symptom | Check | Action |
| --- | --- | --- |
| `401 Unauthorized` | Key value, expiry, and issuing service | Create or enter a new client key |
| `404` or no models | Base URL and API compatibility | Remove model-specific paths and verify `/v1` in provider docs |
| `429 Too Many Requests` | Balance, quota, and provider rate limit | Add balance, wait for reset, or select another source |
| Model is listed but rejected | Protocol supported by that model | Check Responses, Chat Completions, or Messages support |
| Overview has no balance | Provider statistics support | Use the provider dashboard; re-adding the key is unnecessary |
| ChatGPT still uses the old endpoint | Active profile | Select **Connect ChatGPT** again on the intended source |

## Keys and recovery

The API key is stored in the operating system credential store and is not shown
again in full. Only its name, endpoint, models, and non-secret settings remain
visible.

To return to the previous ChatGPT profile, switch to **This computer**, open
**Recovery**, and restore the automatic return point.
