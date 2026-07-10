# Local Pool UI Notes

Date: 2026-07-09

These notes capture local pool UI requirements and product structure. They are
not public marketing copy and must not expose Zenith internal routing, pricing
rules, server capacity internals, or third-party internals.

## UI Areas Covered

- account overview;
- model providers;
- wake tasks;
- multiple instances;
- session management;
- API service overview;
- client API keys;
- account pool;
- models and capabilities;
- statistics and request logs.

## Useful Product Patterns

Account overview:

- quota bars on account cards are immediately useful;
- account plan badge and reset time should stay visible;
- quick actions near each account reduce navigation;
- current local API service status on the account screen helps orientation.

Model providers:

- provider records need test, switch, edit, open, refresh, and delete actions;
- base URL, auth state, balance/quota, and provider health are all needed;
- masked keys and copy buttons are expected.

Multiple instances:

- table layout is clearer than account cards for process-like data;
- status, account, speed/profile, PID, and actions scan well in one row;
- this style should guide Zenith's `Gateway` and `Sources` screens.

API service overview:

- top metric strip is valuable: requests, tokens, cost estimate, latency,
  success/error counts;
- service settings need base URL, client host, local key, port, proxy, and
  timeout/retry controls;
- protocol compatibility cards are useful for users configuring clients.

Client keys:

- named local keys are a core feature;
- each key needs enabled state, usage, rotate, disable, delete, and policy
  expansion;
- per-key model policy should be visible but collapsed by default.

Pool:

- account/source health and request totals belong beside routing settings;
- routing should include strategy, session affinity, retry attempts, retry wait,
  and cooldown behavior;
- account member management and model blocks should be separated from routing
  controls.

Models and capabilities:

- model list, image capability, access scope, aliases, and hidden model rules are
  all needed;
- capability toggles should not sit beside long free-text model rule fields on
  small screens.

Stats and logs:

- request logs need filters by model, account/source, API key, type, status,
  mode, and error;
- logs should show latency, tokens, estimated local cost, request ID, and HTTP
  status;
- daily/weekly/monthly aggregates by account/model/key are useful for local pool
  debugging.

Runtime state:

- each account/source row needs active/disabled/unavailable/refreshing/error
  state;
- model support should show discovered models, hidden models, excluded models,
  and cooldown reason separately;
- recent success/failure buckets make pool health easier to read than only
  lifetime counters;
- when `/v1/models` hides a model, the UI should explain whether it is hidden by
  local key policy, source/account model rule, cooldown, quota, or no healthy
  candidate;
- local key auth errors should distinguish missing key from invalid/disabled
  key.

Session management:

- backup/restore and session visibility are useful, but lower priority than
  gateway/accounts/sources;
- session list should not be part of first local pool MVP unless profile
  attach/restore needs it.

Wake tasks:

- Cockpit's useful baseline is account multi-select, model/reasoning preset,
  daily/weekly/interval/quota-reset triggers, immediate/confirmation execution,
  next-run preview, test action, and per-account history;
- Zenith keeps this under `Connections -> Automations`, after manual quota
  refresh and model test exist;
- default behavior is quota-window driven, not a fixed two-minute scan;
- skip a wake when normal client traffic already started the new cycle;
- use one deduplicated minimal request and verify the countdown afterwards;
- store technical outcomes only. Do not retain generated response text as
  Cockpit currently does;
- local tasks pause with the app; server tasks continue without the desktop.

## Visual Problems To Avoid

- too many rounded cards with similar weight make scanning slow;
- account overview mixes API service, OAuth accounts, quotas, and actions in one
  dense surface;
- provider cards hide comparison data that would be clearer in a table;
- action icon rows lack obvious hierarchy and can place destructive actions near
  routine actions;
- right-side panels leave empty space when only one object is selected;
- dark blue palette plus glow effects make dense text less readable;
- advanced settings appear too early for normal first-run users;
- some controls reveal implementation terms that should be hidden in Zenith's
  public UI.

## Zenith Adaptation

Use these first screens:

1. **Connect**: choose `Zenith API`, `Local Pool on this computer`,
   `Personal Pool Server`, or `Self-Host`.
