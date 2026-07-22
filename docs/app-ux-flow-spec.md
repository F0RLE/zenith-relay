# Zenith Relay UI Design Specification

This is the canonical UI specification for the future Zenith Relay desktop
application. It replaces the previous expanded UX draft.

Interactive schematic:

- [ui-schematic.html](ui-schematic.html)

The schematic defines visual hierarchy and placement. Runtime, security, and
storage behavior remain owned by the architecture documents.

## Product Shape

Zenith Relay has three user-facing runtime scenarios:

```text
Account pool
Ready API
Self-hosted
```

The modes share one shell and the same object names:

```text
connections
pool members
pool keys
endpoint
usage
profiles
```

Mode differences:

| Public label | Internal id | Runtime | Secrets | Endpoint |
| --- | --- | --- | --- | --- |
| Account pool | `local` | Desktop process | Device secret store | `http://127.0.0.1:<port>/v1` |
| Ready API | `zenith` | Selected compatible cloud service | Device secret store | Selected service `/v1` |
| Self-hosted | `remote` | User-managed server | Selected server | User server `/v1` |

The legacy internal ids `local`, `remote`, and `zenith` remain implementation
details. They are not public product names and do not force Ready API to use
Zenith. Inside Ready API, Zenith is the recommended preset alongside a custom
compatible API.

Public UI never shows Zenith internal provider selection, private economy,
owned inventory, or selling-pool routing.

## Design Direction

The application is a desktop operations tool:

```text
quiet
dense
precise
fast to scan
safe around secrets
```

It is not:

```text
a marketing page
a crypto dashboard
a gaming launcher
a terminal imitation
an oversized card dashboard
```

Rules:

- no gradients, glow, decorative illustrations, or background effects;
- no cards inside cards;
- tables and flat sections for operational data;
- one page-level primary action;
- icons for familiar commands, always with tooltips;
- destructive actions stay in menus or danger zones;
- secrets stay masked unless temporarily revealed;
- status always uses icon, text, and color together.

## Information Architecture

Top-level navigation contains exactly seven entries:

```text
Overview
Connections
Pool
Gateway
Usage
Profiles
Settings
```

Nested views:

```text
Connections -> Sources | Accounts | Automations | Remote Server
Pool        -> Members | Keys | Model Rules
Gateway     -> Endpoint | Client Setup | Diagnostics
Usage       -> Requests | Models | Pool Members | Errors
Profiles    -> Profiles | Backups | Repair
Settings    -> General | Appearance | Storage | Updates | Security | Recovery
```

Do not create separate sidebar entries for Sources, Accounts, Servers, Keys,
Models, Instances, Backups, or Diagnostics.

Mode visibility:

| Navigation | Ready API | Account pool | Self-hosted |
| --- | --- | --- | --- |
| Overview | yes | yes | yes |
| Connections | yes | yes | yes |
| Pool | hidden | yes | yes |
| Gateway | yes | yes | yes |
| Usage | yes | yes | yes |
| Profiles | yes | yes | yes |
| Settings | yes | yes | yes |

In Ready API mode, Gateway means endpoint and client configuration. It does
not show local start/stop controls.

## Window And Shell

Target dimensions:

```text
default window       1160 x 760
minimum window        840 x 560
title bar               36px
expanded sidebar       216px
collapsed sidebar       56px
page header              56px
control height           32px
compact control          28px
```

Desktop shell:

```text
+--------------------------------------------------------------------------------+
| Zenith Relay                                                _  square  X       |
+----------------------+---------------------------------------------------------+
| Mode selector        | Page title                              Primary action   |
|----------------------|---------------------------------------------------------|
| Overview             | Context/status strip when needed                        |
| Connections          |---------------------------------------------------------|
| Pool                 |                                                         |
| Gateway              |                    Page content                         |
| Usage                |                                                         |
| Profiles             |                                                         |
|                      |                                                         |
|----------------------|                                                         |
| Settings             |                                                         |
| Help  Update  v1.x   |                                                         |
+----------------------+---------------------------------------------------------+
```

### Title Bar

Contains:

- product name;
- drag region;
- optional update indicator;
- native minimize, maximize, and close controls.

It does not contain balance, endpoint, key input, mode controls, or runtime
buttons.

### Mode Selector

The mode selector sits at the top of the sidebar under the product name.

```text
[ icon ] Account pool                       chevron
```

Click opens a menu:

```text
Account pool
Self-hosted
Ready API
```

Switching mode asks for confirmation only when it would:

