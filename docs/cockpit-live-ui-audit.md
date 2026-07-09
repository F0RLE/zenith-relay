# Cockpit Live UI Audit

Date: 2026-07-09

Reference app: Cockpit Tools, inspected through Windows UI automation. Notes are
for behavior and product structure only. Do not copy UI assets, text, colors, or
implementation.

## Screens Inspected

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

## Useful Patterns

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

Session management:

- backup/restore and session visibility are useful, but lower priority than
  gateway/accounts/sources;
- session list should not be part of first local pool MVP unless profile
  attach/restore needs it.

Wake tasks:

- automatic wake/probe tasks are powerful but advanced;
- first Zenith version should ship manual health refresh/test first;
- scheduled wake tasks can come later under `Advanced`.

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

1. **Connect**: choose `Zenith API`, `Local Pool`, or hidden operator mode.
2. **Sources**: table of provider/API sources. Details open in full-width view
   or drawer.
3. **Accounts**: table/card hybrid with quota bars, plan, reset, health, tags,
   and actions.
4. **Gateway**: service status, base URL, port, local key, protocol cards, test,
   attach/restore.
5. **Pool**: routing strategy, candidates, priority/weight, model blocks,
   cooldowns.
6. **Keys**: generated local API keys with scope and model policy.
7. **Usage**: request logs and aggregate stats.
8. **Settings**: storage, language, theme, backups, advanced timeouts.

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

Do not expose Zenith internal provider routing, upstream price selection, or
server-side owned account pool behavior through these public local UI contracts.

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

Zenith decisions from this pass:

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
