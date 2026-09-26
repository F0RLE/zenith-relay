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

Quick Setup first chooses where the shared pool runs: on this computer or on
your server. On the computer, the connection step can add multiple accounts
and API sources to the same pool. Select a provider and enter its key; for
**Custom API**, also enter the address and a name. A saved source is included
in the pool immediately. Add another connection or continue to the client
choice. Importing the current profile does not advance the wizard for you.
Use **Back** to change a choice or **Set up later** to open the application.
For a direct connection to a single source, switch to **Choose API** inside
the application after setup.

### Install on macOS

Download the DMG for your Mac from the official Zenith Relay GitHub Release and
move **Zenith Relay.app** to **Applications**. The app has an ad-hoc signature,
not Apple notarization. If macOS blocks the first launch, open **System Settings
→ Privacy & Security**, select **Open Anyway** for Zenith Relay, then confirm
**Open**. This approves this app only; you do not need to disable Gatekeeper.

If macOS instead reports the app as damaged, first make sure the DMG came from
the official release. Compare the release's `SHA256SUMS` entry with
`shasum -a 256 ~/Downloads/zenith-relay-macos-arm64.dmg` (use `intel` instead
of `arm64` on an Intel Mac). If the hashes differ, download it again. If they
match, in Terminal remove the download quarantine **only from this app**:

~~~sh
xattr -dr com.apple.quarantine "/Applications/Zenith Relay.app"
~~~

If you installed it elsewhere, use that app's path instead. In-app updates
have a separate Relay updater signature; report it if an update is blocked
again so that the specific update can be checked.

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
Reopening the page may display the last value from this running session without
contacting the provider. Use **Refresh balance** to request a new reading.
The warning identifies a stale or failed reading, not a fresh balance; restarting
Relay or the user-managed server clears this temporary statistics cache.
Replacing a source key also discards the previous key's figures, including
when the API address is unchanged.
Background readings also update the open Pool and selected API overview;
displaying those readings does not send another request to the provider. Model,
quota, and balance refresh state is kept separate from routing and is not shown
as a status line on ordinary cards. Errors and stale-data reasons remain
available in diagnostics.

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

On Relay Server, manual and background account refreshes share the same work.
Repeated clicks do not start parallel checks or bypass a provider's requested
pause. Canceling one waiting request does not stop a check needed by others.
Disabled accounts are not checked periodically; an enabled account can still
be monitored outside the inference pool. Refresh details do not affect routing
and remain available in diagnostics with the provider error when one exists.

Request **Credits** and **reset credits** are different. Fresh positive request
credits can allow work even when percentage quota is exhausted. A reset credit
only allows a separate reset operation. **Reset weekly quota** appears for a
local account with an available credit, asks for confirmation, and consumes
that provider credit.

When an account requires sign-in, sign in to that account again. A revoked
sign-in cannot be restored by refreshing the saved session. Other eligible pool
members can keep working.

An account export contains sign-in credentials. Treat it as a secret file.
It is different from a pool preset, which contains settings only.

On the computer, a late quota/model refresh does not overwrite an account after
you sign in again, change its proxy, or remove and re-add it. Newer quota data
received with a model request also takes precedence over an older background read.
Quota and model lists refresh independently. A manual refresh joins any matching
read already in progress; closing its waiting view does not cancel shared work.
Provider-requested pauses still apply, including to repeated manual refreshes.

### Proxies

**Connections → Proxies** lists saved addresses, assigned accounts and the last
connection check in this session. Usernames and passwords stay hidden.
Use **Import** to add one address per line. **Check after adding** is enabled by
default and can be cleared before importing; the globe icon with the **Test proxy**
tooltip runs another check later.
Only newly added addresses are checked, with up to three checks at a time.

The check makes an HTTPS request to Cloudflare through the selected proxy. It
shows the observed exit IP, country and request duration, with a 12-second limit.
No account, model or API key is used; a failed proxy never falls back to a direct
connection. The declared country from a proxy's username is shown separately.
Results describe this request, not model access or a guarantee of the next IP
for a rotating proxy. A failed check keeps the address and account assignments.
Check results are cleared when you leave Connections or change runtime mode.

### API sources

Select a service at the top of **Add source**, or choose **Custom API**. The key,
API address and name appear below. Known services fill in the address and name;
you can edit them. The provider selector stays available while you fill the form.
Use the provider's API address, not
a dashboard URL. A full endpoint such as `/v1/messages` also supplies a format
hint. Relay reads the model catalog and declared endpoint support. Every model
returned by the provider enters the source inventory immediately.

