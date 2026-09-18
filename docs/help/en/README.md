# Zenith Relay

Relay connects ChatGPT, OpenCode, and other compatible applications to your
accounts and API providers. A pool combines several connections behind one
address: the application sends a request, and Relay chooses who will serve it.

[How Relay works](#how-relay-works) | [Quick start](#quick-start) |
[Overview](#1-overview) | [Connections](#2-connections) | [Pool](#3-pool) |
[API](#4-api) | [Usage](#5-usage) | [Recovery](#6-recovery) |
[Settings](#7-settings) | [Errors](#8-errors)

## How Relay works

- **Connections** stores your accounts, API addresses, and provider keys.
- **Pool** selects which connections may serve requests and which models each
  may use. A member can be an account or an API provider.
- **API** gives the application the pool address and a key to access it.

The application chooses a model. Relay finds members allowed to serve that
model in the requested format, then applies the rotation mode. An alternative
must support the same request; a similarly named model is not a replacement.

Saving a connection, including it in the pool, and connecting an application
to the pool are separate actions. Adding a connection can also include it in
the pool, but check its membership in **Pool**.

### Operating modes

- **Computer**: the pool runs on this device. Relay and its API must keep
  running to serve requests. Use this for multiple personal accounts and APIs.
- **Choose API**: the application connects directly to a selected external
  API. **Pool**, **API**, and **Usage** are hidden, and Relay rotation does not
  apply. Overview shows the selected provider's data when it is available.
- **On your server**: the pool runs in your Relay Server. The desktop application
  manages it and displays its data; closing Relay does not stop the server.
  Available actions depend on the connected server's capabilities.

Changing modes does not transfer accounts or secrets. Moving accounts to your
own server is a separate action with confirmation.

### Quick start

1. Select **Computer**.
2. In **Connections**, sign in to ChatGPT, import your account, or add an API
   provider. Wait for the connection check and model list.
3. Add the connections you need in **Pool**. Allow the required model both in
   the member's policy and in **Model Rules**.
4. In **API**, select **Start API** if it is not already running.
5. Return to **Pool → Connect** and choose ChatGPT or OpenCode. For another
   compatible application, copy the address and API key from **API**.
6. Send a request from the application. **Usage** will show its model, member,
   and result.

**Repeat quick setup** in **Help** opens the setup wizard again. To connect
directly to one provider, use **Choose API** or the API source's **Launch**
action instead of connecting to the pool.

## 1. Overview

**Overview** shows the selected mode's address, available models, members,
requests, and speed. Select the chart period and scope separately. Statistics
for an individual account or API apply only to that connection.

### Balances and estimates

An API may report **Balance**, **Key remaining**, and **Plan remaining**
separately. These are different limits: **No limit** on a key does not mean
an unlimited account balance. Values retain the provider's currency or units;
different currencies are not added together.

Use **Refresh balance** on a pool card or **Refresh** in the selected API's
overview to request fresh data. **No balance API** means Relay could not find
a supported way to read statistics; **Stats access denied** means access
to those statistics was refused. Model requests may still work in either
case. A failed refresh retains the last amount with **Not refreshed**.

**Relay estimate** and **API equiv. used** are calculated from requests
seen by Relay and available prices. They estimate usage value, not a balance
or a confirmed provider charge. **API equiv. left** appears only
when weekly-window and usage data are sufficient. **Payback** compares used
API-equivalent value with the account's purchase cost. Activity outside Relay
is not included in these estimates.

## 2. Connections

### Accounts and quota

In local mode, add an account by signing in through the browser or importing
your own file. **Connections** also manages proxies. A saved account does not
have to participate in the pool.

**Refresh** checks the provider's account state and quota. It does not add
quota or reset its window. The provider defines the window length, remaining
allowance, and reset time. Relay cannot infer a date that was not reported.
A subscription end date alone does not prevent rotation: actual access,
sign-in state, quota, and account errors determine availability.

Request **Credits** and **reset credits** are different. Fresh positive request
credits can allow work even when percentage quota is exhausted. A reset credit
only allows a separate reset operation. **Reset weekly quota** appears for a
local account with an available credit, asks for confirmation, and consumes
that provider credit.

When an account requires sign-in, sign in to that account again. **Force-refresh
sign-in** checks whether its saved session can be refreshed; it cannot replace
a revoked sign-in with a new one. Other eligible pool members can keep working.

An account export contains sign-in credentials. Treat it as a secret file.
It is different from a pool preset, which contains settings only.

### API sources

Add a name, API address, and provider key. Use the provider's API address, not
a dashboard URL. A full endpoint such as `/v1/messages` also supplies a format
hint. Relay reads the model catalog and declared endpoint support. In local
mode, model IDs can be entered manually when discovery is unavailable;
a manual entry does not add model support at the provider.

The API source editor contains:

- **General**: name, address, key replacement and the detected formats.
- **Configure adapters**: automatic or manual routing. Manual routing assigns
  each model's available provider formats separately for each application format.
  Several paths to one model are allowed. Conversions run through the pool.
- **Pricing**: prices for usage estimates; see the pool policy section below.

New sources use **Automatic**. Known service settings, an explicit endpoint,
or provider declarations can supply routes. A model list alone is not proof of
generation support: an unknown provider keeps its catalog while waiting for a
check or manual routing. Existing sources retain their saved manual routes.
**Check generation** sends one small synthetic text request for the selected
model and provider format; it can spend quota. It verifies text generation only,
not tools, images or every reasoning level. Refreshing the catalog does not
generate. Save changed address/key settings before checking; old results then
become invalid. Authentication errors, timeouts and server errors leave support
unknown. A 404/405 marks only the tested model/format unsupported.

**Launch** on a source connects the chosen application directly to that API.
Those requests bypass the pool, its rotation, and Relay's usage history.
ChatGPT/Codex direct connections require native Responses. OpenCode uses the
source's native Responses, Chat Completions, Messages or Gemini SDK. A model
in the source inventory does not by itself establish client compatibility.

### Automations

In local mode, **Connections → Automations** offers two conditions:

- **After primary quota recovery**: a small request to the selected model after
  the window recovers, to start its next reset countdown when the provider
  uses that mechanism. This request consumes quota. Choose automatic execution
  or manually run ready checks.
- **Automatic weekly quota reset**: when the weekly window is exhausted, Relay tries
  to use an available provider reset credit. It does not need a model request.
  Enabling the automation authorizes subsequent resets without confirming
  each one separately.

Select the accounts and enable the task. Local automations run while Relay
is running; server automations depend on Relay Server capabilities.

### Your server

Save the Relay Server address and management token in the server connection
tab. The token lets the desktop application manage the server. Model clients
need its API address and a separate request key; these credentials have
different purposes.

Moving an account to the server is a separate confirmed action. After a
successful move, it participates in the server pool. Its local record remains
for recovery and does not receive local requests.

## 3. Pool

### Members and route selection

Add saved connections in **Members** and open their policies. Removing a
member from the pool does not require deleting it from **Connections**.
Disabled members and accounts requiring sign-in, denied access, or without
available quota are skipped. Their presence does not block other members.

Eligibility is checked for the specific model and request format. A working
pool can therefore lack a route for one model. A temporary restriction on a
single model does not necessarily block the member's other models.

**Pool rotation** has three modes:

- **Smart** considers current load, fresh quota, and recent failures. Similarly
  suitable members share requests according to their request shares. Manual
  order does not affect selection. Cache affinity is considered only among
  similarly suitable members.
- **In order** chooses the first eligible member with a free slot in your
  list. If it is unavailable or at its concurrency cap, Relay checks the next
  members. New requests return to it when it recovers. Only this mode lets
  you reorder members manually.
- **Round robin** distributes new independent requests among eligible members
  according to their request shares. Equal shares alternate; unavailable
  members and those at their concurrency cap are skipped.

In every mode, a chat continuation may need its previous member. Rotation does
not promise a different account for every message in the same conversation.

**Request share** is a ratio for Smart and Round robin. For example, 2 and 1
give roughly two parts of traffic to the first member and one to the second
when they are equally available. It is not a percentage, requests per second,
or extra quota. **Concurrent requests** limits how many requests one member
can serve at a time, across its models and formats. A value of 2 allows two
simultaneous requests. **Unlimited** removes Relay's additional cap; provider
limits still apply.

Cards group members by readiness, quota wait, unavailability, and disabled
state. In the rotation dialog, **In order** preserves the manual queue even
when states change; automatic modes show ready members first.
The current, last-used, and **Next candidate** indicators have different
meanings. Next candidate appears only when the choice for a new text request
agrees across enabled models and formats. An absent hint does not mean the
pool has stopped.

### Failures and retries

Recovery is automatic. Relay tries every compatible member before returning a
candidate failure. A temporary failure pauses that route for at least 5 seconds;
healthy alternatives can serve the request immediately. If needed, Relay makes
one recovery pass within 30 seconds. A repeated failure pauses the route for
60 seconds, then doubles the delay up to 30 minutes. Even the last candidate
observes the pause. A successful request resets its failure count.
Provider retry/reset times and longer member recovery delays remain mandatory.
Sign-in failures, missing model access, and exhausted quota move directly to
another eligible member. A model-specific failure leaves other models usable.

After a pause, one request checks the member's recovery. Others use another
eligible route or wait up to 30 seconds for the check or a free slot.
ChatGPT has a separate longer wait option in
**API → ChatGPT → Wait for route recovery**.

Relay can retry with another member before response data reaches the
application. It does not combine an already-started answer with another
provider's output. Moving a conversation continuation requires sufficient
saved history. A response reference or tool result without its required
context may need the original member; Relay does not silently discard that
context. Invalid requests, such as excess context or an invalid tool call,
cannot be repaired by trying more members.

### Models and member policies

In **Pool member policy → Models**, a switch allows the model for that
specific account or API. It is permission, not a quota indicator. Search and
expandable groups help locate models.

The pool's **Model Rules** tab enables or disables a model for the whole pool.
It also controls model and group order, available reasoning modes, and
per-model speed. Model order affects catalog presentation; member order in
rotation controls connection selection. The application's model list is also
limited by client compatibility.

**Request speed** offers **Standard**, **Fast**, and **Ultrafast**. Set the
pool preference above the members or save a preference for an individual
model. Faster modes require confirmed support from the chosen model and
source. If Ultrafast is unavailable, Relay uses supported Fast or Standard.
This is a processing preference, not a response-time guarantee, extra quota,
or another rotation mode.

### Prices and additional settings

API prices are in **Pool member policy → Pricing** and in the source editor
under **Connections**. Estimates use provider prices first, then matching
catalog prices, then manual prices if neither is available. A manual price
does not change the provider's tariff or unconditionally override other prices.

Token prices are in USD per million tokens. A manual set requires input and
output prices; the reset button removes that manual set. The 5-minute and
1-hour cache-write fields appear for applicable models and formats. They price
cache creation with those lifetimes; they do not enable request caching.

An account's **Settings** includes **Drain** and **Purchase
cost, USD**. The first stops new assignments; the second is only for the
payback estimate. An API source offers an automatic or manual failure recovery
delay. It cannot shorten a provider's mandatory retry delay. Select **Save
policy** to apply the dialog's changes; **Cancel** discards them.

**Save preset** and **Apply preset** transfer membership, rotation, and model
settings. A preset contains no keys, sign-ins, actual quota balance, or request
history. Relay previews changes and matches members to existing connections
before applying it.

## 4. API

### Application address and key

The **API** tab in this section shows status, address, and start controls.
Copy the displayed address. The usual local address is:

```text
http://127.0.0.1:14998/v1
```

**Copy API key** copies Relay's request key. It is different from an external
provider key or a server management token. **Reissue API key** immediately
invalidates the old key; update your clients afterward. Simply copying the
key does not change it. After changing the port, update the address or connect
the application again.

All four formats use the same pool request key and model permissions:

| Format | Local endpoint | Authentication |
| --- | --- | --- |
| Responses | `http://127.0.0.1:14998/v1/responses` | Bearer key |
| Chat Completions | `http://127.0.0.1:14998/v1/chat/completions` | Bearer key |
| Messages | `http://127.0.0.1:14998/v1/messages` | `x-api-key` or Bearer key |
| Gemini | `http://127.0.0.1:14998/v1beta/models/{model}:generateContent` | `x-goog-api-key` or Bearer key |

Gemini streaming uses `:streamGenerateContent?alt=sse`. Replace the host/port
with the displayed server address when using a remote pool. In **Pool → Model
Rules**, the route icon shows compatibility for each application format,
feature status and route-specific reasoning levels. "Not verified" is not a
promise of support. Unsupported conversion options fail before generation;
native routes preserve provider-specific parameters. Realtime, cross-format
WebSocket, audio/video conversion and server-side tool emulation are not offered.

### ChatGPT

In the **ChatGPT** tab, **ChatGPT account** chooses the sign-in used by the
application interface: automatic, a selected account, or **Without account**.
**Switch** applies that choice and restarts ChatGPT. This is separate from
rotation: model requests through the pool address still use eligible pool
members.

**Keep 1% reserved** keeps the selected account's last percent for launching ChatGPT
directly by limiting its use through the pool. It is not a reserve for every
member or a balance setting for API providers.

- **ChatGPT background tasks** allows automatic activity summaries and task
  titles. These can issue separate requests.
- **WebSocket for ChatGPT** controls the client's connection method. Enabling
  it may restart ChatGPT and interrupt an active request. It does not add
  WebSocket support to an external provider.
  A catalog with converted Responses routes uses HTTP/SSE automatically.
  Native Responses routes keep WebSocket support when the catalog permits it.
- **Wait for route recovery** holds recognized ChatGPT requests when eligible
  members are temporarily unavailable, until recovery or cancellation.
  Without it, the client receives an error after normal attempts. Waiting
  cannot fix invalid requests or remove continuation-history requirements.

For a server pool, the ChatGPT tab connects to your server when that capability
is supported.

### OpenCode

In local mode, use **Pool → Connect → OpenCode**. Relay prepares the connection
and compatible model catalog. To use one API directly, select **Launch** on
that source. Models are grouped by native protocol under matching SDKs.
Working connection/model IDs, the selected model and user options are preserved.
Catalog refresh updates Relay-owned groups only while their address, key and
SDK remain unchanged; it does not restart OpenCode. Recovery retains the
original configuration. **API → OpenCode** is currently marked **In development**;
its dedicated integration settings panel is not implemented yet.

## 5. Usage

This section shows requests through the selected local or server pool.
Direct API calls and account activity in other applications are not included.
Changing the period filters the display without deleting records.

Open a request to inspect its model, member, format, time to first output,
total time, tokens, and estimated cost. Requested reasoning and speed can
differ from the values sent upstream; check their respective fields.
Unknown measurements do not mean zero cost.

Failed requests show **Error origin**, Relay's category, and the saved
provider code, HTTP status, and redacted message separately. Older records
may not have a provider message. The journal does not store request or
response text or secrets.

## 6. Recovery

This section is available in **Computer** mode and manages client
application configuration on this device.

**ChatGPT snapshots** are named copies of configuration and sign-in state.
Restoring a selected snapshot replaces current settings and sign-in with its
contents. This is an explicit return to that saved state, so check the
snapshot's name and date.

The automatic backup made before connecting ChatGPT to Relay is separate.
Restoring Relay-managed settings preserves unrelated settings and refuses to
overwrite a newer manual sign-in. This differs from explicitly restoring a
complete named snapshot.

OpenCode keeps an original configuration snapshot, created manually or before
the first connection. It remains until restored; reconnecting does not build
a history of new snapshots. Restoring it and resetting all Relay data are
different actions.

## 7. Settings

**Appearance** controls language and theme. Help follows the selected language.
**Application** shows the version, update controls, and the working data
folder for this device.

In **Pool data**, **Remind me about the restore point** controls the confirmation
before switching ChatGPT. The protected automatic backup is still created
when this reminder is off.

Enable **Debug mode** to investigate a problem. **Diagnostics** then provides
access to error, crash, and operation-stage logs. Errors and crashes are also
recorded without debug mode; detailed operation stages are for troubleshooting.

**Reset local pool data** first attempts to restore Relay-managed ChatGPT
settings safely, then removes local accounts, sources, settings, and usage.
If restoration is unsafe, deletion does not proceed. Fixing a single
connection does not require resetting the entire pool.

## 8. Errors

<!-- relay:error-reference -->

Open the affected card's error or the request in **Usage**. Read **Error origin**,
Relay category, provider code, and HTTP status together. For example,
`invalid_api_key` can refer to the pool key or an external provider key.
Fix the connection named in the details.

This reference covers Relay codes and recognized failure cases. External
providers may return other messages; their redacted details remain available.
`exceeded retry limit` means the client exhausted retries: the last error,
such as 429, explains why. More retries do not replenish quota.

### Routes, pool and access

| Code or message | Cause | Action |
| --- | --- | --- |
| `no_eligible_source` · 503 · "no eligible source is available for this model" | No member currently qualifies for this model and format. | In **Pool**, check membership, pool and member model rules, sign-in, quota, pauses, and draining. Wait for busy members or add a compatible reserve. Test a new independent request and inspect its route in **Usage**. Removing unhealthy accounts is unnecessary for rotation. |
| `exceeded retry limit`, `unexpected status` | The client reports a failed request or exhausted retries. | Check the last HTTP status and code in **Usage**. For 429, distinguish quota exhaustion from request frequency; for 503, inspect available routes. Resolve that cause before retrying. The retry limit itself is not the cause of a provider rejection. |
| `all_sources_cooling_down`, `all_candidates_cooling_down` · 429 | Eligible routes are paused after rate limits. | Wait for `Retry-After` or the next attempt time. Reduce concurrent requests. A reserve must support the same model and format. |
| `all_sources_temporarily_unavailable` · 503 | Eligible members are temporarily unavailable after failures. | Inspect each member's last error and wait for recovery. Fix network failures; follow the quota instructions for exhaustion. |
| `model_not_found` · 404 | The model is absent from the pool's available catalog. | Refresh models in **Connections**, check the exact ID and both sets of **Pool** model permissions, and confirm client format compatibility. |
| `invalid_api_key` · 401 from Relay | The client uses an invalid or old pool key. | Copy the current address and key from **API → API**, or reconnect the application. Rotating the key invalidates its predecessor. |
| `client_api_not_allowed` · 403 | The client key does not permit this API format. | Connect through the intended client profile and use an allowed endpoint. A server management token is not a `/v1` request key. |
| `invalid_host` · 400 | The local API received an unsuitable Host. | Use the address shown in **API** without a proxy rewriting Host. Use your Relay Server for access from another device. |
| `gateway_stopped`, `gateway_unavailable`, `runtime_unavailable` | The API is stopped, unreachable, or not ready. | Start the API in the selected environment and check the address/port. Choose a free port if occupied, then reconnect the client. For a server, check its process, network, and HTTPS. |
| `codex_background_blocked_activity_summary`, `codex_background_blocked_task_title` | A ChatGPT background request was disabled. | Enable **API → ChatGPT → ChatGPT background tasks** when summaries/titles are wanted. This is not a main-request failure or exhausted quota. |

### Quota, rate limits and provider permissions

| Code or message | Cause | Action |
| --- | --- | --- |
| `upstream_quota_exhausted`, `insufficient_quota`, `quota_exhausted` · 429 | The provider confirmed exhausted quota, credits, or a spending cap. | For accounts, wait for the quota window or use an available reset. For APIs, check wallet and key spending limits in the provider dashboard. Add a compatible reserve; refreshing statistics does not replenish quota. |
| `upstream_rate_limited`, `rate_limit_exceeded` · 429 | Request frequency or concurrency is too high. | Respect the stated delay, reduce simultaneous tasks or the member's concurrent request limit, and avoid immediate retry loops. |
| `upstream_usage_not_included`, `usage_not_included` · 403 | The plan does not include this capability. | Choose an entitled model or connection. A normal quota reset does not grant a new capability. |
| `upstream_unauthorized` · `invalid_api_key` from a provider | The provider rejected authentication. | Update the external API key in **Connections**, then refresh models. Sign in again for an account. Changing the pool key does not fix provider authorization. |
| `upstream_account_disabled`, `account_deactivated` · 403 | An account, workspace, or project is disabled. | Check its provider dashboard and restore access through that provider. Use another permitted member while it is unavailable. |
| `upstream_account_verification_required`, `account_verification_required` | Account verification is required. | Complete verification on the provider website, then refresh sign-in and account data in Relay. |
| `upstream_forbidden`, `permission_denied` · 403 | The account or key lacks permission for the operation. | Check project, key, and model permissions. Reading a model list does not prove inference access. |
| `upstream_region_unsupported`, `unsupported_country_region_territory` | The provider does not serve the connection's region. | Check the provider's supported regions and permitted network configuration. Use a connection available in your region. |
| `upstream_edge_challenge`, `edge_security_challenge` | An edge security check replaced the API response. | Verify the API address, service status, and network configuration. Ask the provider for supported API access; signing in to Relay again cannot resolve its edge challenge. |
| `upstream_model_not_found` · provider `model_not_found` | This provider does not expose the requested model ID. | Refresh this source's models and verify key access. Remove an obsolete permission or select an actual available ID. |
| `upstream_model_unavailable`, `model_not_available` · 503 | This provider cannot serve the model temporarily. | Relay pauses this model on the failed route and tries another compatible member. Recovery observes the provider delay. |
| `upstream_model_unsupported`, `model_not_supported` | The route does not support this model. | Check **Connections → API → Formats and adapters** and use a verified format and compatible member. |
| `upstream_model_capacity`, `model_at_capacity` | The model is temporarily overloaded. | Wait or use another compatible source. Signing in again does not increase provider capacity. |

### Requests, history and tools

| Code or message | Cause | Action |
| --- | --- | --- |
| `invalid_request`, `upstream_invalid_request` · 400 / 422 | The body or a request parameter is invalid. | Fix the field named in the redacted message. Requests need a JSON object, nonempty model, and valid `stream`; path/body models must agree. Compact responses do not support streaming. |
| `upstream_context_too_large`, `context_too_large`, `context_length_exceeded` | History exceeds the model context. | Shorten history/attachments, summarize, start a new conversation, or choose a model with a larger context. |
| `request_too_large`, `upstream_payload_too_large` · 413 | The request body or attachments exceed a size limit. | Reduce or split input files. Relay's incoming image-request limit is 64 MiB; the provider may impose a smaller one. |
| `upstream_instructions_required`, `missing_required_parameter` | A required field, including instructions, is absent. | Supply the field named by the provider or update the client generating it. Retrying the same body does not fix it. |
| `upstream_unsupported_request`, `unsupported_request` | A parameter or capability is unsupported. | Disable the named parameter, tool, or mode and use a compatible format. |
| `upstream_content_policy`, `content_policy_violation` | Provider content rules rejected the request. | Revise the request according to the service rules. Rotating members is not a correction for that request. |
| `response_continuation_unavailable`, `response_affinity_miss` | The response owner is unavailable and full replay history is missing. | Restore the original account/API or resend complete history from the client. Start a new conversation if history is lost. A rotation mode change cannot restore context. |
| `upstream_previous_response_not_found`, `previous_response_not_found` | The provider no longer knows the previous response. | Resend full history without the stale response reference, or start a new conversation. Do not transfer just a response ID to another API. |
| `upstream_tool_call_mismatch`, `tool_call_not_found` | A tool result has no matching call, or a call has no result. | Update the client and retry with the complete call/result pair. Do not remove only half of the pair. Start a new conversation if history is damaged. |
| `upstream_encrypted_content_invalid`, `invalid_encrypted_content` | Stored encrypted reasoning context is not accepted. | Return to the original connection or start a new task with ordinary history. Do not manually edit encrypted blocks. |
| `tool_use_not_supported`, `chat_feature_not_supported` | This route cannot represent the requested tool or feature. | Function tools and their results are supported on compatible Chat Completions routes. Check the model's format capabilities; choose a matching native route or remove the specifically unsupported option. |
| `upstream_conflict`, `conflict` · 409 | State changed or another operation is in progress. | Wait for the earlier operation, refresh state, and retry once. Restore conversation history when the conflict concerns continuation. |
| `upstream_candidate_rejected`, `source_rejected` | A route rejected the request without a more specific category. | Read the provider code/message and check model, permissions, and format. A new independent request can use a compatible reserve. |

### Adapters and images

| Code or message | Cause | Action |
| --- | --- | --- |
| `adapter_binding_unsupported`, `source_protocol_invalid`, `source_pool_protocol_unsupported` | The format/adapter binding is incompatible or unverified. | Choose supported **Formats and adapters** in the API editor, check and save them, then add the source to the pool. |
| `adapter_invalid_request` | The adapter cannot translate the request. | Remove the field named in the message or choose a native-format source. |
| `adapter_parameter_unsupported` | A meaningful request parameter has no lossless mapping on the selected route. | Use a native route or remove the unsupported option. Relay does not silently discard it. |
| `adapter_compaction_unsupported` | The adapter cannot perform this compaction operation. | Use a native Responses route for compaction or start a new task with summarized ordinary history. |
| `adapter_continuation_missing`, `adapter_continuation_mismatch` | Adapter continuation state is lost or belongs to another binding. | Restore the former source/adapter. Transfer complete history or start a new conversation when it is unavailable. |
| `adapter_tool_unsupported`, `adapter_reasoning_unsupported` | The adapter cannot represent the tool or reasoning mode. | Choose a supported capability or native format. Allowing a mode in model rules does not add upstream support. |
| `adapter_upstream_response_invalid`, `adapter_upstream_stream_invalid` | The provider response violates the adapter contract. | Update Relay and check the adapter. If reproducible, use a native format and report the code and request ID. |
| `invalid_image_model`, `image_generation_not_enabled` | The image model or capability is unavailable. | Check the image model, member inventory, and pool permissions. A normal text model is unsuitable for image endpoints. |
| `image_generation_user_error` | The provider rejected image parameters. | Correct the prompt, size, format, or named parameter. Image edits require at least one nonempty input image. |
| `image_output_missing` | The request ended without the expected image. | Inspect **Usage**, verify provider support, and retry once or use another compatible route. |

### Connections, streams and WebSocket

| Code or message | Cause | Action |
| --- | --- | --- |
| `upstream_transport_connect`, `upstream_transport_request`, `upstream_transport` | The provider connection could not be established or executed. | Check the API address, DNS, internet, certificate, and assigned proxy, then test the connection. Do not disable TLS verification to bypass an error. |
| `upstream_transport_timeout`, `upstream_request_timeout`, `request_timeout`, `upstream_gateway_timeout`, `gateway_timeout` · 408 / 504 | A timeout expired; local WebSocket also waits for initial `response.create`. | Check latency, proxy, and service health. Reconnect a stuck client and retry after a pause. |
| `upstream_transport_body`, `upstream_body`, `upstream_error` | Request/response body transfer failed. | Check network/proxy stability. After output starts, retry is the client's decision; Relay does not combine answers from different members. |
| `upstream_server_error`, `internal_server_error` · 500 | Internal provider failure. | Retry after a pause. If persistent, use another compatible source or contact the provider with the request ID. |
| `upstream_bad_gateway`, `bad_gateway` · 502 | The provider gateway received an invalid response. | Check API status and retry later; use another route for a sustained outage. |
| `upstream_overloaded`, `server_is_overloaded`, `upstream_unavailable`, `service_unavailable` · 503 | The provider is overloaded or unavailable. | Respect the retry delay and check that another eligible member exists. |
| `upstream_not_found`, `not_found` · provider 404 | The API path or resource does not exist. | Verify base URL and path prefix. Restore history instead when the message concerns a previous response. |
| `upstream_status`, `upstream_failure` | No more specific provider classification is available. | Use the actual status and redacted details: 401/403 access, 429 quota/frequency, 5xx service health. An unknown code is not success. |
| `upstream_stream`, `stream_error`, `upstream_terminal` | A provider error event ended the stream. | Follow the embedded provider code in request details. Initial HTTP 200 does not prove successful completion. |
| `stream_invalid` | The stream event format is invalid. | Check the protocol and whether a proxy returned HTML/login content. Update Relay and report the request ID if it persists. |
| `stream_incomplete`, `upstream_websocket_closed`, `upstream_websocket` | The connection ended before completion. | Check network and proxy timeouts. Retry the unfinished step from the client; a partial answer is not a completed answer. |
| `stream_first_output_timeout`, `stream_idle_timeout`, `websocket_idle_timeout` | First output or subsequent data did not arrive in time. | Check model latency, proxy, and overload. Retry later or choose a compatible source. |
| `stream_semantic_timeout` | Service events continue without useful progress. | Cancel the stuck task, check the provider, and retry. Keepalive events alone do not prove model progress. |
| `stream_event_too_large`, `upstream_body_too_large` | A response or individual event exceeded Relay's limit. | Reduce output/image volume. For a small request, verify the API and report its error ID. |
| `upstream_websocket_unsupported`, `websocket_not_supported` | The provider cannot use WebSocket. | Use HTTP streaming. If automatic fallback fails, disable **API → ChatGPT → WebSocket for ChatGPT** and reconnect the client. |
| `upstream_websocket_connection_limit`, `websocket_connection_limit_reached` | Too many provider connections. | Close unused connections, reduce concurrent tasks, and wait for the stated pause. |
| `client_cancelled`, `upstream_cancelled` | The client or provider cancelled the request. | Nothing is needed for intentional cancellation. Otherwise check application/connection closure and retry the unfinished request. |
| `response_incomplete` | The answer ended incomplete, for example at an output limit. | Inspect the finish reason. Increase a supported output limit, reduce the task, or request continuation with retained history. |

### Sign-in and credentials

| Code or message | Cause | Action |
| --- | --- | --- |
| `account_auth`, `credential_refresh_requires_reauth`, `invalid_grant`, `refresh_token_expired`, `invalid_refresh_token`, `refresh_token_invalidated`, `token_invalidated` | Sign-in expired, was revoked, or cannot be refreshed. | Sign in to that account again in **Connections**. An old export of an invalid session will not restore access. |
| `refresh_token_missing`, `access_token_missing`, `api_key_missing`, `missing_credentials` | A required token or key is absent. | Sign in or import a fresh complete export. Save API keys in their source cards. |
| `refresh_token_reused`, `upstream_refresh_token_reused`, `credential_refresh_retryable`, `account_refresh` | A transient refresh failure, including concurrent token rotation. | Wait for refresh and check again. New sign-in is needed only if an explicit reauthentication condition subsequently appears. |
| `refresh_lock_timeout`, `refresh_lock_unavailable`, `refresh_lock_configuration` | Another process owns refresh or its lock is inaccessible. | Wait, close duplicate Relay processes, and check data-folder access. Do not manually remove an active process's lock. |
| `credentials_missing`, `secret_missing`, `account_secret_missing`, `source_secret_missing`, `quota_secret_missing`, `credential_load_failed`, `account_runtime_credential_missing` | The record exists but its protected credentials cannot be read or are missing. | Restore the OS user's secret-store access; sign in again or re-enter the source key if lost. Copying only the database is insufficient. |
| `invalid_credentials`, `invalid_token_set`, `secret_invalid`, `account_secret_invalid`, `quota_secret_invalid`, `invalid_access_token`, `access_token_rejected`, `invalid_identity_token` | Credentials are incomplete, damaged, or rejected. | Obtain a fresh export/sign-in. Do not manually alter JWT or token contents. |
| `invalid_account`, `invalid_account_identity`, `invalid_account_id`, `invalid_chatgpt_account_id`, `provider_account_id_missing`, `account_runtime_provider_account_id_missing` | A valid account identifier is missing. | Sign in again or import the full account export, including its identifier. An unsuitable token alone is not enough. |
| `provider_account_lookup_failed`, `account_check_unavailable`, `account_check_failed`, `account_refresh_failed` | Provider account verification failed. | Check credentials, network, and proxy. Wait after 429; sign in after 401. Retry the affected record's check. |
| `account_check_response_too_large` | Account verification exceeded its response limit. | Verify API/proxy configuration. Update Relay and report the error if the correct connection reproduces it. |
| `account_identity_claim_conflict`, `account_identity_mismatch`, `account_changed` | Credentials belong to different accounts or the record changed during the operation. | Refresh the list and restart import from one consistent export. Do not combine separate sign-ins. |
| `agent_identity_invalid`, `invalid_agent_task_id`, `invalid_task_id`, `not_agent_identity`, `models_agent_task_invalid` | Agent identity/task data is invalid or obsolete. | Repeat a supported sign-in or import the complete current package. Do not transfer task IDs/signatures between accounts. |
| `callback_invalid`, `invalid_login_id`, `expired`, `callback_already_received` | An OAuth callback is invalid, expired, or already consumed. | Start a fresh sign-in and finish the latest attempt. For an already accepted callback, check the account list before repeating. |
| `callback_port_unavailable`, `listener_unavailable` | The local callback listener is unavailable. | Close the process occupying the sign-in port or use the supported manual callback flow; begin a new attempt. |
| "OAuth authorization was denied", "state does not match", "token endpoint rejected" | Sign-in was cancelled, belongs to another attempt, or was rejected. | Restart sign-in from Relay in one window. Check provider availability and its message. Never share callback URLs. |

### Quota, model and subscription checks

| Code or message | Cause | Action |
| --- | --- | --- |
| `quota_unauthorized`, `models_unauthorized`, `models_invalid_access_token`, `subscription_unauthorized`, `subscription_access_token_invalid` | Monitoring authentication was rejected. | Refresh account sign-in and check its data again. |
| `quota_forbidden`, `models_forbidden`, `subscription_forbidden` | The provider forbids these data reads. | Check account/workspace permissions and regional availability. Verify model access separately. |
| `account_profile_rate_limited`, `quota_rate_limited`, `models_rate_limited`, `subscription_rate_limited` | Checks are too frequent. | Wait for the pause; do not repeatedly press refresh. |
| `quota_timeout`, `quota_transport`, `models_transport`, `subscription_transport`, `quota_probe_failed` | A monitoring request did not complete. | Check internet and the account proxy, then refresh. The last recorded quota may remain visible until a successful check. |
| `quota_upstream`, `models_upstream`, `subscription_upstream`, `quota_http_status`, `models_http_status`, `subscription_http_status` | The monitoring service failed or returned an unexpected status. | Inspect the status. Retry transient failures later; correct access/request problems for 4xx. This does not prove zero quota. |
| `quota_invalid_response`, `quota_invalid_percentage`, `models_invalid_response`, `subscription_invalid_response` | Monitoring data is invalid. | Check for proxy/login pages replacing API responses. Update Relay and report a persistent code without the raw response body. |
| `quota_response_too_large`, `models_response_too_large`, `subscription_response_too_large` | Monitoring response size exceeded the limit. | Verify API and proxy configuration. A repeat on the correct connection needs provider/Relay compatibility investigation. |
| `models_invalid_account_id`, `subscription_account_id_invalid`, `subscription_account_missing` | The check cannot find the account. | Select the correct workspace and repeat sign-in/import. |
| `models_invalid_client_version`, `models_invalid_endpoint`, `models_client_init`, `subscription_configuration`, `quota_policy_invalid` | Monitoring parameters are unsupported. | Update Relay and check saved connection settings; report the code if already correct. |
| `quota_proxy_unavailable`, `models_proxy_unavailable`, `account_runtime_proxy_invalid` | The account proxy is invalid or unavailable. | Correct its URL/credentials and assignment in **Connections**, then check again. |
| `quota_account_location`, `models_account_location`, `remote_missing` | The account belongs to another environment or was removed from the server. | Select its owning environment. Refresh your server state and explicitly transfer again when needed. |
| `quota_authorization_prepare`, `quota_token_prepare`, `quota_token_refresh`, `quota_prepare`, `models_prepare`, `token_authority_failed` | Monitoring authorization could not be prepared. | Inspect the nested cause and restore sign-in, storage, or proxy access before checking again. |
| `quota_secret_load`, `quota_secret_store`, `models_secret_store` | Monitoring cannot access protected credentials. | Restore the OS user's secret store and restart Relay. Sign in again if the secret is lost. |
| `quota_storage`, `models_storage`, `quota_queue_failed` | The check cannot be queued or its result persisted. | Wait for current operations, check disk space/data permissions, and inspect **Settings → Diagnostics** if persistent. |
| `models_profile_restore` | An unfinished profile restoration prevents refresh. | Finish ChatGPT recovery and refresh models again. |
| `reset_credits_failed` / quota reset failure | Reset failed or its result is uncertain after a disconnect. | First refresh quota and reset credits. 401: sign in; 403/404: unavailable for this account; 429: wait; 5xx: retry later. Do not spend another credit if the reset already succeeded. |

### API balances and prices

| Code or message | Cause | Action |
| --- | --- | --- |
| Balance "Unsupported" (`unsupported`) | The key cannot read a balance or its format is unrecognized. | Check the provider dashboard. A working `/v1` does not guarantee balance access. Do not substitute dashboard passwords for API keys. |
| Balance "Unauthorized" (`unauthorized`) | Statistics require different permissions or the key is invalid. | Check key permissions. Missing statistics alone do not disable otherwise working inference. |
| Balance `rate_limited`, `unavailable`, `invalid_response`; `source_stats_unavailable` | Statistics were limited, unavailable, or malformed. | Wait after 429; otherwise check API/network and refresh. Retained values are stale, and an unknown balance is not zero. |
| `pricing_catalog_refresh_failed` | The price reference could not refresh. | Check connectivity and retry later. Older prices may remain; missing prices do not remove models. |
| `source_pricing_identity_invalid`, `model_price_invalid`, `source_model_price_invalid` | Price identity or amount is invalid. | Use an existing model ID and nonnegative USD prices per million tokens. A manual set needs input and output prices; do not confuse per-token and per-million rates. |
| `account_purchase_cost_invalid` | The account purchase price is invalid or too large. | Correct the USD amount in **Pool member rules → Settings**. It estimates payback and is not the provider balance. |

### Imports and connection settings

| Code or message | Cause | Action |
| --- | --- | --- |
| `empty_input`, `malformed_json`, `json_too_deep`, `input_too_large`, `too_many_items`, `invalid_source_file` | The import is empty, invalid, or too large. | Use the original supported JSON/TXT and split large packages. Do not paste HTML, archives, or a quoted JSON string as an object. |
| `unsupported_bundle_version`, `unsupported_snapshot_version`, `unsupported_schema` | The data version is unsupported. | Update Relay to a compatible version. Do not edit the version number or overwrite the original file. |
| `ambiguous_credentials`, `unknown_auth_mode`, `import_input_conflict` | Incompatible authentication methods or import inputs are mixed. | Use one consistent export and authentication method per record; do not combine an API key and OAuth account credentials. |
| `use_source_import` | This record is an API source. | Add it as an **API** in **Connections**, not as a ChatGPT subscription account. |
| `duplicate_item`, `item_not_selectable`, `import_selection_invalid` | A duplicate or unready import row is selected. | Select one valid record in the preview, correct its errors, and retry only failed rows. |
| `item_not_found`, `import_not_found`, `session_not_found`, `import_expired`, `import_session_invalid`, `invalid_session_id`, `session_collision` | The import preview is unavailable or stale. | Create a new preview from the original file, check selected records, and confirm again. |
| `import_invalid`, `preview_invalid`, `snapshot_invalid`, `snapshot_mismatch`, `snapshot_unsafe` | Temporary import data changed, is damaged, or unsafe. | End this attempt and create a fresh preview. Do not edit temporary snapshots manually. |
| `import_serialize`, `preview_serialize`, `secret_serialize` | Import data could not be prepared. | Retry using a current original export. Update Relay and report the diagnostic code, without the package contents, if it persists. |
| `refresh_exchange_failed`, `refresh_exchange_unavailable` | Refresh-token exchange failed or is unavailable. | Check network/proxy. Obtain a fresh sign-in for invalid tokens; otherwise retry after a pause. |
| `source_base_url_invalid`, `source_invalid`, `source_self_route` | The API address is invalid or points back to Relay. | Use the actual external API base URL and correct prefix. Never point a source at this same pool, which creates a loop. |
| `source_model_discovery_failed`, `source_test_failed`, `models_required` | The model list was not confirmed. | Check key/address/format and refresh models. If an explicit list is required, supply actual available IDs and verify a request. |
| `source_probe_unavailable` | Generation could not be checked: authorization, rate limit, timeout or temporary provider failure. Existing evidence is retained. | Check the key and provider status, then run Check generation again. HTTP 401/403 does not prove a format unsupported. |
| `source_probe_unsupported` | The selected model/format endpoint returned HTTP 404 or 405. | Verify the address and model ID, select another provider format or correct the manual routes. Other formats remain available. |
| `source_probe_invalid_response` | The endpoint did not return a complete text response in the selected format. | Verify the format and provider documentation. A successful catalog or arbitrary HTTP 200 does not confirm generation. |
| `source_probe_stale` | The connection changed while it was being checked. The result was discarded. | Refresh the connection, save any pending address/key changes, then check again. |
| `invalid_label` | The record name is invalid. | Use a short nonempty name without control characters and save again. |
| `source_store_failed`, `account_store_failed`, `source_secret_store_failed` | The connection or secret could not be saved. | Check disk space and data/secret-store access. Refresh the list before retrying to avoid duplicates. |

### Pool settings and background checks

| Code or message | Cause | Action |
| --- | --- | --- |
| `pool_routing_conflict`, `configuration_revision_stale` | Settings changed after the editor opened. | Close the editor, refresh state, and reapply changes to the current configuration. |
| `pool_members_empty`, `pool_members_too_many` | No members were selected or the operation exceeds its limit. | Select existing members and split very large operations. |
| `account_not_found`, `account_missing`, `source_not_found`, `source_priority_target_not_found`, `not_found` | A record was removed or belongs to another pool. | Refresh and select an existing connection in the correct environment. An old editor cannot restore a deleted record. |
| `max_retry_candidates_invalid`, `cooldown_after_failures_invalid`, `source_recovery_delay_invalid` | An imported legacy retry value or member recovery delay is outside supported bounds. | Correct the imported value or the delay in member **Settings**. Pool recovery is automatic. |
| `model_id_invalid`, `model_order_invalid` | A model ID/order is invalid or duplicated. | Refresh the catalog and select actual model IDs without duplicates. |
| `reasoning_levels_invalid`, `model_service_tier_unsupported`, `model_reasoning_recovery_failed` | The model does not confirm a mode/tier or its settings could not be recovered. | Refresh models and choose a supported mode or standard speed. Reopen model rules after a recovery failure. |
| `configuration_preset_invalid`, `configuration_reference_missing` | A preset is invalid or references missing connections. | Export it again with a compatible version; map members to existing connections in preview. Presets do not transfer secrets. |
| `configuration_store_failed`, `configuration_runtime_failed`, `runtime_reload_failed`, `gateway_sync_failed`, `source_runtime_invalid` | Configuration could not be saved or applied to the runtime. | Refresh and inspect actual pool membership/settings, fix the named connection, and apply again. Use diagnostics if it persists; a closed dialog is not proof of success. |
| `wake_task_not_found`, `wake_account_missing` | A background task/account is absent from this environment. | Open the task in its owning environment and select an existing account. |
| `wake_model_unavailable`, `wake_invalid_request`, `wake_invalid_configuration`, `wake_invalid_endpoint` | Background request configuration/model is unsuitable. | Choose an entitled model and supported task parameters. |
| `wake_invalid_access_token`, `wake_invalid_provider_account_id`, `wake_unauthorized`, `wake_credentials_unavailable` | Background work has no usable account sign-in. | Restore account sign-in and checks in **Connections**, then retry the task. |
| `wake_forbidden`, `wake_tags_unsupported`, `wake_confirmation_unsupported` | Permissions or a server feature are unsupported. | Check account rights. Use supported account selection and automatic execution for server tasks. |
| `wake_rate_limited` | The provider limited background requests. | Wait and reduce task frequency/concurrency. |
| `wake_proxy_unavailable`, `wake_timeout`, `wake_transport`, `wake_upstream`, `wake_http_status` | Network or provider failure interrupted background work. | Check proxy and HTTP status, resolve the cause, then retry. |
| `wake_request_too_large`, `wake_response_too_large`, `wake_invalid_response` | Background request/response size or format is invalid. | Use a short request and compatible model. Check the connection and update Relay if persistent. |

### Profiles, server and local data

| Code or message | Cause | Action |
| --- | --- | --- |
| `profile_restore_blocked` | The profile changed during the operation or a newer sign-in exists. | Let ChatGPT finish writing, close it, and inspect recovery again. Preserve current sign-in; select an older snapshot only deliberately. |
| `recovery_required`, `cleanup_incomplete` | Recovery or cleanup from an earlier operation is unfinished. | Open **Recovery** and operation diagnostics. Preserve backups and finish recovery before repeating. A full data reset is not the first remedy. |
| `snapshot_missing`, `snapshot_io` | A required snapshot is missing or unreadable. | Check environment, backup existence, and folder access. Reconfigure the client if the backup is lost; do not replace it with an empty file. |
| `profile_attach_unavailable`, `profile_rotation_invalid`, `profile_rotation_missing`, `system_key_missing`, `diagnostic_key_unavailable` | Managed client attachment could not complete. | Refresh server state and reconnect the application. Use the current request key after rotation, never a management token. |
| `management_unauthorized` · 401 | Your server management token is invalid. | Correct the token in the server connection. It is separate from the `/v1` request key. |
| `management_blocked` · 429 | Repeated failed authentication temporarily blocked management access. | Stop retries, correct the token, and wait for the block to expire. |
| "remote server URL is invalid", "must not contain a path", "HTTP requires the explicit insecure option" | The server address is unsuitable. | Use its origin without `/v1`, query, fragment, or embedded credentials. Prefer HTTPS; insecure HTTP is only an explicit choice for a trusted environment. |
| "remote server redirect was rejected", "remote server request failed" | The server redirects or is unreachable. | Enter the final address directly and check DNS, certificate, network, and Relay Server process. Secrets are not automatically forwarded through redirects. |
| "remote protocol is incompatible", "remote server response is invalid/too large" | Server versions are incompatible or the response is not its management API. | Update desktop and server to compatible versions and check reverse-proxy routing. |
| `proxy_invalid`, `proxy_unavailable`, `proxy_route_ambiguous` | Proxy configuration is invalid or both bypass and proxy use are selected. | Choose one route: shared proxy, individual proxy, or bypass. Correct URL and credentials. |
| `proxy_assignment_invalid`, `proxy_assignment_duplicate` | Bulk proxy assignment does not match the selected accounts. | Supply one URL per selected account and remove duplicate assignments. |
| `secret_store_unavailable`, `credential_store_unavailable`, `vault_failed` | Protected storage is unavailable. | Restore OS-user access; for a server, check the vault and its encryption key. Do not replace the key for an existing database with a new one. |
| `io`, `store_failed`, `persistence_failed`, `account_token_persistence`, `credential_persist_failed`, `metadata_persist_failed` | Reading/writing data or refreshed credentials failed. | Check disk space, permissions, and file locks; close duplicate Relay processes. Preserve a backup before recovery and inspect diagnostics if persistent. |
| `usage_persistence_failed`, `response_affinity_persistence_failed` | Usage or response ownership could not be persisted. | Restore disk writes. Usage may have gaps, and continuation after restart may require complete history. |
| `account_export_failed` | Account export could not be created. | Check protected-credential access and retry the selected record. Exports contain sign-in material and must not be sent to support. |
| `diagnostic_model_unavailable` | No compatible model is available to diagnostics. | Enable a working member's model in **Pool** and retry. |
| `diagnostic_failed`, `diagnostic_upstream_failed`, `diagnostic_invalid`, `diagnostic_incomplete`, `diagnostic_too_large` | A diagnostic request failed or returned an invalid/incomplete response. | Inspect its route/provider error in **Usage**. A successful catalog read is not a successful model answer. |
| `portable_update_unsupported`, `portable_update_unavailable` | No suitable portable update exists for this platform/version. | Choose the official Relay package for your OS/architecture and preserve data before manual update. Do not substitute another platform's package. |
| `portable_not_writable`, `portable_update_failed` | The update could not be written, downloaded, or prepared. | Check disk space, application-folder permissions, and update-server connectivity. Close duplicate Relay processes and retry. For manual updates, use the official package for your platform and preserve data. |
| `invalid_state`, `invalid_configuration`, `unsupported_value`, `operation_failed` | The action or parameters do not suit the current state. | Read the specific message, refresh, and correct the named field/mode. Report the code and diagnostic stage when settings are already valid. |

### If the code is missing

Providers may introduce their own codes. The saved provider code/message in
request details is more specific than a general HTTP status. Relay does not
mix a partial answer with a new answer from another member after output begins.
Continuation may require complete history or the original connection.

For support, include Relay version, operating mode, action, model, HTTP status,
error origin, code, and request ID. Redacted details and logs are available in
**Usage** and **Settings → Diagnostics**. Do not send keys, cookies, sign-in
files, account exports, or conversations.