2. **Servers**: personal server deploy/connect/update and self-host
   capabilities. Details open in full-width view or drawer.
3. **Sources**: table of provider/API sources. Details open in full-width view
   or drawer.
4. **Accounts**: table/card hybrid with quota bars, plan, reset, health, tags,
   and actions.
5. **Gateway**: service status, base URL, port, local key, protocol cards, test,
   attach/restore.
6. **Pool**: routing strategy, candidates, priority/weight, model blocks,
   cooldowns.
7. **Keys**: generated local/server API keys with scope and model policy.
8. **Usage**: request logs and aggregate stats.
9. **Settings**: storage, language, theme, backups, advanced timeouts.

Use table-first layout for:

- sources;
- keys;
- instances/processes;
- request logs;
- pool candidates.

Use compact cards for:

- account summary;
- gateway metric strip;
- warnings;
- setup presets.

Hide advanced controls until needed:

- timeout presets;
- wake tasks;
- model alias textareas;
- LAN access;
- proxy;
- debug logs;
- session visibility repair.

First implementation should make the main path obvious:

```text
add source/account -> test -> start gateway -> copy local endpoint/key -> attach Codex/OpenCode -> see usage
```

## Backend Notes From UI

The UI implies these backend contracts:

- `GET state`: returns service status, current endpoint, sources, accounts,
  keys, pool health, metrics, and warnings;
- `POST source/test`: validates base URL/key/protocol/model;
- `POST gateway/start` and `POST gateway/stop`;
- `POST profile/attach` and `POST profile/restore`;
- `POST key/create`, `POST key/rotate`, `PATCH key`, `DELETE key`;
- `PATCH routing`: strategy, affinity, retry, cooldown, priority/weight;
- `GET logs`: paginated, filterable request events;
- `GET stats`: totals plus account/model/key aggregates;
- `PATCH model-rules`: aliases, hidden models, per-account/source blocks.
- `GET/POST/PATCH/DELETE wake-tasks`: task definitions and state;
- `POST wake-tasks/{id}/test`: bounded preview execution;
- `GET wake-history`: paginated per-account technical outcomes without response
  bodies.

Do not expose Zenith internal production routing, pricing rules, or server
capacity internals through these public local UI contracts.

## Management And Diagnostics UI

Use Tauri commands for normal management. HTTP management, if added later,
belongs under advanced settings and should show:

- localhost-only status;
- management key generated/rotated separately from local API keys;
- failed-auth lockout state;
- remote/LAN management disabled by default;
- last config/runtime update summary with redacted values.

Diagnostics screen should include:

- test non-stream request;
- test stream request;
- model visibility explanation;
- support bundle export with redaction preview;
- log viewer with incremental loading;
- request ID lookup.

Do not show raw request bodies, raw headers, raw tokens, or full account
identities by default.

## State And Failure UX

The main local pool screen should render one backend state snapshot:

- gateway running/stopped;
- endpoint and LAN endpoint;
- visible model count;
- candidate/source/account count;
- last error or warning;
- default profile attach status;
- account/source health rows;
- recent usage totals.

Test failures should use the same compact shape everywhere:

```text
title
stage
cause
suggestion
status
model
details
```

UI rules:

- stage decides icon/color/action, not raw error text;
- profile attach status should show `attached`, `config`, and `auth` separately;
- port cleanup result should say how many processes were stopped and refresh
  gateway status after cleanup;
- timeout presets stay in `Settings > Advanced`;
- request log filters belong above the table, not inside each row;
- batch delete/import jobs need progress, pause/resume, retry failed, and clear
  actions.

## Second Visual Pass

Extra screens inspected:

- expanded sidebar;
- platform dashboard;
- layout customization modal;
- app settings: general, network, data management;
- add Codex account modal;
- add model provider modal.

Additional useful patterns:

- expanded sidebar with icon plus text is much clearer than icon-only sidebar;
- platform dashboard works as a high-level health board: summary cards on top,
  then current/recommended account panels per platform;
- settings rows with label, short hint, and control aligned on the right are
  easy to scan;