The API source editor contains:

- **General**: name, address, key replacement, and discovered models.
- **Pricing**: prices for usage estimates; see the pool policy section below.

Relay always resolves routes automatically. For each request it prefers a matching
native Responses, Chat Completions, Messages, or Gemini endpoint and otherwise
uses the required adapter. Provider declarations and the entered endpoint guide
that choice; when neither identifies a protocol, Relay uses the source fallback.
Refreshing the catalog does not send a generation request and no probe controls
whether a discovered model is present or routed.

**Launch** on a source connects the chosen application directly to that API.
Those requests bypass the pool, its rotation, and Relay's usage history.
ChatGPT/Codex direct connections require native Responses. OpenCode uses the
source's native Responses, Chat Completions, Messages or Gemini SDK. A model
in the source inventory does not by itself establish client compatibility.

### Automations

In local mode, **Connections → Automations** offers two actions:

- **Start quota countdown**: a small request to the selected model after
  the window recovers, to start its next reset countdown when the provider
  uses that mechanism. This request consumes quota and runs automatically
  when the enabled rule's condition is met.
- **Reset weekly quota**: when the weekly window is exhausted, Relay tries
  to use an available provider reset credit. It does not need a model request.
  Enabling the automation authorizes subsequent resets without confirming
  each one separately.

Choose the **Automation type**, then the accounts and, if it sends a request,
the model. **Name** is optional: it defaults to the type's name. A custom name
helps distinguish rules for different accounts.
Save and enable the task. Local automations run while Relay
is running; server automations depend on Relay Server capabilities.
Existing local rules that required a manual run become automatic after an update;
disabled rules stay disabled. No separate start button is needed.

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

Add saved connections in **Members** and open their policies. In the add dialog,
choose accounts or API sources, search by name or address, and select the rows
you need. Selection persists while searching and switching sections. Review
the complete set under **Selected** before clicking **Add to pool**.
**Select shown** only selects the current search and filter results.

Removing a
member from the pool does not require deleting it from **Connections**.
Disabled members and accounts requiring sign-in, denied access, or without
available quota are skipped. Their presence does not block other members.

Eligibility is checked for the specific model and request format. A working
pool can therefore lack a route for one model. A temporary restriction on a
single model does not necessarily block the member's other models.

The 1.1.3 update switches existing profiles to the current pool rotation automatically.
No separate confirmation or gateway stop is needed. Saved member order, request
shares, concurrency limits, pool membership and gateway enabled state remain.
The former Smart mode becomes Automatic; In order and Round robin keep their
mode. There is no separate migration notification or old-scheduler rollback.

New profiles also use this rotation. Older servers without support for these settings
cannot accept them; update your own server before using the rotation editor.
Older configuration presets are converted on import without adding permissions.

**Pool rotation** has three modes:

- **Automatic** chooses the eligible members with the lowest share of occupied
  local capacity. Request shares break ties between equally loaded members.
  Manual order, balance and quota percentages do not rank them; confirmed
  quota or access blocks still exclude a member. Cache affinity only applies
  within the same eligible, equally loaded group.
- **In order** chooses the first eligible member with a free slot in your
  list. If it is unavailable or at its concurrency cap, Relay checks the next
  members. New requests return to it when it recovers. Only this mode lets
  you reorder members manually.
- **Round robin** distributes new independent requests among eligible members
  according to their request shares. Equal shares alternate; unavailable
  members and those at their concurrency cap are skipped.

In every mode, a chat continuation may need its previous member. Rotation does
not promise a different account for every message in the same conversation.

Mode, order, request share and concurrency changes save immediately. Drag a
member by its handle or use the arrows in **In order**. **Close** waits for
pending changes. The list follows changes to pool membership automatically;
if saving fails, an error appears and the stored values are shown again.

**Request share** is a ratio for Automatic and Round robin. For example, 2 and 1
give roughly two parts of traffic to the first member and one to the second
when they are equally available. In Automatic mode this ratio applies only to
members with equal normalized load, not to all traffic. It is not a percentage,
requests per second, or extra quota. **Concurrent requests** limits how many requests one member
can serve at a time, across its models and formats. A value of 2 allows two
simultaneous requests. **Unlimited** removes the member-specific cap; runtime safety and provider
limits still apply.