- stop an active gateway;
- discard unsaved changes;
- leave an active import/deploy wizard.

### Page Header

```text
left:  title + short state subtitle
right: optional secondary action + one primary action
```

Examples:

| Page | Primary action |
| --- | --- |
| Overview | mode-specific next action |
| Connections | Add connection |
| Pool | Action for the active tab |
| Gateway | Start or Stop |
| Usage | Refresh |
| Profiles | Add profile |
| Settings | Save, only when dirty |

The primary action follows the active nested view:

| View | Primary action |
| --- | --- |
| Connections / Automations | Add automation |
| Pool / Members | Add member |
| Pool / Keys | Create key |
| Pool / Model Rules | Add rule |
| Gateway / Endpoint | Start, Stop, or Restart |
| Gateway / Client Setup | Apply to profile |
| Gateway / Diagnostics | Run full check |
| Profiles / Profiles | Launch selected profile |
| Profiles / Backups | Restore selected backup |
| Profiles / Repair | Run profile check |

The Russian UI uses `Участники` for pool members. Runtime and architecture code
may continue to call the same entity a `candidate`; that internal term is not a
user-facing label.

### Command Placement

```text
page primary command    header, far right
page secondary command  left of primary or overflow menu
search and filters      above table, left
bulk actions            above table after row selection
routine row actions     far-right icon/menu column
destructive row action  overflow menu below separator
form save/cancel        sticky footer, right aligned
modal actions           Cancel then Primary, bottom-right
```

## First Run And Quick Setup

First run opens a focused setup flow before the operational shell. The user
must not see tables, routing terminology, logs, or gateway settings before they
have selected what they want to use.

Before the five setup steps, first launch shows one short product-introduction
screen. It answers only:

```text
what Zenith Relay is
what it connects
where it can run
```

The screen contains the product name, one short paragraph, the animated flow
`Accounts -> Zenith Relay -> Applications`, and three compact facts. It does
not mention internal providers, pricing, selling infrastructure, or routing
implementation.

Actions:

| Action | Result |
| --- | --- |
| Get started / Приступить | Open step 1 of quick setup |
| Skip / Пропустить | Open the empty application shell without setup |

The introduction appears only on first launch or after an explicit onboarding
reset. Reopening Quick setup from Settings starts directly at step 1.

The setup has five short steps:

```text
1. Usage mode
2. Connection
3. Check
4. Client
5. Ready
```

It is not a marketing landing page. Every step asks one question, contains one
primary action, and keeps technical details collapsed.

### Language And Localization

The app reads the operating-system language on first launch. A compact language
select floats at the top-right of setup and later remains under Settings ->
General.

Localization rules:

- one screen never mixes interface languages;
- Russian mode uses Russian labels, descriptions, validation, dates, numbers,
  plural forms, status text, and notifications;
- English mode uses the corresponding English strings everywhere;
- product and protocol names such as `Zenith`, `Codex`, `OpenCode`, `OpenAI`,
  `OAuth`, model ids, URLs, and file paths remain unchanged;
- user-facing labels use `Пул аккаунтов`, `Свой сервер`, `Готовый API`, `Адрес
  API`, and `Совместимый API`, not internal mode ids or unexplained
  English terms;
- every UI string comes from translation resources; components do not contain
  hardcoded prose;
- missing translations fall back to English and are reported during build or
  development, not silently mixed into production screens;
- layouts reserve room for longer translated labels and support wrapping;
- right-to-left layout is deferred until an RTL language is added.

### Setup Shell

```text
+ Native title bar --------------------------------------------------------+
+ Progress -------------------------------------------- Language ----------+
| Mode -------- Connection -------- Check -------- Client -------- Ready    |
+ Body --------------------------------------------------------------------+
| one question                                                             |
| one short explanation                                                    |
| selectable options or the current form                                   |
| compact animated flow preview                                            |
+ Footer ------------------------------------------------------------------+
| Back                                                Continue / Finish    |
+-------------------------------------------------------------------------+
```

`Skip setup` asks for confirmation and opens an empty Overview with one action:
`Start quick setup`. The setup can be reopened from Overview, Help, and
Settings -> General.

### Step 1: Usage Mode

Question in Russian:

```text
Где будет работать Zenith Relay?
```

Choices:

| UI label | One-line explanation | Flow preview |
| --- | --- | --- |
| Пул аккаунтов | Sign in to personal accounts and expose a local endpoint | Accounts -> Relay -> application |
| Готовый API | Use Zenith or another compatible hosted API | Ready API -> profile -> application |
| Свой сервер | Run the same personal setup around the clock | Accounts -> server -> devices |