- network settings belong under an advanced settings area, not in the main
  gateway screen;
- data management screen confirms that backups, export, and import need their
  own settings section;
- add-provider form has the right core fields: provider name, base URL,
  protocol, site, key-page URL, key name, and API key;
- protocol selection should be explicit at create time because it affects
  routing and supported endpoints.

Additional problems to avoid:

- icon-only sidebar hides product structure until the user hovers or expands it;
- dashboard has too many platform tiles for a product focused on Zenith API and
  local pool;
- platform layout customization is powerful but too early for first release;
- add-account modal puts several import modes in one dense surface;
- sensitive auth helper data appears too openly in import flow;
- add-provider modal shows a large provider catalog before the user has entered
  a source, which slows a normal custom-provider setup;
- fixed modal footer can cover lower form fields when the modal scrolls;
- provider presets are too numerous and visually equal, so the recommended path
  is not obvious.

Zenith decisions:

- use expanded sidebar by default on desktop, collapsible to icon-only;
- keep `Home` focused on Zenith API status and Local Gateway status, not a
  multi-platform dashboard;
- put `Settings` in full-width rows, not cards;
- keep `Network`, `Backups`, and `Advanced` behind settings tabs;
- make add-account/import a wizard:
  `Choose type -> Paste/import/test -> Preview -> Save`;
- mask sensitive helper data by default and require explicit reveal;
- make add-source wizard prioritize:
  `Zenith API preset`, `OpenRouter preset`, `OpenAI-compatible`, `Custom`;
- move big provider catalogs to searchable presets, not first screen;
- keep create-source footer visible but never covering fields;
- show validation and test result before enabling a source in the pool.

## API Service And Pool Deep Pass

Extra analysis focused on the local API service and multi-account pool.

### API Service Overview

Required concepts:

- service status and selected API service;
- top metrics: requests, image requests, tokens, local cost estimate, average
  latency, success rate;
- base URL and client host;
- generated local API key with reveal/copy controls;
- service port;
- optional proxy URL;
- timeout/retry advanced button;
- protocol cards for OpenAI chat, Responses, Anthropic messages, Gemini, Ollama,
  and model catalog routes.

Problem: too many concepts are in one screen. For Zenith, split:

- `Gateway`: status, base URL, port, local key, protocol cards, test, attach;
- `Keys`: named local keys and scope/policy;
- `Usage`: metric strip and logs;
- `Settings > Advanced`: proxy, timeouts, LAN scope, debug logs.

### Client Keys

Required row shape:

- key label;
- masked key;
- enabled badge;
- created/used timestamp;
- request count;
- copy, disable, rotate, delete actions;
- collapsed `model policy`.

Expanded policy has:

- model prefix;
- allowed models textarea;
- excluded models textarea.

Zenith additions needed:

- source scope;
- account scope;
- allowed wire APIs;
- optional spend/request cap later.

Keep policy collapsed by default. Add/edit/delete/rotate must use confirmation
where destructive or irreversible.

### Pool Screen

Required pool layout:

- left: account cards with email/label, plan badge, request count, token count,
  success/error counts, local cost, estimated share, health, image capability;
- top actions: model block, member management;
- right: routing parameters.

Routing controls:

- strategy dropdown;
- session affinity toggle;
- affinity TTL;
- retry credential count;
- retry wait;
- disable cooldown.

Strategy values:

- auto recommended;
- single account;
- high quota first;
- low quota first;
- high plan first;
- low plan first;
- expiry soon first;
- custom.

Zenith layout:

- use one table for pool candidates by default;
- show health/quota/last error inline;
- open full-width detail when one account/source selected;
- keep routing settings in a separate `Routing` section, not mixed with member
  cards;
- move `disable cooldown` to advanced/debug only.

### Member Management Modal

Required behavior:

- modal title: add to API service;
- collection participant list;
- restrict free accounts toggle;
- search accounts;
- quota filter;
- team/group filter;
- paginated account list;
- checkbox per account;
- selected account highlighted;
- save collection button.

Zenith adaptation:

```text
Pool -> Add candidates
  tabs: Accounts | Sources
  filters: provider, health, quota, tag, model support
  list: checkbox rows
  footer: selected count + save
```