Cards group members by readiness, quota wait, unavailability, and disabled
state. In the rotation dialog, **In order** preserves the manual queue even
when states change; automatic modes show ready members first.
The current, last-used, and **Next candidate** indicators have different
meanings. Next candidate appears only when the choice for a new text request
agrees across enabled models and formats. An absent hint does not mean the
pool has stopped.

### Failures and retries

Recovery is automatic, but each request has a shared attempt limit: three
upstream sends by default, including auth and compatibility retries. Relay first
tries an untried eligible physical member; changing a protocol alias is not a
new independent source. A retry requires proof that the failed operation was
not accepted, plus enough context to replay it safely. An unexplained server
error, broken stream or disconnect after send does not provide that proof.

Transient inference failures are paced per route. The first two independent
request failures wait 250 and 500 ms; three open the circuit, with a retry delay
starting at 2 seconds and increasing up to 60 seconds. Retries of the same
logical request do not repeatedly increase its failure count. Provider retry
and quota-reset deadlines remain mandatory, including for the last member;
configured member delays cannot shorten explicit provider hints. A model-scoped
block does not disable the source's other models.

Recovery waits hold no slot. The normal retry window starts at the first safe
rejection and lasts up to 30 seconds. Capacity/recovery waits share a separate
30-second accumulated queue budget across all retry passes and transports.
Waiting also has pool-wide and per-key request-count and memory limits; a full
queue returns a local error without penalizing any provider. Cancelling removes
the waiter immediately. A half-open recovery request and the pool's recovery budget prevent
every waiting request from testing the same failed route at once. The
**API → API → Wait for route recovery** option can extend waiting for text
requests on all four input formats, but cannot reset
the send limit or retry an uncertain or already-visible result.

Relay does not limit the duration of an active generation. Long reasoning or a
pause in output does not terminate or resend the request. Relay keeps streaming
connections alive and waits for provider completion or failure; you can cancel
the request in the client. The client, proxy and provider may have their own
timeouts. Connecting to an unreachable address still has a bounded timeout.

Relay can retry with another member only after a proven safe rejection and
before response data reaches the application. It does not combine an already-started answer with another
provider's output. Moving a conversation continuation also requires sufficient
saved history. Saved history proves portability, not whether a failed send ran. A response reference or tool result without its required
context may need the original member; Relay does not silently discard that
context. Invalid requests, such as excess context or an invalid tool call,
cannot be repaired by trying more members.

### Models and member policies

ChatGPT/Codex shows GPT models under their original IDs, including models
available through an API provider when the signed-in account lacks them.
The pool and request key still determine access. A missing native catalog card
does not rename a model or grant it additional tools or reasoning modes.

Picker names come from the specific model's
catalog metadata. Without a name, Relay builds a compact label from the ID.
New models do not need a separate version list. Matching names do not merge
distinct models; technical IDs remain available for routing. Membership in
the OpenAI group does not replace the shared reference and Relay rules for other capabilities.

In **Pool member policy → Models**, a switch allows the model for that
specific account or API. It is permission, not a quota indicator. Search and
expandable groups help locate models.

The pool's **Model Rules** tab enables or disables a model for the whole pool.
It also controls model and group order, available reasoning modes, and
per-model speed. Model order affects catalog presentation; member order in
rotation controls connection selection. The application's model list is also
limited by client compatibility.

**Reset model order** in the pool toolbar clears manual model and group positions.
Companies start with OpenAI, Anthropic, Google and xAI, followed by the others
alphabetically. Within each company, catalog families stay together,
ordered by their newest release; versions within a family run newest first.
Version ties use the update date, then discovery order. Models without a family
follow known families; undated versions follow dated versions in their family.
Newly discovered models and families follow these rules automatically.
Reset leaves model switches, prices, reasoning and rotation settings intact.
On a remote pool, the button requires a server supporting order reset.

Model Rules retains every model of pooled members. Missing sign-in, keys,
proxies or compatible routes do not remove names, groups, prices or known
reasoning modes. Matching model IDs share one row; similar display names do
not merge distinct IDs. Relay checks request availability when selecting a member.

**Request speed** offers **Standard**, **Fast**, and **Ultrafast** for OpenAI
models. This is a Relay family rule: account/provider speed lists and temporary
unavailability do not change the choices. Select a pool default or a preference
for a model. An explicit speed from the application wins and is preserved when
Relay switches members. This requests a processing tier; it does not guarantee
response time or add quota.