The selected choice uses a check icon, border, and soft accent background.
`Пул аккаунтов` is selected by default. `Continue` is disabled only when
no choice is selected.

### Step 2: Connection

The entire step changes for the selected mode.

Account pool:

```text
Sign in to a Codex account through OAuth
Import an existing local session
Optionally add another supported account type later
```

Self-hosted:

```text
[ Connect existing server | Deploy new server ]
server address + management token
or deployment method + generated configuration
```

Ready API:

```text
[ Zenith, Recommended | OpenAI | OpenRouter | Custom API ]
masked key
name, API address, and protocol for non-Zenith sources
```

The local path is account-first and never requires an external API key. Zenith
is not a top-level mode and is not promoted in the account or server paths. The
user chooses one starting method; additional accounts and optional API sources
are added later from Connections. No priority, weight, model rule, or timeout
is shown in quick setup.

Secrets remain masked. Advanced fields are behind `Additional settings`.

### Step 3: Check

The check runs automatically after entering the step. It shows a vertical list
of stages instead of a spinner with no explanation:

```text
Checking credentials
Checking endpoint
Reading models and capabilities
Reading balance or quota
```

Each stage has `pending`, `running`, `success`, or `error` state. On success the
screen shows only:

- connection name;
- endpoint;
- available model count;
- balance, quota, or pool-member count;
- `Continue`.

On failure the failed stage stays visible with one concrete recovery action:
`Change key`, `Sign in again`, `Change server address`, or `Retry`.

### Step 4: Client

Question:

```text
Что подключить к выбранному API?
```

Choices are separate applications:

```text
Codex
OpenCode
Другой совместимый клиент
Настроить позже
```

Selecting Codex or OpenCode shows the detected profile and exact action that
will be performed. Their files, authentication, history, backups, and launch
commands remain separate.

Before applying:

```text
detect profile -> show path -> create/reuse backup -> apply -> verify
```

The user sees `A backup will be created` before confirming. Manual config is
shown only for `Другой совместимый клиент`.

### Step 5: Ready

The final screen contains:

- success state;
- selected mode;
- active endpoint with Copy button;
- attached client, when selected;
- `Open application` as the primary action;
- `Launch Codex` or `Launch OpenCode` as an optional secondary action.

It does not repeat setup instructions or show advanced settings.

### Motion

Motion explains state changes rather than decorating the page:

- step content moves `8px` and fades over `160-200ms`;
- the progress line advances between steps;
- the selected mode flow animates one request dot from source to endpoint to
  client;
- connection-check stages activate one by one;
- success uses one short check animation and does not loop;
- layout does not move when button text changes;
- `prefers-reduced-motion` removes movement and keeps immediate state changes.

### Returning Users

Completed setup opens Overview directly. Quick setup reappears only when:

- no valid connection exists;
- the user chooses `Start quick setup`;
- onboarding state was explicitly reset.

An unavailable connection shows a recovery strip on Overview. It does not force
the full wizard when the user can repair the existing connection in place.

## Overview

Purpose: answer three questions immediately:

```text
Is the selected mode working?
What endpoint is active?
What action is needed next?
```

Layout:

```text
+ Header -------------------------------------------------------------------+
| Overview                                              [Mode primary action]|
+ Status -------------------------------------------------------------------+
| state | endpoint | balance/quota | warning                                |
+ Metrics ------------------------------------------------------------------+
| Requests today | Healthy | Models | Errors                               |
+ Runtime ------------------------------------------------------------------+
| Endpoint and client state             | Health and capacity               |
| [Copy endpoint] [Open Gateway]         | [Open Connections/Pool]           |
+ Activity -----------------------------------------------------------------+
| Last five requests, warnings, and updates                    [View Usage]  |
+---------------------------------------------------------------------------+
```

Mode primary action:

| Mode/state | Action |
| --- | --- |
| Ready API disconnected | Connect API |
| Ready API connected | Open connection |
| Account pool stopped | Start endpoint |
| Account pool running | Stop endpoint |
| Self-hosted disconnected | Connect server |
| Self-hosted offline | Retry |
| Self-hosted online | Open endpoint |

Metrics are a flat horizontal band. They are not separate floating cards.

Empty Overview shows one next action, not setup documentation.

## Connections

Purpose: manage credentials and upstream access.

Layout:

