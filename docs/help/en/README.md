# Zenith Relay

Zenith Relay is a personal desktop relay for ChatGPT accounts and compatible
API connections. It puts the connections you choose behind one private
OpenAI-compatible endpoint, then lets the ChatGPT client or another client use the
pool.

This guide follows the application from left to right. The same guide is shown
inside **Help**. The [Russian version](../ru/README.md) is also available.

## 1. Overview

**Overview** is the place to check whether the relay is ready before sending a
request. It shows the current runtime, active endpoint, healthy pool capacity,
models, errors, request count, and speed. Its charts can be viewed by day, week,
or month; the selected window is explicit and never silently changes.

Use the scope control to compare the whole pool, one account, or one API source.
When a provider exposes spend or balance data, Relay displays it as provider
data. When it does not, the connection can still work; absence of statistics is
not a failed connection.

## 2. Connections

**Connections** is where credentials and upstream endpoints are added. You can
sign in to ChatGPT, import an existing account file, add a compatible API
source, configure a proxy, or connect to a Relay Server that you operate.

After adding a connection, wait for its status and model list to settle. A
provider may expose models automatically, or you can enter model IDs manually
when its discovery endpoint is unavailable. A manual list is a local assertion:
the provider still decides whether a request is accepted.

Quota windows, expiry dates, reset times, and reset credits come from the
provider. Relay does not replace them with a fixed five-hour or weekly formula.
If a weekly reset credit is reported as available, the account shows **Reset
weekly quota** and asks for a simple Yes/No confirmation. In local mode,
**Connections → Automations** can perform that reset automatically when the
weekly window reaches zero.

If you bought an account, enter its purchase price in the account settings.
Relay compares completed, priced requests with that cost and shows:

- **API equiv. used** — the value of requests already observed by Relay at API
  prices;
- **API equiv. left** — an approximate value only when the provider window,
  prices, and Relay usage are complete enough to estimate it;
- **Payback** — the used API-equivalent value compared with the purchase cost.

These are estimates for comparison, not money held by Relay and not a provider
invoice. Activity outside Relay is not included.

## 3. Pool

**Pool** decides which connections may receive traffic. Add or remove accounts
and API sources, enable or disable individual models, and arrange the order in
which eligible members are tried. A disabled, expired, unhealthy, out-of-quota,
or excluded member remains visible for repair but is not selected.

The **Model Rules** table is also where model behavior is controlled:

- **Reasoning** lists the modes supplied by the backend catalog. Relay passes
  the selected mode through; it does not invent a mode or send a separate probe
  request for each one.
- **Request speed** is the standard/fast service-tier choice for a route. Fast
  is shown only when that route confirms `fast` or `priority`; it is a speed
  request, not another quota or another model.
- **Price** is the provider or verified catalog price, with an explicit local
  override when needed. It is used for API-equivalent estimates and does not
  alter the provider invoice.

The table preserves the provider's model order. Relay applies pool rules and
health checks around that order; it does not silently alphabetize or replace
the provider catalog.

Adapters are explicit. A native route keeps its original request format. A
Responses source may be assigned to a Messages or Gemini bridge when that
route is configured. The bridge translates the request and response; it does
not make an unsupported upstream protocol native. A model is sent only through
the format assigned to it.

## 4. API

The **API** tab controls the endpoint that clients use. In local mode, start
the API and copy the displayed address, normally:

    http://127.0.0.1:14998/v1

Keep Relay open while the local pool is serving requests. If you use a
Relay Server that you operate, the server keeps the endpoint running after the
desktop app closes.

Use **Copy API key** when a compatible application needs the credential. Relay
fetches it only for that action and copies it directly to the clipboard; it is
never rendered in the application.

Use the reissue action next to it only when the key may have been exposed. The
previous key stops working after the replacement is copied.

The **Application** tab configures ChatGPT separately from the API. Its
connection creates a protected recovery point before changing the selected
profile, and can be returned from **Recovery**. The ChatGPT client can
use every enabled, compatible model in the pool through the endpoint, not only
models from the ChatGPT family. Reasoning, request speed, price rules, and pool
order still come from **Pool**.

The ChatGPT application's WebSocket preference is separate from the provider transport.
If WebSocket is unavailable, Relay can use its supported HTTP route; a client
flag does not turn an upstream HTTP provider into a WebSocket provider.

## 5. Usage

**Usage** is the request history for the current runtime. It opens on the
complete available period; shorter daily, weekly, or monthly views are filters,
not data deletion. The request table records status, model, selected pool
member, protocol, reasoning mode, service tier, first/total timing, generation
speed, token totals, cache reads/writes, and API-equivalent value.

Open a row for its route and error source. **Provider** means the upstream
service rejected or failed the request. **Account** means sign-in, quota, or
account state needs attention. **Relay** means local configuration or protocol
handling needs attention. Request and response text, raw headers, cookies, and
secrets are not stored.

## 6. Recovery

**Recovery** keeps local ChatGPT profile snapshots. Relay creates an automatic
return point before a managed profile switch. You can also create a named
snapshot before a risky change and restore it after confirmation.

A snapshot restores the profile files that it contains, including sign-in and
client settings. Relay refuses to overwrite a newer manual sign-in. The local
pool reset first attempts to restore the previous profile, then removes local
accounts, sources, settings, and usage; named snapshots stay available.

Account export is different from a snapshot: it is a credential-bearing transfer
file. Treat it as a secret, use it only for the intended import, and delete it
afterwards.

## 7. Errors

Start with the **Error source** and the status on the affected card or request.
The following groups cover the normal failures.

| Area | Typical message | What to do |
| --- | --- | --- |
| Account | Sign-in required, token expired/revoked, invalid grant | Sign in to that account again; do not delete a healthy fallback. |
| Account | Subscription expired, workspace disabled, forbidden | Check the account plan or provider access and remove it from the pool until fixed. |
| Account | Quota exhausted or reset pending | Wait for the provider reset, use an available reset credit, or let the weekly automation run. |
| Account | Checkpoint, captcha, verification, or proxy unavailable | Complete the provider check or repair the proxy, then refresh the account. |
| Account | Credentials unavailable, malformed response, connection timeout | Restore the protected credential or network path and refresh; the pool will not select it while unavailable. |
| Provider | 401/403 unauthorized or forbidden | Check the API key, endpoint, permissions, and provider plan. |
| Provider | 404, no models, or model not found | Check the API root and protocol. Remove a model-specific path; use a verified manual model ID only when discovery is unavailable. |
| Provider | 429, overloaded, unavailable, or 5xx | Wait for recovery or enable another eligible source; repeated failures cool down that exact route. |
| Provider | Timeout, incomplete stream, invalid request, unsupported tools or region | Check the provider's supported request shape, model, region, and limits. |
| Pool | No eligible pool member | Enable a member, add the model to its rules, repair its account/source, or add a compatible fallback. |
| Pool | All members cooling down or out of quota | Wait for retry/reset or change the pool membership/order. |
| Pool | No compatible adapter or route | Assign the model to the correct native or bridge format in **Model Rules**. |
| Relay | API stopped, wrong address, profile switch failed, local data unavailable | Start the endpoint, reconnect the displayed address, restore a recovery point, or inspect the local error details. |

Relay can retry a different eligible member only before response data reaches the
client. Once a response has started, its owner stays fixed so the client does
not receive two different answers in one stream.

When asking for help, copy the sanitized status, HTTP code, model, and **Error
source**. Never include API keys, cookies, tokens, prompts, or provider bodies.
