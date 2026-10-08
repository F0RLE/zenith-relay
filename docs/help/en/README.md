# Zenith Relay

Relay connects ChatGPT, OpenCode, and other compatible clients to saved
accounts and API sources. A pool chooses an eligible connection for each
request and exposes one private API address.

[How Relay works](#how-relay-works) | [Quick start](#quick-start) |
[Overview](#1-overview) | [Connections](#2-connections) | [Pool](#3-pool) |
[API](#4-api) | [Usage](#5-usage) | [Recovery](#6-recovery) |
[Settings](#7-settings) | [Errors](#8-errors)

## How Relay works

- **Connections** stores accounts, API sources, keys, and proxies.
- **Pool** decides which saved connections and models may receive requests.
- **API** shows the address and request key for clients.

Saving a connection, adding it to the pool, and connecting a client are separate
steps. A connection can be saved without being used by the pool.

### Operating modes

- **Computer:** the local Relay process runs the pool and its API.
- **Choose API:** a client uses one selected API source directly; pool rotation
  and pooled usage do not apply.
- **On your server:** a user-operated Relay Server runs the pool while the
  desktop manages it. Closing the desktop does not stop that server.

Changing the mode does not move accounts or secrets. Account transfer to a
user-operated server is a separate confirmed action.

### Quick start

1. Select **Computer** or **On your server**.
2. In **Connections**, sign in to ChatGPT, import an account, or add an API
   source.
3. In **Pool**, add the connections and allow the required models.
4. Start **API**, then use **Pool → Connect** for ChatGPT or OpenCode. Other
   clients use the address and request key shown in **API**.
5. Send a request and inspect **Overview** or **Usage**.

**Repeat quick setup** in **Help** opens the wizard again. Use **Choose API** or
an API source's **Launch** action for a direct connection to one source.

### Install on macOS

Move the downloaded DMG application to **Applications**. The build is ad-hoc
signed and not notarized. If macOS blocks the first launch, use
**System Settings → Privacy & Security → Open Anyway** for this application.
Do not disable Gatekeeper for the whole system.

If the release checksum matches but macOS still reports the app as damaged,
remove quarantine only from the application:

~~~sh
xattr -dr com.apple.quarantine "/Applications/Zenith Relay.app"
~~~

## 1. Overview

**Overview** shows the selected mode, address, available models, pool members,
request totals, and speed. Select a period and scope to change the summary.

### Balances and estimates

Provider statistics may expose a balance, key allowance, or subscription
allowance. Relay keeps those values separate and preserves the provider's
currency or units. **No balance API** means no supported statistics endpoint
was found; **Stats access denied** means the provider refused it. Requests may
still work in both cases.

Use **Refresh balance** to request a new value. A failed refresh keeps the last
value and marks it stale. A restart clears the temporary statistics cache.
Relay estimates and **API equiv. used** are usage estimates, not a provider
charge or an account balance.

## 2. Connections

### Accounts and quota

Add a ChatGPT account through the sign-in window or import your own account
file. Account notes are editable; credentials and exports are secret data.
**Refresh** reads the provider's account state, models, and quota. It does not
add quota or invent a reset date. A revoked sign-in must be completed again.

A saved connection does not have to be a pool member. Disabled or unavailable
accounts can remain saved for recovery. Refreshes use the account's configured
proxy and keep newer credentials from being overwritten by an older request.

### Proxies

Save proxies in **Connections → Proxies** or import one address per line.
**Test proxy** checks the selected proxy and shows the observed result. The
check does not use an account key and never falls back to a direct connection.

### API sources

Select a known service or **Custom API** in **Add source**. Enter the provider
address, key, and name. A full endpoint can provide a protocol hint. Relay
stores every model returned by the source; route compatibility is checked later.

**Launch** connects a client directly to that source and bypasses pool rotation
and pooled usage. Use **Pool → Connect** when the request should be routed by
Relay.

### Automations

**Connections → Automations** can start a quota countdown or request a provider
reset when its condition is met. Select the type, accounts, and model when a
request is required, then save and enable the rule. Local tasks run while Relay
runs; server tasks depend on server capabilities. There is no separate manual
start button.

### Your server

Save the server address and management token in the server connection. Clients
use the server API address and a separate request key. Moving an account to the
server is explicit and confirmed; it is not automatic synchronization.

## 3. Pool

### Members and route selection

Add saved accounts and API sources in **Members**. Open a member policy to
allow or deny models. Removing a member does not delete its saved connection.
Relay checks the requested model and protocol, then skips disabled, signed-out,
unavailable, quota-blocked, or capacity-limited members.

The pool has two rotation modes:

- **Automatic** chooses an eligible member using current load, fresh quota or
  credits, weights, and the configured concurrency limit. A continuation stays
  with its response owner unless safe saved history allows a move.
- **Manual** follows the saved member order, skips unavailable or full members,
  and wraps to the first member after the last one.

Changes to mode, order, weights, limits, and membership save with the pool
policy. The next-candidate hint is advisory; dispatch checks availability again.

### Failures and retries

Relay retries only before response data reaches the client and only when it can
show that the provider did not accept the request. A sent or uncertain request
is not silently replayed on another account. Provider rate limits and reset
windows remain in force. Long generations are not ended merely because output
is quiet; the client can cancel them.

### Models and member policies

A member policy controls access to a model; it is not a quota value. **Model
Rules** controls pool-wide enablement, display order, reasoning modes, and speed
preferences. Model IDs remain distinct even when their display names match.
Unknown limits or reasoning levels remain unknown; metadata does not grant a
route that the source cannot execute.

**Reset model order** restores the provider-group order. Manual model order and
member rotation order are separate settings. **Standard**, **Fast**, and
**Ultrafast** are processing tiers for compatible OpenAI models and do not
promise a response time.

### Prices and additional settings

Prices in a member policy are used for usage estimates. Provider observations
win over reference prices; a manual price is a fallback. Prices do not change
route eligibility or charge a customer wallet. Cache fields describe an
observed or configured price and do not enable caching.

**Drain** stops new assignments. **Purchase cost, USD** is used only by the
payback estimate. Presets transfer pool and model settings, not keys, sign-ins,
quota, or request history.

## 4. API

### Application address and key

The **API** tab shows status, the local address, start controls, and Relay's
request key. The default local base address is:

```text
http://127.0.0.1:14998/v1
```

The request key is different from a provider key and a server management token.
**Reissue API key** replaces it and requires confirmation. A port change
restarts the local API.

Supported local paths are:

| Format | Path |
| --- | --- |
| Responses | `/v1/responses` |
| Chat Completions | `/v1/chat/completions` |
| Anthropic Messages | `/v1/messages` |
| Gemini | `/v1beta/models/{model}:generateContent` |

Streaming Gemini uses `:streamGenerateContent?alt=sse`. Native routes preserve
provider parameters; a conversion fails before generation when the requested
shape cannot be represented. Realtime and audio/video conversion are not
provided.

### Basis Points

**Use Basis Points** is an optional ChatGPT account transport. It shares the
account's quota and rotation slot and may return a completed response before
Relay emits requested SSE frames. It does not guarantee provider quality or
replace native capability. Unsupported structured output, remote image URLs,
explicit fast speed, and opaque `previous_response_id` continuation are
rejected for this route.

### ChatGPT

The **ChatGPT** tab selects the account used by the ChatGPT interface. That
choice is separate from the account selected for a pooled request. Background
tasks and the ChatGPT WebSocket setting control that client integration only.

**Wait for route recovery** can hold eligible text requests while the pool
recovers. It does not repair invalid input or replay a request already accepted
by a provider. Codex model cards and Relay model rules are updated separately;
Ultra is a Codex orchestration mode, not a provider reasoning level.

### OpenCode

Use **Pool → Connect → OpenCode** for a pooled connection or **Launch** for one
API source. Relay preserves the source address, key, SDK, and selected model
when it can. **Recovery** can restore the saved OpenCode configuration.

## 5. Usage

**Usage** contains requests sent through the selected local or server pool.
Direct provider calls and activity outside Relay are not included. A request
shows the requested model, sent model, member, protocol, timing, tokens, and
estimated cost when available. Error origin, provider code, status, and a
redacted message are stored separately from request and response text.

## 6. Recovery

**Recovery** is available in **Computer** mode. A ChatGPT snapshot contains
Relay-managed `config.toml` and `auth.json`; an OpenCode snapshot contains its
selected JSON configuration. Restore replaces the selected snapshot contents.
The automatic backup created before connecting a client is separate and does
not overwrite a newer manual sign-in or external edit.

## 7. Settings

**Appearance** controls language and theme. **Application** shows the version,
updates, and data folder. **Debug mode** enables detailed operation logs;
errors and crashes are recorded without it. **Reset local pool data** first
tries safe client recovery, then removes local pool data only when recovery is
safe.

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
| `admission_queue_full` | Relay reached the waiting-request count or retained-memory limit for the pool or request key. No new provider request was sent. | Reduce concurrent waiting requests or their input size, let queued requests finish, then retry. |
| `admission_wait_expired` | The logical request exhausted its total admission wait or retry deadline. | Retry as a new request after capacity recovers; review concurrency limits or the explicit wait-until-available setting. |
| `no_eligible_source` · 503 · "no eligible source is available for this model" | No member currently qualifies for this model and format. | In **Pool**, check membership, pool and member model rules, sign-in, quota, pauses, and draining. Wait for busy members or add a compatible reserve. Test a new independent request and inspect its route in **Usage**. Removing unhealthy accounts is unnecessary for rotation. |
| `exceeded retry limit`, `unexpected status` | The client reports a failed request or exhausted retries. | Check the last HTTP status and code in **Usage**. For 429, distinguish quota exhaustion from request frequency; for 503, inspect available routes. Resolve that cause before retrying. The retry limit itself is not the cause of a provider rejection. |
| `all_sources_cooling_down`, `all_candidates_cooling_down` · 429 | Eligible routes are paused after rate limits. | Wait for `Retry-After` or the next attempt time. Reduce concurrent requests. A reserve must support the same model and format. |
| `all_sources_temporarily_unavailable` · 503 | Eligible members are temporarily unavailable after failures. | Inspect each member's last error and wait for recovery. Fix network failures; follow the quota instructions for exhaustion. |
| `model_not_found` · 404 | The model is absent from the pool's available catalog. | Refresh models in **Connections**, check the exact ID and both sets of **Pool** model permissions, and confirm client format compatibility. |
| `route_not_found` · 404 | The requested API path does not exist. | Check the Relay API address and use an endpoint supported by the connected client. |
| `method_not_allowed` · 405 | The endpoint does not accept this HTTP method. | Use the method required by that endpoint; for example, generation uses POST. |
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
| `upstream_route_degraded`, `route_degraded` | The ChatGPT response reports a different model or an internal downgrade id such as `degrade2`. | With Degraded routes enabled in API, Relay rejects it in JSON, SSE (including response.created) and WebSocket. A pre-generation refusal can rotate; generated or delivered output is never replayed. Dated snapshots of the same model are allowed. The account pauses briefly. This checks reported identity, not quality. |
| `upstream_model_unavailable`, `model_not_available` · 403/503 | The model is temporarily unavailable, including Basis Points `Model access has changed`. | Only this model on the member pauses; other models remain available. Relay can try another compatible member. Basis Points access can differ between accounts and from native Responses; the model pause currently covers both account transports. This is not exhausted quota. |
| `upstream_model_unsupported`, `model_not_supported` | The selected provider path does not support this model. | Refresh the source catalog and verify the model ID, API address, key permissions, and provider support. Relay will use another compatible member when one is available. |
| `upstream_model_capacity`, `model_at_capacity` | The model is temporarily overloaded. | Wait or use another compatible source. Signing in again does not increase provider capacity. |

### Requests, history and tools

| Code or message | Cause | Action |
| --- | --- | --- |
| `invalid_request`, `upstream_invalid_request` · 400 / 422 | The body or a request parameter is invalid. | Fix the field named in the redacted message. Requests need a JSON object, nonempty model, and valid `stream`; path/body models must agree. Compact responses do not support streaming. |
| `invalid_stream_id` · 400 | A Responses WebSocket `stream_id` is invalid. | Use 1–256 ASCII letters, digits, `_`, `-` or `.`; omit the field for the default stream. |
| `upstream_context_too_large`, `context_too_large`, `context_length_exceeded` | History exceeds the model context. | Shorten history/attachments, summarize, start a new conversation, or choose a model with a larger context. |
| `request_too_large`, `upstream_payload_too_large` · 413 | The provider rejected the request as too large. | Shorten history or remove images. Relay does not reject JSON generation, image upload, or image edit for size. Retrying the unchanged request will not help. |
| `request_encoding_unsupported` · 415 | Unsupported or stacked request compression. | Use an uncompressed JSON body or a single `gzip` / `zstd` encoding. |
| `request_encoding_invalid` · 400 | Compressed input is corrupt, incomplete or needs a zstd window above 64 MiB. | Update the client or send an uncompressed request. Relay does not apply a size cap to compressed or expanded JSON. |
| `compaction_response_invalid` · 502 | Context compaction did not finish with a valid encrypted result. | Keep the existing conversation history and retry explicitly after checking the upstream connection. Relay does not replay this generation or fabricate a summary. |
| `upstream_instructions_required`, `missing_required_parameter` | A required field, including instructions, is absent. | Supply the field named by the provider or update the client generating it. Retrying the same body does not fix it. |
| `upstream_unsupported_request`, `unsupported_request` | A parameter or capability is unsupported. | Disable the named parameter, tool, or mode and use a compatible format. |
| `upstream_content_policy`, `content_policy_violation` | Provider policy rejected the request, including `This request was blocked by our usage policy`. | A real HTTP 403 stays 403, even if a host labels it insufficient_quota. Relay does not zero quota, pause the account or rotate members for this refusal. Check the provider's rules and route access; headers do not establish permission. |
| `response_continuation_unavailable`, `response_affinity_miss` | The response owner is unavailable and full replay history is missing. | Restore the original account/API or resend complete history from the client. Start a new conversation if history is lost. A rotation mode change cannot restore context. |
| `upstream_previous_response_not_found`, `previous_response_not_found` | The provider no longer knows the previous response. | Resend full history without the stale response reference, or start a new conversation. Do not transfer just a response ID to another API. |
| `upstream_tool_call_mismatch`, `tool_call_not_found` | A tool result has no matching call, or a call has no result. | Relay retries once when the complete pair proves a missing or confused call identifier. It never removes results or guesses between parallel calls. If the error remains, update the client and resend the complete call/result pair, or start a new conversation if the missing history cannot be recovered. |
| `upstream_encrypted_content_invalid`, `invalid_encrypted_content` | The account rejected encrypted reasoning or compaction context from another connection. | Relay removes the rejected ciphertext and its bound ID, keeps any visible summary, and retries once with the remaining history. If that retry fails, return to the connection that created the item or start a new task with ordinary history. Do not manually edit encrypted blocks. |
| `tool_use_not_supported`, `chat_feature_not_supported` | This route cannot represent the requested tool or feature. | Function tools and their results are supported on compatible Chat Completions routes. Check the model's format capabilities; choose a matching native route or remove the specifically unsupported option. |
| `upstream_conflict`, `conflict` · 409 | State changed or another operation is in progress. | Wait for the earlier operation, refresh state, and retry once. Restore conversation history when the conflict concerns continuation. |
| `upstream_candidate_rejected`, `source_rejected` | A route rejected the request without a more specific category. | Read the provider code/message and check model, permissions, and format. A new independent request can use a compatible reserve. |

### Adapters and images

| Code or message | Cause | Action |
| --- | --- | --- |
| `adapter_binding_unsupported`, `source_protocol_invalid`, `source_pool_protocol_unsupported` | Relay could not build a compatible automatic route for the requested protocol and model. | Refresh the source catalog and verify its API address and model ID. Use another member when the provider does not expose a compatible native path or translatable format. |
| `adapter_invalid_request` | The adapter cannot translate the request. | Remove the field named in the message or choose a native-format source. |
| `adapter_parameter_unsupported` | A meaningful request parameter has no lossless mapping on the selected route. | Check the field named in the message and `error.param`. Use a native route or change that option. Encrypted input history requires its compatible native route; do not delete it from the conversation. Relay does not silently discard it. |
| `adapter_compaction_unsupported` | This route cannot accept another provider's encrypted compaction checkpoint. | Codex auto-compact on any non-native model, including `/v1/responses/compact`, is handled by Relay. Use a native Responses route for a foreign checkpoint, or start a new task. |
| `adapter_continuation_missing`, `adapter_continuation_mismatch` | Adapter continuation state is lost or belongs to another binding. | Restore the former source/adapter. Transfer complete history or start a new conversation when it is unavailable. |
| `adapter_tool_unsupported`, `adapter_reasoning_unsupported` | The adapter cannot represent the tool or reasoning mode. | Choose a supported capability or native format. Allowing a mode in model rules does not add upstream support. |
| `adapter_upstream_response_invalid`, `adapter_upstream_stream_invalid` | The provider response does not match a supported conversion. | Update Relay and check the source's API address and format. If reproducible, use a source with a native format and report the code and request ID. |
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
| `stream_invalid` | The stream event format is invalid. | Open the request details. Type `relay_stream_parser` identifies Relay's JSON parser diagnostics: error category, position and frame sizes, without response content. Report these diagnostics and the request ID. Older records may lack details; reproduce on the current build. |
| `stream_incomplete`, `upstream_websocket_closed`, `upstream_websocket` | The connection ended before completion. | Check network and proxy timeouts. Retry the unfinished step from the client; a partial answer is not a completed answer. |
| `stream_first_output_timeout`, `stream_idle_timeout`, `websocket_idle_timeout`, `stream_semantic_timeout` | A stream timeout from an older Relay version or an external service. The current version does not time out an active generation while waiting for output. | Update Relay and your Relay Server. For provider or proxy errors, check that service's limits. You can cancel a stuck request in the client. |
| `stream_event_too_large` | A WebSocket frame exceeded Relay's 64 MiB frame limit. | Shorten that turn or use HTTP streaming. For a small request, verify the API and report its error ID. |
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
| `quota_authorization_prepare`, `quota_token_prepare`, `quota_token_refresh`, `quota_prepare`, `models_prepare`, `models_authorization_prepare`, `token_authority_failed` | Monitoring could not prepare the local sign-in, token, or model check. The account itself may still be usable. | Refresh the account again. If it remains, sign in again or check the account proxy. This is not a provider rejection of the account. |
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
| `source_model_discovery_failed`, `source_test_failed`, `models_required` | Relay could not read a usable model catalog. | Check the key, API address, provider permissions, and network, then refresh models. A successful catalog is enough for inventory; it does not prove every generation feature. |
| `source_probe_unavailable` | A legacy explicit diagnostic request failed because of authorization, rate limiting, timeout, or a temporary provider error. Normal setup, catalog refresh, and route selection do not call it. | Check the key and provider status. This diagnostic result does not change model inventory or automatic routing. |
| `source_probe_unsupported` | A legacy explicit diagnostic request received HTTP 404 or 405 for its selected model and format. | Verify the address and model ID. The diagnostic result does not add or remove models or choose the route. |
| `source_probe_invalid_response` | A legacy explicit diagnostic request did not receive a complete text response. | Check the provider's documented endpoint. Normal model discovery and routing do not depend on this diagnostic. |
| `source_probe_stale` | The connection changed during a legacy explicit diagnostic request, so its result was discarded. | Save the current address and key, then refresh the source catalog. |
| `invalid_label` | The record name is invalid. | Use a short nonempty name without control characters and save again. |
| `source_store_failed`, `account_store_failed`, `source_secret_store_failed` | The connection or secret could not be saved. | Check disk space and data/secret-store access. Refresh the list before retrying to avoid duplicates. |

### Pool settings and background checks

| Code or message | Cause | Action |
| --- | --- | --- |
| `pool_routing_conflict`, `configuration_revision_stale` | Settings changed during saving or after a preset preview. | Rotation automatically retries against current settings. If the error remains, wait for other edits to finish and repeat your change using the displayed values. For a preset, refresh its preview before applying. |
| `pool_members_empty`, `pool_members_too_many` | No members were selected or the operation exceeds its limit. | Select existing members and split very large operations. |
| `account_not_found`, `account_missing`, `source_not_found`, `source_priority_target_not_found`, `not_found` | A record was removed or belongs to another pool. | Refresh and select an existing connection in the correct environment. An old editor cannot restore a deleted record. |
| `max_retry_candidates_invalid`, `source_recovery_delay_invalid` | A configured retry value or member recovery delay is outside supported bounds. | Correct the retry value or the delay in member **Settings**. Pool recovery is automatic. |
| `model_id_invalid`, `model_order_invalid` | A model ID/order is invalid or duplicated. | Refresh the catalog and select actual model IDs without duplicates. |
| `reasoning_levels_invalid`, `model_service_tier_unsupported`, `model_reasoning_recovery_failed` | The selected reasoning mode is absent from reference metadata, the speed is outside Relay family policy, or model settings could not be recovered. | Refresh models and choose a supported mode or standard speed. Reopen model rules after a recovery failure. |
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
| `profile_restore_blocked` | The profile changed during the operation or automatic rollback would replace a newer sign-in. | Let ChatGPT finish writing and retry the explicit connection. Relay saves a new manual sign-in as the next restore point; if the error repeats, inspect the profile under **Recovery**. |
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
| `proxy_check_timeout` | The connection check exceeded 12 seconds. | Check the address and port, or retry later. The saved proxy is retained. |
| `proxy_check_connection_failed`, `proxy_check_auth_failed` | The selected proxy could not be reached or rejected authentication. | Correct its address, port, username and password. No direct connection is attempted. |
| `proxy_check_rejected`, `proxy_check_invalid_response` | The check service rejected the request or returned no valid exit IP. | Retry later and check whether this proxy permits HTTPS traffic to Cloudflare. This does not establish model availability. |
| `proxy_check_unavailable` | The saved proxy could not be read or was removed during a check. | Refresh the list and restore access to protected storage before retrying. |
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