```text
+ Header -------------------------------------------------------------------+
| Connections                                      [Import] [Add connection]|
+ Tabs ---------------------------------------------------------------------+
| Sources | Accounts | Automations | Remote Server                         |
+ Toolbar ------------------------------------------------------------------+
| Search...  Status v  Protocol v                              [Refresh]     |
+ Table --------------------------------------------------------------------+
| select | status | name | host/identity | models/quota | checked | menu    |
+---------------------------------------------------------------------------+
```

Visible tabs:

| Mode | Tabs |
| --- | --- |
| Ready API | API connection |
| Account pool | Sources, Accounts, Automations |
| Self-hosted | Sources, Accounts, Automations, Remote Server |

### Add Connection

`Add connection` opens a short menu:

```text
Sign in to account
Import existing session
Compatible API source
OpenRouter preset
Custom source
Connect server
Deploy server
```

Only actions valid for the current mode are shown.

In Ready API mode, the menu offers `Zenith API` with a `Recommended` badge,
`OpenAI`, `OpenRouter`, and `Custom API`. Account import is hidden. In Account
pool and Self-hosted server modes, account actions appear first; optional
API-source actions follow them without highlighting Zenith. `Import` opens
account/session import directly.

### Sources Table

Columns:

```text
Status
Name
Host
Protocol
Models
Pool and role
Actions
```

Inline actions:

- Test;
- Edit;
- include/exclude from the pool without changing the saved connection.

Menu actions:

- Enable/Disable;
- Delete.

The pool column names the runtime effect instead of exposing sentinel priority
numbers: `API first`, `Stabilizer`, or `Last resort`. Creating a source from
Connections leaves it outside the pool until the user explicitly includes it.

### Accounts Table

Columns:

```text
Health
Label
Plan
Subscription end date
Quota and reset
Models
Last used
Menu
```

Every account row shows the normalized subscription end date when the provider
reports one. Missing metadata is displayed as unavailable and is never replaced
with access-token expiry.

Inline action:

- Refresh quota.

Menu actions:

- Open details;
- Test model;
- Reauthenticate;
- Drain/Resume;
- Enable/Disable;
- Delete.

Account rows also expose `Create automation` when the account has normalized
quota windows and supports a harmless wake request.

### Automations

Visible only in Account pool and Self-hosted modes. Purpose: start a new quota
countdown after a selected account window has fully recovered, before client
demand arrives.

The page uses one table, not task cards:

```text
Enabled | Name | Accounts | Quota windows | Model | Trigger | Last result | Menu
```

Header actions:

```text
History
Add automation
```

The default task type is `Start quota countdown`. Its editor contains:

```text
name
accounts: all eligible | selected accounts | account tags
quota windows: 5-hour | weekly | every supported window
model: lightest supported | explicit model
trigger: when selected window becomes fully available
fallback schedule: none | daily | weekly | interval
execution: automatic | require confirmation
random delay: 0-15 minutes
```

The normal editor does not expose a free-form prompt. Relay generates a fixed
minimal request, limits output, disables tools, and discards response content.

Task detail shows:

- selected accounts and window mapping;
- current eligibility and next expected check;
- last run totals: successful, skipped, unconfirmed, failed;
- per-account technical status without response text;
- `Test selected accounts`, `Pause`, `Edit`, and `Delete`.

Trigger rules shown in UI:

- run only for enabled, healthy OAuth accounts with a supported model;
- skip when a normal client request already started the new quota cycle;
- run once per account and quota-window cycle;
- verify that a future reset countdown appeared after the wake request;
- do not keep retrying when the provider does not confirm the new cycle;
- local tasks pause while Relay is closed; server tasks continue independently.

### Remote Server

MVP supports one active server record.

Shows:

```text
status
host
version
endpoint
capabilities
last check
```

Actions:

```text
Test connection
Refresh capabilities
Update server
Disconnect
```

`Connect existing` and `Deploy new` appear only when there is no active server.

### Connection Detail

Connection editing uses a full page:

```text
Back | Name + status                         Test  Disable  Save
----------------------------------------------------------------
Overview | Models/Quota | Pool policy | Usage | Settings
----------------------------------------------------------------
content, max width 920px
----------------------------------------------------------------
Danger zone                                            Delete
```

Do not edit authentication, model lists, quota, or protocol in a narrow drawer.

## Pool

Visible only in Account pool and Self-hosted modes.

Purpose: control which connected accounts and sources may serve requests.

Tabs:

```text
Members
Keys
Model Rules
```

### Members

Layout:

```text
+ Header --------------------------------------------------- [Add member]   +
+ Toolbar -------- [Display order v] [Refresh quotas] [Refresh settings]  +
+ Summary -----------------------------------------------------------------+
| In rotation | Waiting for quota | Unavailable | Disabled                      |
+ Table -------------------------------------------------------------------+
| enabled | type | name | health | quota reserve | last used | menu       |
+ Selected editor ----------------------------------------------------------+
| tie-break priority | traffic share | drain | allowed models | exclusions |
+---------------------------------------------------------------------------+
```

User-editable policy:

```text
enabled
draining
manual tie-break priority
weight
allowed/excluded models
```

The pool exposes automatic, highest-quota, subscription-expiry, and
subscription-plan strategies. Automatic order is:

```text
hard filters
mandatory previous-response binding
API source role tier
OAuth preference inside the stabilizer tier
greatest current known quota after the ChatGPT-interface reserve
fewest active requests when quota is equal
committed dispatch balance when quota and active load are equal
stable id
```

Highest-quota uses the same hard filters, source roles, OAuth preference, and
quota comparison, then a stable id. It deliberately ignores active load and
dispatch history, so the account with the greatest quota stays selected.

Subscription-expiry and subscription-plan strategies apply the same hard gates,
then follow their configured group order. An active OAuth account remains usable
by multiple chats; local last-used timestamps and cooldown timers never reorder it.

The scheduler makes this choice for every request. `API first` sources run before
OAuth accounts, `Stabilizer` sources remain in the same tier, and `Last resort`
sources wait behind other eligible members. Traffic share affects API sources,
not quota-based OAuth selection. Sorting the
visible member list never changes runtime order. Rows use `In rotation`,
`Waiting for quota`, `Unavailable`, and `Disabled`; a manually chosen OAuth
account is labelled `ChatGPT interface`, keeps a 1% reserve, and is not a pinned
pool route.

The backend is the sole owner of that four-value operational status. It describes
connection health independently of pool membership; `inPool` is a separate fact.
Connections and Pool render `operationalStatus` from the runtime snapshot and
never derive a second status from quota, health, proxy, or cooldown fields.

Request details show the redacted routing reason and the counters used for that
decision. This explains quota, parallel-load, response ownership, and tie-break choices
without exposing prompt/response content or credentials.

Creating or importing a connection does not add it to the pool. The empty
Members view asks the user to choose existing connections, and only confirmed
selections become eligible runtime candidates. The table defaults to an
availability/quota approximation of runtime order and may be sorted by
effective quota or name for inspection.

Member rows may show an `API equivalent` derived from recorded input and
output tokens for model ids present in Relay's versioned official OpenAI price
catalog. This value is informational only: it is not subscription spend,
provider billing, or a routing input. Tokens without a catalog price or without
an input/output split remain explicitly unpriced instead of being silently
estimated.

`Refresh quotas` updates only enabled OAuth accounts currently in the pool and
uses a bounded batch. Refresh settings use fixed safe presets: background
interval `120..=3600` seconds and request timeout `10..=20` seconds. Manual
refresh remains available regardless of the background interval.

Buttons:

```text
Add member         page primary action
Refresh quotas      secondary, enabled when the pool has an OAuth account
Refresh settings    icon action beside refresh
Preview selection  secondary
Save changes       appears only when dirty
Reset              ghost, appears only when dirty
```

Member deletion removes it from the pool but does not delete the underlying
source/account.

### Keys

Table:

```text
Status | Label | Masked key | Scope | Models | Requests | Last used | Menu
```

Page action:

- Create key.

Row actions:

- Copy;
- Edit policy;
- Enable/Disable;
- Rotate;
- Delete.

Rotate and Delete require confirmation. Key values are shown only once after
creation or rotation, then remain masked.

### Model Rules

Contains:

- model aliases;
- globally hidden models;
- per-member exclusions;
- local key allowed/excluded models.

Model availability preview explains which member still supports a model.

## Gateway

Gateway changes meaning by mode but keeps the same structure.

Tabs:

```text
Endpoint
Client Setup
Diagnostics
```

### Endpoint

Layout:

```text
+ Header ----------------------------------------------------- [Start/Stop]  +
+ Endpoint -----------------------------------------------------------------+
| URL                                        [Copy]                        |
| active key                                 [Copy] [Manage keys]          |
+ Runtime ------------------------------------------------------------------+
| Status | Models | Last request | First response | Active requests        |
+ Settings -----------------------------------------------------------------+
| Port | Host | Bind scope                         [Apply and restart]       |
+ Advanced -----------------------------------------------------------------+
| Common account proxy | Timeouts | Debug | Management API                  |
+---------------------------------------------------------------------------+
```