Do not show provider/internal implementation names beyond user-created source
names. Free-account restriction is provider-specific policy and should not be a
global public first-screen control.

### Models And Capabilities

Required:

- available model list;
- image capability toggle;
- access scope toggle;
- aliases textarea;
- hidden models textarea.

Zenith adaptation:

- `Models` belongs under `Sources` or `Pool` details, not global first screen;
- discovered models should be per source;
- merged visible catalog should be read-only summary;
- manual aliases/hidden models belong in advanced model rules.

### Stats And Logs

Required:

- top metric strip reused;
- tabs: request log, by account, by model, by key;
- time window: day/week/month;
- filters: model, account, API key, type, status, mode, error;
- log rows show model, status, API key, timestamp, type, account, latency,
  tokens, local cost, request id, HTTP status.

Zenith adaptation:

- keep this as a dense table;
- default filters: model, source/account, local key, status, error;
- show `copy request id` and `open detail`;
- redact account identity in screenshots/support export unless user reveals it;
- never label local estimated cost as Zenith billing.

## Backend/UI Product Decisions From Deep Pass

- First screen after choosing Local Pool should be:

```text
Sources/accounts present? -> show Pool health + Gateway start
Empty? -> add source/account wizard
```

- `Gateway` must show copy-ready client config, but not overwhelm with every
  protocol until user picks target client.
- `Pool` must show candidate truth: enabled, unhealthy, cooling down,
  quota-limited, unsupported model, draining.
- `Keys` must be separate because key scope/policy is its own object.
- `Usage` must be separate because request logs need table density.
- Advanced route strategy should not be in setup wizard; default `Auto` first.
- Session affinity should be on by default for Codex-like clients, but labeled
  as conversation consistency/prompt-cache stability, not internal routing.

## Instances And Client Profiles

Instances need a dense process-style table, not account cards.

Columns:

```text
name
profile path
bound account/source/local key
launch mode
speed/profile preset
PID/running
initialized
last launched
actions
```

Actions:

```text
start
stop
open window
edit
bind/unbind
repair history
attach local gateway
restore previous login/API key
delete
```

Rules:

- show delete only as inline/destructive action with confirm;
- for one selected instance, open full-width detail;
- show "needs first launch" state when profile is not initialized;
- show credential-kind change warning in plain user terms:
  "history may need refresh after switching between login and API key";
- do not describe local routing internals.

## Profile Repair UX

Profile tools should be boring and predictable.

Tabs or sections:

```text
Attach
Restore
History repair
Backups
```

Attach/restore status should show:

- profile path;
- attached/not attached;
- current endpoint host;
- local key label;
- last backup time;
- restore availability;
- last error/action.

History repair UX:

1. choose instances;
2. choose target provider only when auto-detection is wrong;
3. run preview;
4. show counts and affected running instances;
5. confirm repair;
6. show backup path only in advanced/details.

Preview summary is mandatory. Repair button stays disabled until preview
completes.

## Client Auth Adapter UX

Client adapters are optional target integrations.

Screen shape:

```text
Client
Status
Detected path
Target account/local key
Last sync
Action
```

Supported states:

```text
not detected
detected
synced
needs restart
sync failed
unsupported on this OS
```

Adapter details should show:

- which client file will be changed;
- whether a legacy path was migrated/synced;
- verification result;
- restart/reload action if needed.

Do not expose token contents. Reveal only masked account identity and local key
label.

## Source And Account Wizards

Provider/source setup should be wizard-first:

```text
Choose source type -> Enter connection -> Test model -> Save -> Add to pool
```

Account import should be wizard-first:

```text
Choose import type -> Paste/select files -> Preview -> Confirm selected -> Refresh quota
```

Both flows need a review screen before enabling the source/account in the pool.
This prevents half-valid entries from immediately affecting local gateway
selection.

Detailed screen-by-screen behavior, click paths, mode boundaries, design
requirements, button inventory, responsive rules, and MVP acceptance checks are tracked in
[`app-ux-flow-spec.md`](./app-ux-flow-spec.md).