Model names, groups, reasoning, tools, images and limits come from shared
reference catalogs. Missing fields use Relay defaults; unknown limits and
reasoning levels remain unspecified. A participant's empty or conflicting
capability fields do not replace this information. Prices are the exception:
a participant's declared price is used when available.

### Prices and additional settings

API prices are in **Pool member policy → Pricing** and in the source editor
under **Connections**. Estimates use provider prices first, then matching
catalog prices, then manual prices if neither is available. A manual price
does not change the provider's tariff or unconditionally override other prices.

Token prices are in USD per million tokens. A manual set requires input and
output prices; the reset button removes that manual set. The 5-minute and
1-hour cache-write fields appear when a Messages route permits manual pricing
or the model has a price explicitly tagged with that lifetime, even if its
catalog came from another endpoint. A dash means the price is unknown. These
fields price cache creation; they do not enable request caching or prove that
the source accepts a cache-control option.

Request details show cache reads and writes from the provider's usage response.
Relay shows a write lifetime only when that response reports one; otherwise it
marks the lifetime as unreported. Model documentation is a separate note. For
GPT-5.6 and later, OpenAI documents a 30-minute minimum after the latest write
or reuse, but usage does not provide a live remaining-time countdown.

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

**Copy key** copies Relay's request key. It is different from an external
provider key or a server management token. In the menu beside it, **Reissue API
key** asks for confirmation, then replaces the key and copies the new one.
Update your clients afterward. Simply copying the key does not change it.
Local **Port** settings are below the address and key. Save a changed port to
apply it; a running API restarts at the new address. Update the address in
your applications or connect them again.

All four formats use the same pool request key and model permissions:

| Format | Local endpoint | Authentication |
| --- | --- | --- |
| Responses | `http://127.0.0.1:14998/v1/responses` | Bearer key |
| Chat Completions | `http://127.0.0.1:14998/v1/chat/completions` | Bearer key |
| Messages | `http://127.0.0.1:14998/v1/messages` | `x-api-key` or Bearer key |
| Gemini | `http://127.0.0.1:14998/v1beta/models/{model}:generateContent` | `x-goog-api-key` or Bearer key |

Gemini streaming uses `:streamGenerateContent?alt=sse`. Replace the host/port
with the displayed server address when using a remote pool. Relay selects the
model's native format and converts the application's request automatically
when needed. Unsupported conversion options fail before generation;
native routes preserve provider-specific parameters. Realtime, cross-format
WebSocket, audio/video conversion and server-side tool emulation are not offered.

### Model substitution protection

**Model substitution protection** is available in the **API** tab.
It enables the alternative **Excel / Basis Points**
route for compatible OpenAI accounts. This mode is intended to work around
possible substitution of the selected model, but Relay
cannot verify which model actually runs the request at the provider. Changes
save immediately and apply to all compatible accounts. The route used is still
shown in request details under **Usage**.

The route shares the account's quota and rotation slot. Applications can use
any of Relay's four supported request formats; client tool calls and results
are translated into the calling format. If the application requests a stream,
SSE events arrive after the provider completes the response, without
incremental output during generation. Images and explicitly requested fast
speeds cannot use this transport; they need another compatible route.
Continuation by `previous_response_id` is also unsupported: Relay rejects it
explicitly rather than losing context. Send complete history without this
field or use another route.

If Excel / Basis Points returns `adapter_upstream_response_invalid` with
`output.run_officejs.code`, the model generated invalid JSON for a tool call.
Relay does not execute that call or cool down the account; retry the request
manually. Share the error code and request ID, if available, for diagnosis;
do not share tool arguments.

### Tool optimization

Tools let an agent perform actions such as reading files or searching.
Applications send their descriptions with requests. Relay can either send that
catalog normally or ask a compatible provider to load large schemas on demand.
Relay never hides tools by name or uses this setting as an execution permission
boundary.

#### Setup

1. Open **API → Tool optimization** in **Computer** mode or on a compatible
   **On your server** connection. Older servers may not offer this setting.
2. Turn the switch on or off. The choice saves immediately and applies to new
   requests without restarting the API. In-flight requests keep their original
   setting, including retries.

#### Modes

- **Standard** (`pass_through`) is the default. Relay forwards the complete
  catalog without changes.