The common proxy control accepts a new HTTP(S) proxy value or clears the saved
value; it never reveals a stored address. A separate `Require a proxy for OAuth
accounts` toggle disables direct account egress without deleting affected
accounts. `Connections -> Accounts` shows only `Direct`, `Common`, or `Account
proxy` plus availability; direct accounts blocked by the policy are visibly
unavailable. Each account can replace or clear its override, and the bulk
dialog assigns one proxy line per selected account while reporting unused
lines.

Mode behavior:

| Mode | Header action | Editable runtime settings |
| --- | --- | --- |
| Ready API | none | none |
| Account pool | Start/Stop | port, host, bind scope |
| Self-hosted | Retry/Restart when supported | capability-dependent |

Key rotation is owned by `Pool -> Keys`, not the endpoint header.

### Client Setup

Client tabs:

```text
Codex
OpenCode
Other
```

Shows one generated configuration at a time.

Actions:

```text
Copy config
Apply to profile
Restore previous
Open profile
```

Apply always creates or reuses a backup. Restore requires preview and
confirmation.

### Diagnostics

Contains:

- non-stream request test;
- stream test;
- endpoint health;
- port check;
- redacted logs;
- support bundle export.

Advanced destructive action `Kill occupied port` requires process preview and
confirmation.

## Usage

Purpose: diagnostics and local accounting, not Zenith billing.

Layout:

```text
+ Header ----------------------------------------------- [Refresh] [Export] +
+ Metrics -----------------------------------------------------------------+
| Requests | Success | Total tokens | Latency                              |
+ Filters ------------------------------------------------------------------+
| Range | Model | Pool member | Key | Status | Request ID                 |
+ Table --------------------------------------------------------------------+
| time | status | model | pool member | latency | total tokens | request id |
+ Models / Pool Members aggregate -----------------------------------------+
| requests | success | input tokens | output tokens | total tokens | latency |
+ Request dialog -----------------------------------------------------------+
| timing | input/output/total usage | selected member | redacted error       |
+---------------------------------------------------------------------------+
```

Request details open in a focused dialog and keep the underlying filters and
table state intact after closing.

Rules:

- `This computer` and `My server` render the same Usage components, columns,
  request details, aggregates, charts, filters, and empty/error states; only
  the backend command/data source changes;
- Remote mode reads server-owned rows and aggregates. It never merges local
  telemetry or displays a cached local zero as the server result;
- when the server is disconnected, keep the last remote page visibly stale
  and show reconnect state instead of silently replacing it with local data;
- no prompt or response body by default;
- no monetary estimate is shown without authoritative provider/model pricing;
- an OpenAI `API equivalent` is allowed only from the versioned official price
  catalog and recorded split token counts, with unpriced tokens disclosed; it
  is never labeled as actual cost, subscription spend, or Zenith billing;
- request rows/details expose latency, TTFT, generation duration, visible
  output tokens/second, input/cache-read/cache-write/reasoning/output/total
  tokens, and API equivalent when those values are known;
- Remote API equivalent uses server-owned price overrides and remains present
  after the launcher or server restarts;
- account/source aggregates use the current safe label and keep a stable
  redacted hint when that object no longer exists;
- raw upstream errors are redacted and collapsed;
- selected account/source identity is masked where needed;
- Clear Logs lives in the page overflow menu and requires confirmation.

## Profiles

Purpose: manage Codex/OpenCode configurations and launches.

Layout:

```text
+ Header ----------------------------------------------------- [Add profile] +
+ Table --------------------------------------------------------------------+
| status | name | client | active endpoint | backup | last launch | menu    |
+ Selected detail ----------------------------------------------------------+
| binding | config path | backup state | history state | launch controls   |
+---------------------------------------------------------------------------+
```

Primary action depends on state:

| State | Primary action |
| --- | --- |
| no profile | Add profile |
| not attached | Attach current endpoint |
| attached | Launch |
| broken managed config | Restore previous |

Other actions:

- Stop known process;
- Open folder;
- Repair history;
- Edit profile;
- Delete profile.

Restore and Repair always show a preview before writing.

## Settings

Settings contains application defaults, not routine runtime operations.

Sections:

```text
General
Appearance
Storage
Updates
Security
Recovery
```

Layout:

```text
section navigation | form content, max width 760px
                   |----------------------------------------------
                   | dirty footer: Discard  Save
                   |----------------------------------------------
                   | danger zone: Reset local data
```

