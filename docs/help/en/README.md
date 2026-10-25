# Zenith Relay

Relay connects an installed ChatGPT application, Claude Desktop, OpenCode,
or another API client to your accounts and API sources. Use one source directly or build a
pool that chooses an available connection for each request. Conversations
and work with the model stay in your chosen application.

[Quick start](#quick-start) | [How Relay works](#how-relay-works) |
[Overview](#1-overview) | [Connections](#2-connections) | [Pool](#3-pool) |
[Access](#4-access) | [Usage](#5-usage) |
[Recovery](#6-recovery) | [Settings](#7-settings) | [Errors](#8-errors)

## Quick start

You need your own account or provider API key and an installed application
to work in. Relay does not install ChatGPT, Claude Desktop, or OpenCode.

### Connect an application through the pool

1. Select **Computer** mode.
2. Add at least one connection:
   - account: **Connections → Accounts → Sign in → ChatGPT** or **Excel / Basis Points**,
     then **Open sign-in window**;
   - API: **Connections → Sources → Add source**, select the service
     and enter its key.
3. Open **Pool → Members → Add**. Select the saved connection and add it.
   Skip this step if the connection is already a pool member.
   Its card should appear in **Members**.
4. In **Pool → Model Rules**, check that the model you need is enabled.
   If it is missing, refresh the connection and check your access with
   the provider.
5. In **Access → Local API**, select **Start API**.
   The status should change to **API is running**.
6. Open **Pool → Connect** and select ChatGPT, Claude Desktop, or OpenCode.
   Claude Desktop uses the exact ordered pool model list and its third-party
   inference profile. Relay validates the connection, closes a running Claude
   Desktop and saves the profile before any optional launch.
   The application opens after connecting by default. To turn this off, disable
   **Settings → Application → Launch applications after connecting**.
   For another client, use
   [manual API setup](#local-api).
7. Select a model in the application and send a short request.
   Open **Usage → Requests** in Relay to see its result and selected member.

Use **SIWC** or **Excel / BPS** for text; opt Codex into images with
**Add to pool for images**. The client badge selects the route; **Connect**
adds its separate sign-in/quota. Switching preserves sign-ins, **Notes** and
weight/concurrency, but clears image-only opt-in. Finish active requests first
and fix incompatible automation models: rules follow the selected route.
Codex/BPS can use saved Notes for email/password/authenticator 2FA on all three
OSes; new accounts, missing Notes and SIWC stay manual. See [Account sign-in](#account-sign-in).

Quick Setup configures mode, connection and client without launching it.
Skip or repeat it via **Help → Repeat quick setup**. Claude Desktop needs Computer mode.

### Work without the pool

- **Codex:** **Launch in ChatGPT** switches accounts using native credentials. SIWC/BPS use the pool only.
- **API:** add a source in **Connections → Sources**, then **Launch** it
  with your application.

Direct requests bypass rotation and the **Usage** log. Return to the pool
through **Pool → Connect** in **Computer** mode.

## How Relay works

A pooled request follows this path:
**application → Relay API → eligible member → provider**.

- **Connection:** a saved account sign-in or an API address with its key.
- **Pool member:** a connection allowed to receive requests through Relay.
  Saving a connection and adding it to the pool are separate actions.
- **Model:** an exact provider model ID. Similar names do not imply
  identical access.
- **Client:** the application that sends requests. Connect it to the pool
  after adding members.

### Operating modes

| Mode | When to choose it |
| --- | --- |
| **Computer** | The pool and its API run on this computer. The usual starting point. |
| **Relay Hosted** | In development. |

In **Computer**, closing the window keeps Relay in the tray; quitting stops
the local API and automations. Mode changes do not stop it or transfer secrets.

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

**Overview** shows pool activity, connections, models, errors, and charts.
Filter by period and connection; open **Usage** for individual requests.

### Reading the charts

**Weekly quota in $**, after the speed charts, estimates the full
weekly allowance from your requests. **All models** of the selected subscription
is the default. Each account counts once; Plus and Pro are estimated separately.
Subscription and model
menus appear only when there are alternatives; hover or focus a point for its date,
amount and estimate range when available. Subscription comparisons use measured quota consumption.

Measurements appear after at least 5% observed weekly consumption with known
token prices. Collection uses ordinary request data and continues while
Relay is minimized. Linked Codex/BPS/SIWC routes count as one proven weekly ledger;
SIWC requires that weekly evidence from Codex/BPS. Evidence
from the last 24 hours is preferred, with fallback for at most 72 hours and
never beyond reset. History lasts up to 30 days; missing data appears as a gap.
Resets and subscription/pricing changes are separate events. External account
use and quota update lag can bias estimates.

**Today** shows 24 hourly intervals including the current hour; **Week** shows
7 days and **Month** 30 days. Times are local. The connection selector above
the charts limits their data to that account or API source.

The top **Requests** and **Errors** counters use the same filter; errors count
all recorded unsuccessful requests. **Connections** counts enabled accounts and
API sources. **Models** counts models
published by the pool API.

| Chart | Meaning |
| --- | --- |
| **Token usage** | Input includes instructions/history/tools and cache reads/writes; output includes reported reasoning. Left axis: tokens; right: cache-hit %. |
| **API equiv. used** | Estimated cost at API prices. Coverage depends on available prices. |
| **Requests** | Requests per interval, including errors; individual outcomes are in **Usage**. |
| **Generation speed** | Output tokens/s after the first token, excluding reported reasoning and initial waiting. |
| **E2E speed** | Output tokens / full duration including initial waiting. |

The legend labels colors. Dashed purple **Cache hit rate** is the cached share of input.
Headers summarize the period; speed uses total tokens/time. Hover or focus points
for exact values. Gaps and **—** mean unmeasured. Loading errors show a warning.

### Balances and estimates

Balance and quota come from providers; **API equiv. used** is Relay's estimate at API prices.

**No balance API** means unreadable. **Stats access denied** is a
statistics refusal; model requests may still work. Failed refreshes retain
the last value, marked stale.

## 2. Connections

Save accounts, API sources, and proxies, and configure automations here.
Account behavior settings are under **Access → Accounts**.

<!-- relay:topic:sign-in -->

### Account sign-in

Open **Connections → Accounts → Sign in** and choose a method:

- **Codex** — the default; account switching, linked SIWC quota display, and explicit image-only pool use.
- **ChatGPT (SIWC)** — Relay sign-in window, for the pool.
- **Excel / Basis Points** — pool use; access is checked with the provider (BPS).

1. Select **Open sign-in window**.
2. Sign in. Relay saves the account and closes the dialog.
3. Add the account to **Pool** manually when needed.

Every method has **Copy sign-in link** for browser recovery on the same computer.

**Notes** saves email, phone, password and 2FA with the account. A new-account
draft survives folding, clears on cancellation, and does not autofill login forms.

The password/2FA **Copy** button copies the whole value without revealing it.
For selected text, use **Ctrl+C** (**Cmd+C** on macOS) or the context menu.

For an existing **Excel / Basis Points** account, right-click **Sign-in required**
to use saved email/password and optional authenticator 2FA. Left-click stays manual.
This works on Windows, macOS and Linux with the account's isolated profile/proxy.
CAPTCHA, additional checks, consent, unfamiliar forms, errors or a 45-second limit
reveal the window for manual continuation. **Open sign-in window** does so at any
time. Email/SMS codes and social sign-in remain manual. Relay validates account
identity before saving; adding a route never reuses another client's grant.

For SIWC, authorize plan usage during sign-in. Codex/BPS counters do not confirm SIWC limits.
If SIWC confirms exhausted quota before generation, Relay cools that connection
and selects the next eligible pool member. Once generation starts or execution
is uncertain, the request is not replayed.
Use the same account and method when signing in again.
The close button or Escape cancels sign-in before saving.

<!-- relay:topic-end:sign-in -->

### Import, export, and account state

**Import** accepts files and pasted data. Review the recognized records first,
then select those you want and confirm the import. You can also drop a file
into Relay. Enable **Add selected to pool after import** only if wanted:
it applies to eligible pool connections and API sources, not Codex. After
importing Codex, use **Add to pool for images** on its account card if wanted.
Free plans can be imported and added to the pool; actual access and quotas are
determined by the provider, not the plan label.
After a partial failure, you can retry only the failed records.

Zenith preserves all three sign-in methods. SIWC supports only Zenith;
sign in again after import before use. Existing SIWC connections are not
overwritten. Do not run copied refresh tokens on two devices.
BPS supports Zenith and Sub2API.

Cards show plan, quota, resets and state. **Refresh** requests current quota data
from the provider. Linked Codex/BPS can supply missing SIWC displays and fresh
rotation ranking. SIWC access depends on its authorization and own limits.
Hover/focus shows source/time; amber means stale. Refresh reads
that source without extra polling. Credits and quota have independent freshness
windows. Rotation uses fresh credit observations;
the last balance stays visible until a complete poll.

BPS cards also show token expiry (hover for date; `—` if unknown).
A refresh token may renew access; a revoked session needs sign-in.
Retry network failures later; credential recovery preserves your enabled state
and membership. Follow the card's sign-in hint. The shared visibility control
reveals account details hidden by default.

**Remove from pool** keeps the connection but stops new assignments.
Left-click **Delete** in **Connections** to move the account to **Trash**, retaining its
sign-in data while stopping pool participation and background tasks.
Use **Connections → Trash → Restore** to return it disabled and outside the
pool; enable it and add it to the pool separately when needed.
Right-click the same button to open the **Delete permanently** menu. Selecting
the action asks for confirmation, then removes the account and its sign-in
data without recovery. In Trash, this action is at the bottom of each card.
Exports from a card menu or selected accounts contain sign-in data:
store them like passwords, and do not attach them to an error report.

### Account timezones

The timezone follows the assigned proxy's observed exit IP automatically.
Hover over the proxy for its timezone and check time.

Checks run in the background even while Proxies is closed. An observation stays
valid for 15 minutes and refreshes before expiry; failures retry after a delay.
Model requests never wait for geolocation. When no current timezone is known,
the client's original timezone is kept, without guessing from country or latency.

Relay changes existing timezone fields in the account's request environment
and web-search location, including HTTP and WebSocket requests.
Dates, ordinary text, system timezone, and API-source requests stay unchanged.

### Proxies

Proxies are optional. In **Connections → Proxies → Import**, add one address
per line, then assign a proxy in the account's settings. You can also select
a sign-in proxy using the lock in the sign-in dialog.

**Test proxy** shows country/city and HTTPS-check time.
Hover over the location for exit IP, timezone, and the saved check date.
Model access is checked separately; a failed proxy never falls back to direct.

The bottom strip keeps three actions: connectivity, **Test speed**, and accounts.
The speed test runs only on click and downloads a 5 MiB sample; its result appears
in the middle of the card. Multiple tests run in order; waiting cards show
**Queued…** until measurement starts. Hover over the people icon/count for **Accounts**.
Set an optional name through the card's **Rename proxy** menu; the address and
assignments stay unchanged.

Exit country is automatic; city/timezone use an optional online lookup.
A failed city lookup keeps the country.
IPv4/IPv6 conflicts hide region/timezone; hover for IPs. One available IP family is sufficient.

New, unlocated, and legacy proxies are checked once automatically; use
**Test proxy** to refresh. The detected timezone applies without restarting.

Each card shows account and API-source assignments. Deleting an account proxy
asks for confirmation and returns those accounts to the common proxy, or to a
direct connection when no common proxy is set and direct access is permitted.
First replace an API-source proxy in the source settings. For a common proxy,
use **Clear common proxy** on this tab and confirm before deleting its entry.
This removes the common setting, not entries in proxy storage or explicit
account assignments. Subsequent inherited requests use direct access only
when pool policy permits it; otherwise they remain unavailable.

### API sources

1. Open **Connections → Sources → Add source**.
2. Select the service or **Custom API**, and enter a name, address, and provider
   key. **Get API key** opens the selected service's console.
3. Save the source and check the discovered models. If the catalog did not
   load, check the address, key, and error on the card.
4. Add the source to **Pool** or select **Launch** for a direct connection.

A preset fills in the address and request format. Obtain a key and check model
access with your provider. For **Custom API**, use its specified address and format.

Direct ChatGPT launch requires a Responses endpoint. If direct launch is
unavailable because of the API format, add the source to the pool and connect
the application through it, provided the pool supports the required conversion.

**Moonshot / Kimi API** and **Kimi Code** are different connections. Choose
the preset for your key and console region. Relay does not currently read
Kimi Code quota or MiniMax balance and M Plan quota; check them with
the provider. Other supported services are listed in the add-source dialog.

### Automations

In **Connections → Automations → Add automation**, choose an action, pool
accounts, and a model if required. Actions can start a countdown after
the primary quota recovers or request a reset when weekly quota is exhausted.
Save the rule and enable its switch; its card shows the result.
After saving or enabling it, Relay checks current quota. An already-full window
is considered without waiting for another transition to 100% or repeating a wake
in the same cycle.
If the selected model is no longer available for the accounts, edit the rule
and choose another. Relay keeps your selection without replacing it automatically.

Starting the countdown sends a request and may use quota. A reset requires
support from the provider. Automations do not add subscription allowance.
Tasks run while Relay runs.
There is no manual run button.

### Saved account recovery

The old personal-server desktop mode is retired. Saved settings open **Computer**;
they do not connect to Relay Hosted or delete server credentials or inactive local
account recovery records. Stop the old server's use of an account before choosing
**Recover local account**. Relay does not contact that server; the recovered account
remains disabled and outside the pool until you explicitly enable it.
**Relay Hosted** is a separate service in development. To share this computer's
pool, enable **Access → Local API → Allow local network access**.

## 3. Pool

### Members and route selection

**Members** contains SIWC connections with plan-use consent, eligible BPS connections, and
API sources added to the pool. In **Connections**, use **Add to pool** on a
saved account or the bulk action; addition is immediate without an account-kind warning.
Cards show which members are in rotation, waiting for quota, disabled,
or unavailable. **Next candidate** previews the choice for the current pool state.

Select members with the button on each card, or use the checkbox above the cards
to select the entire pool. The same toolbar then offers refresh, enable, disable
and **Remove from pool** for the selection. Removal asks for confirmation and keeps
accounts, API sources and their credentials in **Connections**. After a partial
failure, selection is retained and cards show the persisted state.

### Pool rotation

<!-- relay:topic:pool-rotation -->

Cards follow the scheduler's current preference, left-to-right then down, without
highlighting. Ready candidates come before waits, unavailable and disabled
members; timed waits use retry order. If model/protocol routes disagree, the
remaining cards use fresh limiting quota, then usable provider credits in automatic
mode, or saved order otherwise. Selection accounts for model, load and session
ownership. The view reflects current rotation state; ongoing requests retain their member.

In **Pool rotation settings**, **How pool rotation works** opens this Help
without closing your settings. The settings window keeps only the controls.

#### Modes

- **Automatic:** Relay balances load, fresh quota or credits, and request
  shares. A quota advantage below five percentage points does not force a
  switch; a lead of five points or more excludes the lower remainder.
- **Round robin:** independent requests cycle through available members in
  saved order. **Use custom request shares** enables weighted distribution.
- **Primary & reserve:** the first compatible available member handles requests;
  later members are reserves when earlier ones are unavailable or full.

Reorder members by dragging or using the arrows. All modes skip incompatible
models, unavailable members, and members at their concurrency limit.
Shares target relative request counts;
protocol-owned conversations and different route availability affect distribution.
**Concurrent requests** limits load on a member;
**Unlimited** (0) removes Relay's additional limit; provider limits still apply.
Changes save automatically.

#### Presets

Three buttons apply a ready-made setup in one click:

- **Recommended:** automatic selection by load and available resources.
- **Even distribution:** independent requests cycle in the saved order.
- **Accounts → API:** primary/reserve routing with accounts before API sources.
  The order inside each group is preserved. Reserve API requests may spend
  their balance.

Presets reset shares to 1 and extra limits to 0. Strategy-only changes keep
both. Manual overrides show **Custom**.
Switching strategies does not restart applications or the gateway, interrupt
active requests, clear cooldowns, or move an opaque continuation to another account.

<!-- relay:topic-end:pool-rotation -->

### Failures and retries

The pool may select another member after a refusal before generation.
A started or uncertain request is not automatically replayed on another account.
Continuing a conversation may also require its original member.
Having several accounts therefore does not guarantee that every interruption
will be invisible to the client. You can cancel a wait in the client.

### Models and member policies

There are two permission levels:

- **Pool member policy** determines which of that member's models can be used.
- **Pool → Model Rules** controls pool-wide models, their order, reasoning
  modes, speed, and visibility in ChatGPT.

A model must be enabled at both levels and available from the provider.
Permission in Relay does not unlock a restricted model or replenish quota.
**API only** means the model is not shown in ChatGPT's model list.

Models are grouped by developer, then family and class, with newer versions
first. Accounts and API sources share Relay's order.
Drag models within a group or move a whole group.
**Reset model order** restores Relay's model order.
Speed choices come from the model reference and compatible pool
connections. **Fast** and **Ultrafast** appear only when both support them;
they do not promise a response time. BPS uses **Standard**.
Text models also have a **Context window** control. **Short context** preserves
the model's published default window. Only when that window is unknown does it
use 25% of a known maximum. **Long context** uses the full known window. The
control shows tokens without percentage captions; hover to see the reference maximum.
**Custom limit** accepts
1,024–2,000,000 tokens, capped by a known hard maximum. Changes save automatically;
a valid custom number saves after a short pause or on Enter. Escape cancels the draft.
Relay updates without restarting the pool or interrupting requests. If Codex
is running, a compact status icon explains that it needs a restart to reload
its catalog. Refresh failures are also available by hovering or focusing the icon.
OpenCode may need a new session. Claude Desktop manages its own history;
Relay does not forcibly trim it to this budget.

### Prices and additional settings

In **Pool member policy → Pricing**, choose **Tier** (Standard/Flex/Fast/Ultrafast)
and **Context** (price thresholds, e.g. ≤ 272k / > 272k, or **All sizes**).
Ranges come from model prices or saved manual settings.
These selectors choose the prices to edit. Estimates
use actual tier/input size; missing prices show `—`, without Standard fallback.

Relay prefers the provider's price, then the reference catalog; a manual
price is a fallback. Cache fields set read and write prices.
**Purchase cost, USD** is only used to estimate payback.
**Drain** stops new assignments to a member.

Use **Configuration preset** to save settings or import them with a preview
of changes. A preset contains pool, model, and price rules; it does not include
accounts, keys, quota, or request history.

## 4. Access

Configure access to the pool and connected application behavior here:
**Local API**, **Applications**, and **Accounts**.

### Local API

1. Start the API and copy the **API endpoint**.
2. Select **Copy key** and paste it into the client's API key field.
3. Choose a supported request format and the exact ID of an enabled model.
4. Send a request and check it in **Usage**.

The usual base address for an OpenAI-compatible client is:

```text
http://127.0.0.1:14998/v1
```

In **Computer** mode, the pool runs on this device.
Use the displayed address; the port may differ.
`127.0.0.1` is local only. For another computer or phone, enable
**Allow local network access** and copy the **Network API address** and
**Network API key**. Start the API separately. Local profiles and keys stay
unchanged, including when rotating the network key. Disabling sharing
closes network access.
Use trusted private IPv4 networks; HTTP is unencrypted. Allow the port
only on private firewall profiles; never forward it to the internet.
Browser-origin requests and account management are blocked.
Do not add `/v1` twice.

**Which key goes where**

| Key | Where to use it |
| --- | --- |
| Provider key | In the API source under **Connections**. |
| Relay request key | In the application you are connecting to the pool. |

**Reissue API key** is in the menu beside the key. After confirmation,
the old key stops working and the new key is copied to the clipboard.
Update clients where you entered it manually. When changing the port,
use the save button beside it: a running API restarts, and the client's
address needs updating.

**Wait for route recovery** can hold eligible text requests until a member
becomes available again. The client can cancel the wait.
This does not fix a wrong key, an invalid format, or a response already started.

**Paths for manual setup**

| Format | Path from the server root |
| --- | --- |
| Model catalog | `/v1/models` |
| Responses | `/v1/responses` |
| Chat Completions | `/v1/chat/completions` |
| Anthropic Messages | `/v1/messages` |
| Gemini | `/v1beta/models/{model}:generateContent` |
| Gemini Interactions, native API sources only | `/v1beta/interactions` |

Streaming Gemini uses `/v1beta/models/{model}:streamGenerateContent?alt=sse`.

Interactions requires an explicitly configured Interactions API source, for
example an endpoint ending in `/v1beta/interactions`; ordinary Gemini support
does not enable it. Send `stream: true` for SSE. Continuations must use the same
local key, model and source that created the interaction; unknown IDs are
rejected. Other client protocols, account sign-in, agents and background or
retrieval operations are not supplied by this native endpoint.
This format needs the server root without a trailing `/v1`.
Tools, images, and other features depend on the member and request format.
Unsupported conversions return an error.
Realtime and audio/video conversion are not provided.

### Applications

In **Access → Applications**, select ChatGPT, Claude Desktop or OpenCode.
Use the folder button or enter an absolute **Application path**, then **Save**.
Select the desktop `.app` on macOS or `.exe` on Windows, not a shortcut, CLI,
command with arguments, or configuration folder.

Clear and **Save** to restore automatic detection. Saving never launches, closes
or connects the application. If the file moves, update or clear the path.
Paths remain local and are excluded from pool presets.

Launch uses the saved path, then `OPENCODE_BIN` / `CLAUDE_DESKTOP_BIN`, then
automatic discovery. Invalid overrides must be corrected or removed; they never
fall back silently. OpenCode and Claude confirm a running desktop before success.
A timeout preserves settings: check the installation and retry.

#### ChatGPT

In **Access → Applications → ChatGPT**, **ChatGPT account** selects the sign-in
used by the application's interface. Choose an account, **Automatic**, or
**Without account**. Select **Switch** to apply the choice and restart the application.
The pool distributes generation requests through its API.

**Keep 1% reserved** is intended to preserve part of the selected account's
quota for direct ChatGPT launch.

Set context in **Pool → Model Rules**; see [model policies](#models-and-member-policies).
Relay refreshes its catalog without restarting the pool, but an already open
ChatGPT needs relaunch to read it. Speed changes do not need that catalog reload.
Automatic compaction uses about 90% of the selected window, subject to known
input/output limits; there is no separate compaction switch.

For **SIWC**, Relay compacts through a text summary on the same connection,
without the tool catalog. Relay restores its checkpoint as a summary on the
next turn; foreign encrypted checkpoints are not supported.

**ChatGPT background tasks** allows automatic activity summaries and task titles.
**Memory extraction** processes previous chats (`memories.extract_model`).
**Memory summaries** combines the extracted information into long-term memory
(`memories.consolidation_model`). Each row has its own model and permission switch.
Select a local pool model, or **Application default** to remove that stage's
override. Connect ChatGPT to the local pool first, then restart the application
manually after changing a model. The main chat model stays unchanged.

The switches immediately allow or block new, explicitly marked memory requests
through Relay; they do not enable Codex's memory feature, delete saved memory,
or stop work already in flight. Both stages are allowed by default and are
independent of activity summaries/titles. Blocked work appears in usage as a
local Relay restriction. Applying it requires the client to supply operation
metadata. Existing memory can still be read. Context compaction for the
main chat remains enabled. A smaller context window or missing capabilities can
prevent memory work from completing; pool membership alone does not establish
application-level compatibility.

**WebSocket for ChatGPT** controls ChatGPT → Relay; BPS and converted routes still
use HTTP upstream. Changes ask to restart ChatGPT. Request details show
**Client transport**.

#### Claude Desktop

In **Computer** mode, start the API and select **Pool → Connect → Claude Desktop**.
Relay validates the connection, closes Desktop, and writes its own profile while
keeping other profiles. It launches Desktop again only when automatic launch is
enabled. For explicit launch, use **Overview → Launch application → Claude Desktop**.
Managed device inference policy can override the local profile.

The list preserves pool names/order, with one entry per enabled Messages-routable
model and at most 200 models. Excess models are rejected before changing profiles.
Claude-compatible IDs route to the real model. Desktop's effort menu is capped
at the highest allowed level, but it may still show unsupported intermediate
choices: the current profile cannot supply an exact sparse list. For a model
with Low/High/Max, choose one of those levels. Relay rejects Medium and XHigh.
Disabled thinking stays disabled.

After updating Relay, reconnect Desktop and start a new chat. **MCP tools** and
**System tools** count tool definitions. Keep working tools, skills and history.
Relay enables on-demand MCP schemas; native Messages still
requires provider support for discovery.

Caching/prices follow the actual provider. Required features that cannot be
converted return an error. `Claude-3p/configLibrary` is the profile directory,
not an executable path. Do not share its Relay profile: it contains a pool key.

Use [Recovery](#6-recovery) to undo attachment. Newer manual selections and
edited profiles are preserved. If a recovery record was lost, reconnect can
reclaim an exactly matching Relay profile, but cannot recover an unknown previous
selection; restore then leaves that selection unset.

#### OpenCode

Connect through **Pool → Connect → OpenCode** or **Launch** on an individual
API source. Additional OpenCode settings under **Access** currently show
**In development**. This does not prevent connecting the client.
Use **Recovery** to restore its saved configuration.

### Accounts

This tab configures requests through accounts. Use **Connections** to sign in
and manage saved accounts.

**Reject model substitution** checks the model name in an account response.
Relay rejects a response reporting another model or an internal downgrade
ID such as `degrade2`. Dated versions of the same model are accepted.

## 5. Usage

This log contains requests through the local pool.
Choose a period, status, model or member, then open a row in **Sends** or
**Errors**. **24 hours** counts backwards from now. The selected page (up to
50 records) loads with its period totals; other pages load on navigation and
historical filter choices on opening a filter. **More filters** adds protocol,
transport, error category and request ID; its **Reset** clears those filters only.

Details show status, timing, cost, token/tool counters and copyable provider
errors. **Technical details** adds IDs, routing and transport measurements.
Requested, sent and provider-declared model names are distinct; the declared name
comes from the provider response. Missing measurements appear as a dash, including
WS handshake timing on reused sockets and token timing for buffered replies.
SSE can stream during tool lookup.

**Sends** counts every model call, retry and tool lookup; **Client requests**
and success rate count final outcomes once. Elapsed timings start at the client
request. `≈` means an API-price estimate; `—` means unknown.
Cache tokens already count in input, reasoning in output. Sends expire
with the log; request/response text and secrets are not stored.

**Cache reads/writes** measure cache tokens in this request. Reported zero
differs from an absent counter; reusing a prefix need not write it again.
**Cache retention (estimate)** uses the latest observed read/write for the same
chat, connection and upstream model (the selected request in other reports).
A miss does not renew it; expiry does not prove eviction. Unknown TTL stays
unknown; mixed TTLs refer to separate prefixes. GPT names and public API TTLs
do not establish retention for BPS or another compatible service.

**Technical details → Context** compares JSON size in bytes. Even a greeting
includes instructions, history and tools; BPS can add unobserved context, and
cache hits do not free context space. [BPS catalog lookup](../../project/BASIS_POINTS.md)
loads tool schemas on demand without dropping tools or constraints. Debug
comparisons contain no chat text and do not prove a cache-miss cause.

**Save diagnostics** exports local, redacted JSON; nothing is uploaded. TTFT
comparisons group the same route by effective reasoning, with counts, median,
p95 where sufficient, input size and cache fraction. Retries/multi-send discovery
are excluded. The comparison uses existing request history and marks incomplete
data. Reasoning settings are preserved; measurements come from recorded requests.

Optional `ZENITH_RELAY_HTTP_HEADERS_TIMEOUT_SECONDS=60` bounds native HTTP
connection, upload and header waiting across attempts. Allowed: 1–1800 seconds;
unset/zero/invalid disables it. It includes provider queueing, stops at headers
and excludes response bodies, BPS and native WS. `http_headers_timeout` (504,
Relay origin) may occur after execution starts: no automatic resend or account
disable. Check the client outcome before resending; increase/disable for slow sources.

### Image requests

Select Codex in **Connections → Accounts → Add to pool for images**, or its
**Images only** row in **Pool → Add member**. Sign-in alone does not add it.
Relay must run, not the Codex application; this route cannot serve text.

Use `/v1/images/generations` or `/v1/images/edits`, the local API key and an
image model from `/v1/models`. Native ChatGPT/Codex uses no extra text-model
call; Excel/BPS and subscription-sharing connections are unsupported. Provider
access and limits, not plan names, decide availability.

Edits accept JSON image URLs/data URLs or multipart files and masks; limit:
128 MiB. `stream: true` returns partial/completed base64 images; nonstreaming
`response_format: "url"` returns inline data URLs. Relay does not host/save files.
Details show completed images and reported text/image input tokens, already in
totals. Missing usage/prices stay unknown; subscription credit charges are not
available. Interruptions retain observed usage without automatic regeneration.

## 6. Recovery

This page restores installed client settings. It is available in **Computer**
mode and does not restore quota, deleted Relay accounts, or conversations.

1. Select **ChatGPT** or **OpenCode**.
2. Before making changes, enter a **Snapshot name** and select **Create snapshot**.
3. To return to it, select the snapshot, choose **Restore**, and review
   the confirmation. For ChatGPT, the saved configuration and sign-in replace
   the current profile.

ChatGPT supports several named snapshots. OpenCode stores one snapshot
until restoration; restoring consumes it.

For Claude Desktop, select its tab. Before connecting, enter a **Snapshot name**
and select **Create snapshot** to save one original recovery point. To return
to it, choose **Restore** and confirm. This works with the local API stopped.
Restoring an attached profile validates the files, closes Desktop and restores
the selection without relaunching it. Consuming a snapshot made before connecting
does not close Desktop or change its files. Unrelated profiles and newer manual
selections are preserved.

The automatic protected backup before switching ChatGPT is separate from
named snapshots. Automatic recovery does not overwrite a newer manual sign-in.
If Relay reports a conflict, check the current profile before trying again.

## 7. Settings

**Appearance** changes language and theme. **Application** shows the version
and working data folder. Language, theme, and operating mode save immediately.
Fields with a separate save button, such as the API port, need that button.

**Launch applications after connecting** is on by default. Turn it off to
configure a pool connection without opening ChatGPT, Claude Desktop, or OpenCode. Your choice
saves immediately and survives restarting Relay. An explicitly disabled setting
from an earlier version stays disabled. **Launch application** on Overview and
direct launch actions still open the selected application.

**Overview** opens the application without changing its connection, profile, keys
or catalog, or closing a running client—even with the API stopped or pool empty.
To attach Relay, use **Pool → Connect**. External profile changes do not block
ordinary launch. New manual ChatGPT context settings survive catalog updates,
reconnection and recovery.

**Updates.** Select **Check** beside the version or **Update available**.
Read the changes and select **Update**.
Checking a version does not install an update.
The Windows portable folder must be writable.

**Diagnostics.** In **Computer** mode, enable **Pool data → Debug mode**
to reveal **Diagnostics** with log folders. Errors and crashes are recorded
without debug mode; detailed operations require it.

**Reset local pool data** removes accounts, sources, settings, and statistics
after safely restoring client settings. If recovery is not possible,
the deletion does not proceed. For an ordinary connection error, try the
checks below first; a reset is not reauthentication.

## 8. Errors

### Check these first

| What happens | What to do |
| --- | --- |
| A saved account is missing from the member picker | The pool accepts SIWC with plan-use consent, BPS connections regardless of plan, and Codex for images only. Select the connection's route first. Accounts awaiting recovery and members already added are not listed. |
| **Connect** is unavailable | Start **Access → Local API** and check available pool members. |
| The API is running but has no models | Check members, provider access, and model permissions both in the member policy and in **Model Rules**. Starting the API does not add models. |
| The client cannot connect | Check the address, port, and Relay request key. `127.0.0.1` only works on the same computer. |
| A request is missing from **Usage** | Clear filters and check the mode. Direct account or source launch bypasses the pool. |
| Every member is waiting for quota | Wait for the provider's reset or add another available connection. **Refresh** only rereads quota. |
| A context change had no effect | Close and relaunch ChatGPT. Check that you are using a connection managed by Relay. |

If this did not help, search for the code or message in the reference below.

<!-- relay:error-reference -->

Open the affected card's error or the request in **Usage**. Read **Error origin**,
Relay category, provider code, and HTTP status together. For example,
`invalid_api_key` can refer to the pool key or an external provider key.
Fix the connection named in the details.

If a provider request ID is shown, copy it when contacting that provider.
It identifies the provider's request separately from Relay's local request ID.
Relay also includes a supported provider ID in the error message returned to
the client, so it remains visible in clients that display only the message.

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
| `invalid_api_key` · 401 from Relay | The client uses an invalid or old pool key. | Copy the current address and key from the API tab under **Access**, or reconnect the application. Rotating the key invalidates its predecessor. |
| `client_api_not_allowed` · 403 | The client key does not permit this API format. | Connect through the intended client profile and use an allowed endpoint with the Relay request key. |
| `invalid_host` · 400 | The local API received an unsuitable Host. | Use the address shown in **Access → Local API** without a proxy rewriting Host. For another device, enable **Allow local network access** and use the displayed LAN address. |
| `gateway_stopped`, `gateway_unavailable`, `runtime_unavailable` | The API is stopped, unreachable, or not ready. | Start the local API and check the address/port. Choose a free port if occupied, then reconnect the client. For LAN access, check the private network and firewall. |
| `codex_background_blocked_activity_summary`, `codex_background_blocked_task_title` | A ChatGPT background request was disabled. | Enable **Access → Applications → ChatGPT → ChatGPT background tasks** when summaries/titles are wanted. This is not a main-request failure or exhausted quota. |
| `codex_background_blocked_memory_extraction`, `codex_background_blocked_memory_consolidation` | Relay disabled this memory stage (403). | Enable **Memory extraction** or **Memory summaries** under **Access → Applications → ChatGPT**. No provider call or empty success is generated. |

### Quota, rate limits and provider permissions

| Code or message | Cause | Action |
| --- | --- | --- |
| `upstream_quota_exhausted`, `insufficient_quota`, `quota_exhausted` · 429 | The provider confirmed exhausted quota, credits, or a spending cap. | For accounts, wait for the quota window or use an available reset. For APIs, check wallet and key spending limits in the provider dashboard. Add a compatible reserve; refreshing statistics does not replenish quota. |
| `upstream_rate_limited`, `rate_limit_exceeded` · 429 | Provider request, concurrency or tokens-per-minute (TPM) limit; the message may say `Request too large`. | Respect the stated delay; reduce simultaneous tasks or request tokens. That wording alone does not prove context overflow or a terminal 413. Avoid immediate retry loops. |
| `upstream_turn_rate_limited`, `turn_rate_limit_exceeded` · 429 | The current conversation turn reached a rate limit. | Wait for the stated delay before continuing. This error alone does not require another login or account; Relay does not rotate the pool or pause the whole account. |
| `upstream_usage_not_included`, `usage_not_included` · 403 | The plan does not include this capability. | Choose an entitled model or connection. A normal quota reset does not grant a new capability. |
| `upstream_siwc_not_eligible`, `subscription_sharing_user_not_eligible` · 403 | SIWC is unavailable for this user, workspace or policy. | Check SIWC eligibility and plan-use permission. Relay does not rotate accounts for this refusal. |
| `upstream_unauthorized` · `invalid_api_key` from a provider | The provider rejected authentication. | Update an external API key in **Connections**, then refresh models. BPS with a saved refresh token renews access before the next request; the failed generation is not repeated automatically. Follow the account's recovery hint and sign in again when refresh is no longer possible. Changing the pool key does not fix provider authorization. |
| `upstream_account_disabled`, `account_deactivated` · 403 | An account, workspace, or project is disabled. | Check its provider dashboard and restore access through that provider. Use another permitted member while it is unavailable. |
| `upstream_account_verification_required`, `account_verification_required` | The provider requires account verification. | Complete it on the provider website, then refresh sign-in and account data. This is not a general Relay phone requirement; see [BPS sign-in requirements](#account-sign-in). |
| `upstream_forbidden`, `permission_denied` · 403 | The account or key lacks permission for the operation. | Check project, key, and model permissions. Reading a model list does not prove inference access. |
| `upstream_region_unsupported`, `unsupported_country_region_territory` | The provider does not serve the connection's region. | Check the provider's supported regions and permitted network configuration. Use a connection available in your region. |
| `upstream_edge_challenge`, `edge_security_challenge` | Edge security, including Cloudflare 1010, blocked API access. | Check the API address, proxy and provider access. Signing in again does not resolve this block. |
| `upstream_model_not_found` · provider `model_not_found` | This provider does not expose the requested model ID. | Refresh this source's models and verify key access. Remove an obsolete permission or select an actual available ID. |
| `upstream_route_degraded`, `route_degraded` | The ChatGPT response reports a different model or an internal downgrade id such as `degrade2`. | With Reject model substitution enabled under Access → Accounts, Relay rejects it in JSON, SSE (including response.created) and WebSocket. A pre-generation refusal can rotate; generated or delivered output is never replayed. Dated snapshots of the same model are allowed. The account pauses briefly. This checks reported identity, not quality. |
| `upstream_model_unavailable`, `model_not_available` · 403/503 | The model is temporarily unavailable, including Basis Points `Model access has changed`. | The route pauses this model on the member. BPS `basispoints_model_access_changed` and `model_not_allowed` pause its canonical model and aliases for 30 minutes. The failed request ends without another account or a transport change. This is not exhausted quota. |
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
| `response_continuation_unavailable`, `response_affinity_miss` | The response owner is unavailable or the required continuation context is missing/changed. | Restore the original account/API or resend complete replayable history. Gemini continuation tokens require the same local key, model and original contents; they cannot move to another owner. Start a new conversation if state is lost. A rotation mode change cannot restore context. |
| `upstream_previous_response_not_found`, `previous_response_not_found` | The provider no longer knows the previous response. | Resend full history without the stale response reference, or start a new conversation. Do not transfer just a response ID to another API. |
| `upstream_tool_call_mismatch`, `tool_call_not_found` | A tool result has no matching call, or a call has no result. | Relay retries once when the complete pair proves a missing or confused call identifier. It never removes results or guesses between parallel calls. If the error remains, update the client and resend the complete call/result pair, or start a new conversation if the missing history cannot be recovered. |
| `upstream_encrypted_content_invalid`, `invalid_encrypted_content` | The account rejected encrypted reasoning or compaction context. | For reasoning alone, Relay can remove the rejected ciphertext and its bound ID, retain visible summaries, and retry once before output. An encrypted compaction checkpoint blocks this repair: Relay keeps the complete history and returns the error. Return to the connection that created the item or start a new task with ordinary history. Do not manually edit encrypted blocks. |
| `tool_use_not_supported`, `chat_feature_not_supported` | This route cannot represent the requested tool or feature. | Function tools and their results are supported on compatible Chat Completions routes. Check the model's format capabilities; choose a matching native route or remove the specifically unsupported option. |
| `upstream_conflict`, `conflict` · 409 | State changed or another operation is in progress. | Wait for the earlier operation, refresh state, and retry once. Restore conversation history when the conflict concerns continuation. |
| `upstream_candidate_rejected`, `source_rejected` | A route rejected the request without a more specific category. | Read the provider code/message and check model, permissions, and format. A new independent request can use a compatible reserve. |

### Adapters and images

| Code or message | Cause | Action |
| --- | --- | --- |
| `adapter_binding_unsupported`, `source_protocol_invalid`, `source_pool_protocol_unsupported` | Relay could not build a compatible automatic route for the requested protocol and model. | Refresh the source catalog and verify its API address and model ID. Use another member when the provider does not expose a compatible native path or translatable format. |
| `adapter_invalid_request` | The adapter cannot translate the request. | Remove the field named in the message or choose a native-format source. |
| `adapter_parameter_unsupported` | A meaningful request parameter has no lossless mapping on the selected route. | Check `error.param` and use a compatible native route. For `text.format` in command approval, a BPS-only pool cannot provide native strict JSON: add an allowed compatible route, not an approval bypass. Encrypted history also requires its native route; do not delete it. |
| `adapter_compaction_unsupported` | This route cannot accept another provider's encrypted compaction checkpoint. | ChatGPT auto-compaction on any non-native model, including `/v1/responses/compact`, is handled by Relay. Use a native Responses route for a foreign checkpoint, or start a new task. |
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
| `http_headers_timeout` · 504 | The optional Relay native HTTP deadline expired before response headers; upstream execution may have started. | Check the client outcome before manually resending. Increase or unset `ZENITH_RELAY_HTTP_HEADERS_TIMEOUT_SECONDS` for slow sources. No automatic replay or account penalty occurs. |
| `stream_invalid` | Invalid stream event. | Report the request ID and `relay_stream_parser` diagnostics from request details: category, position and frame sizes, without content. For missing diagnostics, reproduce on the current build. |
| `stream_incomplete`, `upstream_websocket_closed`, `upstream_websocket` | The connection ended before completion. | Check network and proxy timeouts. Retry the unfinished step from the client; a partial answer is not a completed answer. |
| `stream_first_output_timeout`, `stream_idle_timeout`, `websocket_idle_timeout`, `stream_semantic_timeout` | Legacy/external timeout; current Relay does not time out active generation. | Update Relay, check provider/proxy limits, or cancel a stuck request in the client. |
| `stream_event_too_large` | A WebSocket frame exceeded Relay's 64 MiB frame limit. | Shorten that turn or use HTTP streaming. For a small request, verify the API and report its error ID. |
| `upstream_websocket_unsupported`, `websocket_not_supported` | The provider cannot use WebSocket. | Use HTTP streaming. If automatic fallback fails, disable **Access → Applications → ChatGPT → WebSocket for ChatGPT** and reconnect the client. |
| `upstream_websocket_slow_handshake`, `websocket_slow_handshake` | The optional initial WebSocket connection took too long; no generation was sent over it. | Relay can use HTTP before sending the request, without penalizing the account. If this repeats, check connection and proxy latency. |
| `upstream_websocket_handshake_rejected`, `websocket_handshake_rejected` | The upstream relay's internal WS connection returned 403, not an account-authentication failure. | Check endpoint/proxy permissions. No automatic replay or HTTP downgrade. |
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
| `basis_points_free_account_unsupported` | A historical local Free-plan restriction, removed from current sign-in, import, and pool execution. | Refresh account data or recreate an old import preview. Actual provider access and quota checks still apply. |
| `quota_unauthorized`, `models_unauthorized`, `models_invalid_access_token`, `subscription_unauthorized`, `subscription_access_token_invalid` | Monitoring authentication was rejected. | Refresh account sign-in and check its data again. |
| `models_forbidden` | The model-list request returned HTTP 403; this alone does not prove an account block. | Verify model access separately. Initial sign-in and refresh preserve this error without blocking an otherwise healthy account. |
| `pool_models_unavailable` | The pool has no available ChatGPT-compatible models. | Check enabled members and model rules, refresh model access, then reconnect. |
| `quota_forbidden`, `subscription_forbidden` | The provider forbids these data reads. | Check account/workspace permissions and regional availability. Verify model access separately. |
| `account_profile_rate_limited`, `quota_rate_limited`, `models_rate_limited`, `subscription_rate_limited` | Checks are too frequent. | Wait for the pause; do not repeatedly press refresh. |
| `quota_timeout`, `models_timeout` | A quota or model-list check timed out while connecting or reading the response. | Check internet and the configured account or common proxy, then refresh. A timeout does not prove that the account is blocked; saved values remain until a successful check. |
| `quota_transport`, `models_transport`, `subscription_transport`, `quota_probe_failed` | A monitoring request failed to complete because of a connection error. | Check internet and the configured proxy, then refresh. The last recorded quota or model list may remain visible until a successful check. |
| `quota_upstream`, `models_upstream`, `subscription_upstream`, `quota_http_status`, `models_http_status`, `subscription_http_status` | The monitoring service failed or returned an unexpected status. | Inspect the status. Retry transient failures later; correct access/request problems for 4xx. This does not prove zero quota. |
| `quota_invalid_response`, `quota_invalid_percentage`, `models_invalid_response`, `subscription_invalid_response` | Monitoring data is invalid. | Check for proxy/login pages replacing API responses. Update Relay and report a persistent code without the raw response body. |
| `quota_response_too_large`, `models_response_too_large`, `subscription_response_too_large` | Monitoring response size exceeded the limit. | Verify API and proxy configuration. A repeat on the correct connection needs provider/Relay compatibility investigation. |
| `models_invalid_account_id`, `subscription_account_id_invalid`, `subscription_account_missing` | The check cannot find the account. | Select the correct workspace and repeat sign-in/import. |
| `models_invalid_client_version`, `models_invalid_endpoint`, `models_client_init`, `subscription_configuration`, `quota_policy_invalid` | Monitoring parameters are unsupported. | Update Relay and check saved connection settings; report the code if already correct. |
| `quota_proxy_unavailable`, `models_proxy_unavailable`, `account_runtime_proxy_invalid` | The account proxy is invalid or unavailable. | Correct its URL/credentials and assignment in **Connections**, then check again. |
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
| `use_source_import` | This record is an API source. | Add it under **Connections → Sources**. |
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
| `reasoning_levels_invalid`, `model_service_tier_unsupported`, `model_context_unsupported`, `model_reasoning_recovery_failed` | The selected reasoning mode, speed, or context size is absent from the model reference, a pool connection cannot carry it, or model settings could not be recovered. | Refresh models and choose a supported mode, Standard speed, or the default context. Reopen model rules after a recovery failure. |
| `configuration_preset_invalid`, `configuration_reference_missing` | A preset is invalid or references missing connections. | Export it again with a compatible version; map members to existing connections in preview. Presets do not transfer secrets. |
| `configuration_store_failed`, `configuration_runtime_failed`, `runtime_reload_failed`, `gateway_sync_failed`, `source_runtime_invalid` | Configuration could not be saved or applied to the runtime. | Refresh and inspect actual pool membership/settings, fix the named connection, and apply again. Use diagnostics if it persists; a closed dialog is not proof of success. |
| `wake_task_not_found`, `wake_account_missing` | A background task/account is absent from this environment. | Open the task in its owning environment and select an existing account. |
| `wake_model_unavailable`, `wake_invalid_request`, `wake_invalid_configuration`, `wake_invalid_endpoint` | Background request configuration/model is unsuitable. | Choose an entitled model and supported task parameters. |
| `wake_invalid_access_token`, `wake_invalid_provider_account_id`, `wake_unauthorized`, `wake_credentials_unavailable` | Background work has no usable account sign-in. | Restore account sign-in and checks in **Connections**. Make sure the rule is enabled and wait for its next automatic run when the condition is met. |
| `wake_forbidden`, `wake_tags_unsupported`, `wake_confirmation_unsupported` | The selected background-task option is unsupported. | Check account rights and use supported account selection and automatic execution. |
| `wake_rate_limited` | The provider limited background requests. | Wait and reduce task frequency/concurrency. |
| `wake_proxy_unavailable`, `wake_timeout`, `wake_transport`, `wake_upstream`, `wake_http_status` | Network or provider failure interrupted background work. | Check proxy and HTTP status, resolve the cause, then retry. |
| `wake_request_too_large`, `wake_response_too_large`, `wake_invalid_response` | Background request/response size or format is invalid. | Use a short request and compatible model. Check the connection and update Relay if persistent. |

### Profiles and local data

| Code or message | Cause | Action |
| --- | --- | --- |
| `profile_restore_blocked` | The profile changed during the operation or automatic rollback would replace a newer sign-in. | Let ChatGPT finish writing and retry the explicit connection. Relay saves a new manual sign-in as the next restore point; if the error repeats, inspect the profile under **Recovery**. |
| `recovery_required`, `cleanup_incomplete` | Recovery or cleanup from an earlier operation is unfinished. | Open **Recovery** and operation diagnostics. Preserve backups and finish recovery before repeating. A full data reset is not the first remedy. |
| `account_rotation_recovery_required` | Account token rotation cannot safely resume; the request was not sent. | Sign in to that account again. Preserve its data instead of resetting Relay. |
| `account_credential_store_unavailable` | Protected account credentials could not be read before dispatch. | Check access to the operating system credential store, then retry. Do not delete account data to work around a temporary storage failure. |
| `snapshot_missing`, `snapshot_io` | A required snapshot is missing or unreadable. | Check environment, backup existence, and folder access. Reconfigure the client if the backup is lost; do not replace it with an empty file. |
| `profile_attach_unavailable`, `system_key_missing`, `diagnostic_key_unavailable` | Managed client attachment could not complete. | Refresh the local API state and reconnect the application. Use the current Relay request key after rotation. |
| `proxy_invalid`, `proxy_unavailable`, `proxy_route_ambiguous` | Proxy configuration is invalid or both bypass and proxy use are selected. | Choose one route: shared proxy, individual proxy, or bypass. Correct URL and credentials. |
| `proxy_assignment_invalid`, `proxy_assignment_duplicate` | Bulk proxy assignment does not match the selected accounts. | Supply one URL per selected account and remove duplicate assignments. |
| `proxy_check_timeout` | The connection check exceeded 12 seconds. | Check the address and port, or retry later. The saved proxy is retained. |
| `proxy_check_connection_failed`, `proxy_check_auth_failed` | The selected proxy could not be reached or rejected authentication. | Correct its address, port, username and password. No direct connection is attempted. |
| `proxy_check_rejected`, `proxy_check_invalid_response` | The check service rejected the request or returned no valid exit IP. | Retry later and check whether this proxy permits HTTPS traffic to Cloudflare. This does not establish model availability. |
| `proxy_check_unavailable` | The saved proxy could not be read or was removed during a check. | Refresh the list and restore access to protected storage before retrying. |
| `secret_store_unavailable`, `credential_store_unavailable`, `vault_failed` | Protected storage is unavailable. | Restore access for the current OS user and preserve protected storage. Do not replace an existing vault key. |
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