- **Optimized** (`automatic`) enables the provider's hosted `tool_search` and
  deferred function-schema loading for every eligible native Responses request.
  The provider chooses which schemas to load, while all original definitions
  remain available to it. Catalog size does not affect whether optimization is
  applied. Other protocols, converted routes and explicit tool choices keep the
  normal catalog behavior; Relay does not guess relevance from prompt text or
  truncate the list.

Provider-native deferred loading is used only where the selected route is
native Responses and the request uses automatic or unspecified `tool_choice`.
Individual flat functions still expose their names and descriptions; namespaces
provide larger context savings because their parameter schemas stay out of the
initial model context. Relay does not invent namespaces or perform local
semantic search. If an endpoint rejects the standard `defer_loading` fields, Relay
retries once before output without deferred loading. Client/provider deferred
or tool-search catalogs are not narrowed by Relay.

#### Limits and results

Deferred loading adds only the standard provider hint; it does not rename tools
or rewrite their schemas. Built-in provider tools and unknown tool kinds are
preserved. This setting is not an execution-permission boundary: use the
application's own permission settings to control which actions may run.

**Usage → request details → Tools** shows input/forwarded counts, whether
provider deferred loading was used, a compatibility retry when applicable, and
**List size (bytes)** before and after forwarding. The size measures compact
catalog JSON, not tokens or guaranteed monetary savings; provider-reported
usage is the source of truth. Diagnostics store aggregates only, not tool names
or schemas. A standard request keeps the before/after counts and sizes equal.

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

Under **API → API**, **Wait for route recovery** holds text requests through
Responses, Chat Completions, Messages and Gemini when configured eligible
members are temporarily unavailable, until recovery or client cancellation.
Image generation is not covered. Without it, the client receives an error after
normal attempts. Waiting cannot fix invalid requests, waive continuation-history
requirements or replay a generation already accepted upstream.
On a remote pool, this control requires a Relay Server advertising
`route_recovery_v1`; update an older server to enable the four-format behavior.

For a server pool, the ChatGPT tab connects to your server when that capability
is supported.

Relay publishes Fast and Ultrafast for OpenAI models automatically. An open
Codex keeps using its loaded catalog. On the next launch through Relay, pending
updates are applied before the client opens. If an update fails, the previous
catalog is retained and a warning appears. Reconnecting the pool also updates
the catalog. The selected speed remains unchanged across accounts and API
sources; observed processing tier and usage come from the upstream response.

**Ultra in Codex's reasoning picker** is a Codex subagent orchestration mode,
not a higher API reasoning effort. Relay offers it for an exact model that the
installed Codex marks as eligible when the pool can route Max and the required
subagent effort. Codex's Ultra toggle must also be enabled. If the option is
still absent after a catalog update, relaunch Codex through Relay. This is
separate from the **Ultrafast** speed tier.

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
When routing changes the model ID, **Requested model** shows the client's ID
and **Sent to source** shows the ID Relay sent to the member. The latter does
not prove which model the provider used internally.

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
Disconnecting undoes only unchanged Relay-owned `config.toml` fields and restores
the previous sign-in only while the current one still belongs to Relay. New
manual sign-ins and settings edited outside the app stay in place; reconnecting
explicitly saves a new sign-in as the next restore point before applying the
selected Relay connection. Automatic rollback never replaces a new sign-in.
This differs from explicitly restoring
a complete named snapshot, which replaces its contents.
If the Relay-created model catalog file changes outside the app, disconnecting
restores the previous settings and sign-in but leaves the edited file untouched.
A catalog refresh will not overwrite it. Before reconnecting, preserve or move
that file out of the backup folder if needed; Relay will not replace it without
verification.

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
| `admission_queue_full` | Relay reached the waiting-request count or retained-memory limit for the pool or request key. No new provider request was sent. | Reduce concurrent waiting requests or their input size, let queued requests finish, then retry. |
| `admission_wait_expired` | The logical request exhausted its total admission wait or retry deadline. | Retry as a new request after capacity recovers; review concurrency limits or the explicit wait-until-available setting. |
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
| `upstream_model_unsupported`, `model_not_supported` | The selected provider path does not support this model. | Refresh the source catalog and verify the model ID, API address, key permissions, and provider support. Relay will use another compatible member when one is available. |
| `upstream_model_capacity`, `model_at_capacity` | The model is temporarily overloaded. | Wait or use another compatible source. Signing in again does not increase provider capacity. |

### Requests, history and tools