General:

- language;
- startup behavior;
- default mode.

Appearance:

- system/light/dark theme;
- compact table density.

Storage:

- data path;
- log retention;
- open data/backups folders;
- redacted export/import.

Updates:

- current version;
- update channel;
- check/install update.

Security:

- secret store state;
- temporary reveal timeout;
- LAN and insecure Self-hosted warnings.

Recovery:

- quarantined files;
- restore backup;
- reset onboarding;
- reset local pool data.

Start/Stop, connection tests, quotas, keys, and profile attach actions do not
belong in Settings.

## Wizards

All wizards use:

```text
header: title + step count + close
body: current step only
footer: Back | status | Cancel | Continue/Save
```

Wizard width is `680-760px`. Import preview may use up to `960px`.

### Add Source

```text
1. Type
2. URL, key, protocol
3. Models and test
4. Review and save
```

The source may be saved disabled after a failed test. It is not enabled in the
pool silently.

### Import Account

```text
1. OAuth/login/file/token source
2. Parse/login
3. Preview identities and duplicates
4. Optional quota refresh
5. Confirm selected accounts
```

Failed rows remain visible with a reason. Partial import is allowed.

### Connect My Server

```text
1. URL and management token
2. Test health and capabilities
3. Confirm server identity
4. Save connection
```

### Deploy My Server

```text
1. Deployment method
2. Generate configuration
3. Show command/package
4. Test deployed server
5. Save as active Self-hosted runtime
```

MVP methods:

- single binary;
- Docker Compose;
- manual instructions.

## Component System

### Theme

Default follows the operating system. Light is the screenshot/reference theme.

Light:

```text
app background       #f5f7f8
surface              #ffffff
surface subtle       #fafbfc
border               #d9dee3
border strong        #bdc6ce
text primary         #182026
text secondary       #46515a
text muted           #707b84
accent               #0e9f77
accent hover         #0b8564
accent soft          #e5f6f0
focus                #2fc69e
```

Dark:

```text
app background       #111315
surface              #191c1f
surface subtle       #1f2327
border               #30363b
border strong        #464e55
text primary         #f2f4f5
text secondary       #c9ced2
text muted           #929aa1
accent               #2ab38e
accent hover         #38c7a0
accent soft          #153a31
focus                #4bd5af
```

Status colors:

```text
ready       green
warning     amber
error       red
info        blue
disabled    neutral gray
```

Do not tint whole pages or tables with the accent.

### Typography

```text
font stack       Inter, Segoe UI, system-ui, sans-serif
page title       20px / 28px / 600
section title    14px / 20px / 600
body             13px / 20px / 400
table            13px / 18px / 400
label            12px / 16px / 500
button           13px / 18px / 500
caption          12px / 16px / 400
```

Letter spacing is `0`. Font size never scales with viewport width.

### Spacing And Shape

```text
spacing scale     4, 8, 12, 16, 20, 24, 32
page padding      20px, 14px compact
section gap       16px
table row         44-48px
input/button      32px
icon button       32x32px
radius            6px controls, 8px modal max
drawer            440px, 520px request detail
```

### Buttons

Variants:

```text
Primary      accent fill
Secondary    surface + border
Ghost        no border
Danger       red, only for final destructive action
Icon         square, tooltip required
Link         text command
Segmented    mutually exclusive mode/options
```

Rules:

- one page-level Primary button;
- loading buttons keep their width;
- Copy flashes `Copied` without resizing;
- disabled commands expose a reason;
- Delete, Rotate, Restore, Repair, Reveal, and Reset require confirmation or a
  temporary reveal flow;
- use Lucide icons already installed in the application.

### Forms

```text
input height         32px
textarea minimum     96px
field width          320-520px
form row gap         14px
```

Rules:

- validate after blur or submit;
- URLs, keys, tokens, model ids, and paths use monospace value text;
- secret reveal is temporary;
- toggles are only for immediate binary settings;
- three or more options use select, menu, or segmented control.

### Tables

Rules:

- sticky header when scrolling;
- status first, object name second, menu last;
- numeric and quota values align consistently;
- row hover does not reveal the only access to actions;
- action menu is keyboard reachable;
- empty state shows one Primary and at most one Secondary action;
- bulk actions appear only after selection.

### Quota

```text
6px progress bar
exact percentage
exact reset time
secondary window below when present
```

States:

```text
Available
Low
Exhausted
Unknown
Unsupported
Refresh failed
```

