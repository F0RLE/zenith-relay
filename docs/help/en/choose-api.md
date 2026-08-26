# Choose API

> ChatGPT connects directly to a hosted OpenAI-compatible API. Relay does not
> create a personal pool or keep Relay-side request history in this mode.

Use this mode when you already have a provider endpoint and inference API key.
Use **This computer** or **My server** when you want to route your own
ChatGPT accounts.

## Quick navigation

- [Before you start](#before-you-start)
- [Connect an API](#connect-an-api)
- [What you can see](#what-you-can-see)
- [If the connection fails](#if-the-connection-fails)
- [Keys and recovery](#keys-and-recovery)

## Before you start

- A provider API key for inference requests.
- The compatible API root, normally ending in /v1.
- A model supported by that provider.
- **Responses API** support when you want the direct **Connect ChatGPT** action.

The server management token used by **My server** is not an inference API key.

## Connect an API

1. Switch the mode picker to **Choose API**.
2. Open **Connections → Sources** and choose **Add source**.
3. Select a known provider or **Custom API**.
4. Enter the source name, API root, provider key, and protocol. Select
   **Responses API** for direct ChatGPT/Codex profile attachment.
5. Save and wait for model discovery when **Automatic** model mode is selected.
   Open the source menu and choose **Connect ChatGPT** when the source has a
   native Responses route.
6. Confirm the profile backup. The provider continues serving requests after
   Relay closes.

Chat Completions sources can be saved and used through their compatible
endpoint, but their direct **Connect ChatGPT** action stays unavailable. A
Messages or bridged route is not silently treated as a native Responses route.

## What you can see

**Connections** stores the source and its non-secret settings. **Overview**
shows models, balance, spend, and request counters when the provider exposes
them. **Pool**, **API & ChatGPT**, and **Usage** are hidden because the
provider owns routing and request history in this mode.

## Prices and model checks

Relay uses provider-reported model prices first, then a verified global catalog,
then an explicit source override. A custom price changes local API-equivalent
estimates only; it does not change the provider invoice.

In **Automatic** mode, the source model list is refreshed when you save or
manually refresh it. In **Manual** mode, enter model IDs when `/models` is
unavailable; Relay keeps that list and does not background-probe it. Manual
models are an operator assertion, so verify the model and protocol with the
provider and expect unsupported requests to fail normally. Reasoning modes are
provider catalog metadata or manual pool rules; Relay does not issue a
separate reasoning probe, and a manually allowed effort may still be rejected
by the upstream.

## If the connection fails

| Symptom | What to check | Action |
| --- | --- | --- |
| 401 Unauthorized | Key, expiry, and issuing service. | Create or enter a new provider key. |
| 404 or no models | API root and protocol. | Remove a model-specific path and verify /v1. |
| 429 Too Many Requests | Balance and provider rate limit. | Wait, add balance, or choose another source. |
| **Connect ChatGPT** unavailable | Source protocol binding. | Use a native Responses API route. |
| Model listed but rejected | Provider support for that model and request shape. | Check the provider's documentation. |
| Overview has no balance | Provider statistics support. | Use the provider dashboard; the key may still work. |
| ChatGPT uses an old endpoint | Active profile. | Connect the intended source again. |

## Keys and recovery

The API key is stored in the operating system credential store and is not
shown again in full. It does not appear in snapshots, diagnostics, exports, or
usage records.

To return to a previous ChatGPT profile, switch to **This computer**, open
**Recovery**, and restore the automatic return point or a named snapshot.