| Code or message | Cause | Action |
| --- | --- | --- |
| `invalid_request`, `upstream_invalid_request` · 400 / 422 | The body or a request parameter is invalid. | Fix the field named in the redacted message. Requests need a JSON object, nonempty model, and valid `stream`; path/body models must agree. Compact responses do not support streaming. |
| `invalid_stream_id` · 400 | A Responses WebSocket `stream_id` is invalid. | Use 1–256 ASCII letters, digits, `_`, `-` or `.`; omit the field for the default stream. |
| `upstream_context_too_large`, `context_too_large`, `context_length_exceeded` | History exceeds the model context. | Shorten history/attachments, summarize, start a new conversation, or choose a model with a larger context. |
| `request_too_large`, `upstream_payload_too_large` · 413 | The request body or attachments exceed a size limit. | Reduce or split input files. Relay's incoming image-request limit is 64 MiB; the provider may impose a smaller one. |
| `request_encoding_unsupported` · 415 | Unsupported or stacked request compression. | Use an uncompressed JSON body or a single `gzip` / `zstd` encoding. |
| `request_encoding_invalid` · 400 | Compressed input is corrupt, incomplete or needs a decoder window above 64 MiB. | Update the client or send an uncompressed request. Both compressed and expanded JSON are limited to 64 MiB. |
| `compaction_response_invalid` · 502 | Context compaction did not finish with a valid encrypted result. | Keep the existing conversation history and retry explicitly after checking the upstream connection. Relay does not replay this generation or fabricate a summary. |
| `upstream_instructions_required`, `missing_required_parameter` | A required field, including instructions, is absent. | Supply the field named by the provider or update the client generating it. Retrying the same body does not fix it. |
| `upstream_unsupported_request`, `unsupported_request` | A parameter or capability is unsupported. | Disable the named parameter, tool, or mode and use a compatible format. |
| `upstream_content_policy`, `content_policy_violation` | Provider content rules rejected the request. | Revise the request according to the service rules. Rotating members is not a correction for that request. |
| `response_continuation_unavailable`, `response_affinity_miss` | The response owner is unavailable and full replay history is missing. | Restore the original account/API or resend complete history from the client. Start a new conversation if history is lost. A rotation mode change cannot restore context. |
| `upstream_previous_response_not_found`, `previous_response_not_found` | The provider no longer knows the previous response. | Resend full history without the stale response reference, or start a new conversation. Do not transfer just a response ID to another API. |
| `upstream_tool_call_mismatch`, `tool_call_not_found` | A tool result has no matching call, or a call has no result. | Relay retries once when the complete pair proves a missing or confused call identifier. It never removes results or guesses between parallel calls. If the error remains, update the client and resend the complete call/result pair, or start a new conversation if the missing history cannot be recovered. |
| `upstream_encrypted_content_invalid`, `invalid_encrypted_content` | Stored encrypted reasoning context is not accepted. | Return to the original connection or start a new task with ordinary history. Do not manually edit encrypted blocks. |
| `tool_use_not_supported`, `chat_feature_not_supported` | This route cannot represent the requested tool or feature. | Function tools and their results are supported on compatible Chat Completions routes. Check the model's format capabilities; choose a matching native route or remove the specifically unsupported option. |
| `upstream_conflict`, `conflict` · 409 | State changed or another operation is in progress. | Wait for the earlier operation, refresh state, and retry once. Restore conversation history when the conflict concerns continuation. |
| `upstream_candidate_rejected`, `source_rejected` | A route rejected the request without a more specific category. | Read the provider code/message and check model, permissions, and format. A new independent request can use a compatible reserve. |

### Adapters and images

| Code or message | Cause | Action |
| --- | --- | --- |
| `adapter_binding_unsupported`, `source_protocol_invalid`, `source_pool_protocol_unsupported` | Relay could not build a compatible automatic route for the requested protocol and model. | Refresh the source catalog and verify its API address and model ID. Use another member when the provider does not expose a compatible native path or translatable format. |
| `adapter_invalid_request` | The adapter cannot translate the request. | Remove the field named in the message or choose a native-format source. |
| `adapter_parameter_unsupported` | A meaningful request parameter has no lossless mapping on the selected route. | Check the field named in the message and `error.param`. Use a native route or change that option. Encrypted input history requires its compatible native route; do not delete it from the conversation. Relay does not silently discard it. |
| `adapter_compaction_unsupported` | The adapter cannot perform this compaction operation. | Use a native Responses route for compaction or start a new task with summarized ordinary history. |
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