Quota state never relies on color alone.

### Modals And Drawers

```text
confirmation modal    420px
short form            520px
wizard                680-760px
import preview        up to 960px
drawer                440px
request drawer        520px
```

Dirty modal does not close on backdrop click. `Esc` asks to discard when
needed. Focus returns to the opening control after close.

### Toasts

Bottom-right, maximum three visible.

```text
success     3 seconds
info        5 seconds
error       persistent until dismissed or recovered
```

Toasts never include secrets or full upstream responses.

## States

Every screen and command implements:

```text
loading
empty
ready
partial/degraded
error
offline
permission denied
dirty/unsaved
success feedback
```

Rules:

- stale data remains visible with a warning during refresh;
- a failed background refresh does not replace valid data with zeroes;
- disabled controls show why they are disabled;
- retry action stays near the failed object;
- errors identify stage: validation, auth, connection, quota, execution,
  storage, or profile write;
- raw errors are available only in redacted diagnostics.

## Responsive Behavior

Wide `>=1180px`:

- expanded sidebar;
- table plus optional drawer.

Standard `1024-1179px`:

- expanded sidebar unless user collapsed it;
- metrics may wrap into two rows;
- drawer remains `440px`.

Compact `840-1023px`:

- sidebar becomes icon rail;
- detail replaces list;
- request drawer becomes full content width;
- status strip hides secondary text;
- tables use horizontal scroll or compact row layout;
- buttons keep stable size and labels wrap only where allowed.

The desktop window does not resize below `840x560`. Mobile/web requires a
separate product layout.

## Accessibility

Required for first implementation:

- full keyboard navigation;
- visible focus ring;
- tooltip and accessible label for icon-only buttons;
- focus trap in modal/drawer;
- focus restored after close;
- `Esc` closes only when no unsaved data would be lost;
- status does not rely on color;
- WCAG AA contrast;
- reduced-motion support;
- destructive confirmation focuses Cancel first;
- copy, reveal, rotate, and delete announce results to screen readers.

Reserved shortcuts:

```text
Ctrl+R    refresh active view
Ctrl+L    focus endpoint on Gateway
Esc       close menu/drawer/modal safely
```

Command palette is deferred.

## Implementation Mapping

```text
Overview      -> state
Connections   -> sources/accounts/automations/self_host/deploy
Pool          -> routing/models/keys
Gateway       -> gateway/profile/diagnostics
Usage         -> telemetry
Profiles      -> instances/profile/repair
Settings      -> settings/storage/update/recovery
```

Frontend never:

- reads secret storage directly;
- writes Codex/OpenCode files directly;
- infers runtime state from files;
- calculates Zenith billing or public prices;
- chooses Zenith backend providers;
- exposes private account-pool details.

## MVP Screen Order

1. Shell, mode selector, Overview.
2. Account-first connection and local endpoint lifecycle.
3. Connections Accounts view, account sign-in, and import flow.
4. Gateway Endpoint and Client Setup.
5. Pool Members and Keys.
6. Optional API Sources view and Add Source wizard.
7. Quota wake Automations and execution history.
8. Usage table and request drawer.
9. Profiles attach/restore/repair.
10. Self-hosted connect/deploy/server view.
11. Settings, recovery, dark theme, and compact layout.

## Acceptance Checklist

- Exactly seven top-level navigation entries exist.
- First run presents three modes, not four connection products.
- Every page has no more than one page-level primary action.
- Sources, Accounts, Remote Server, Keys, and Models are nested views, not
  duplicate sidebar pages.
- Ready API supports Zenith and custom compatible APIs without making Zenith a
  separate top-level scenario.
- Account pool works with OAuth accounts only, can start the endpoint, copy
  key/URL, and attach a client without requiring an external API source.
- Pool policy exposes tie-break priority/weight/model rules without exposing internal
  Zenith routing.
- Self-hosted can connect or deploy and remains clearly user-managed.
- Wake automation runs at most once per account/window cycle and never stores
  generated response content.
- Every command has loading, failure, disabled reason, and success feedback.
- Secrets remain masked and absent from logs, toasts, and support exports.
- `1160x760` and `840x560` screenshots have no overlaps or clipped commands.
- Light and dark themes preserve hierarchy and contrast.
- Keyboard-only use reaches all actions and confirmations.
- Playwright verifies every top-level screen in all three modes.
- Windows, macOS, and Linux use the same screens and feature set; only native
  paths, window controls, secret-store wording, and package-specific actions may
  differ.
