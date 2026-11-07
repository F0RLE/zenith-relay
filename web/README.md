# Relay Hosted web

[Russian translation](README.ru.md)

An independent React/TypeScript/Vite frontend in the Relay repository. The customer
dashboard and owner panel operate only in the Hosted context. Hosted API integration,
authentication and server operations remain future H13/H14/H15 work. Development uses
synthetic data held in window memory. The production build includes the screens but
excludes fixtures and simulated authenticated sessions.

## Local development

From `web/`, using the same Bun installation as Desktop:

```powershell
bun install --frozen-lockfile
bun run dev
```

Open `http://127.0.0.1:1441/`. The landing page's Get started action opens the demo
dashboard directly, without a welcome dialog or sessionStorage markers. The secondary
Models & pricing action opens `/app/models` without an intermediate Overview page.

## Dashboards and synthetic data

Customer dashboard: `http://127.0.0.1:1441/app/overview`. Sections under `/app/`:
`overview`, `usage`, `models`, `status`, `keys`, `billing`.
Status follows Models and includes a summary, search and model availability history
for the past 24 hours or 7 days. Hover, focus or touch reveals interval metrics;
arrow keys select an interval and Escape closes the tooltip. Times use UTC. Sources
and routing are private. Until monitoring is connected, metrics remain unknown and
history stays neutral. The catalog does not prove availability, and personal request
history does not substitute for service status.
The legacy `/preview.html?page=…` remains a development entry into the same interface,
not a separate dashboard. Direct links, reload and Back/Forward preserve the section.

Owner panel: `http://127.0.0.1:1441/admin/overview`. Sections under `/admin/`:
`overview`, `connections`, `pools`, `models`, `usage`, `notifications`.
`src/features/app/routes.ts` defines the routes. The owner panel is a separate surface;
the customer dashboard has no role switch or impersonation action.

The preview supports adding synthetic connections, admitting them to the pool
separately, reordering, parking, pausing and restoring them outside the pool.
Connections, Pool and Overview share these changes until reload. Search matches
names, sources and models; changing the filter or tab clears the selection. Trash
preserves connections, and restoring one does not automatically rejoin the pool.
Member order is relative to the entire pool, including members hidden by filters.
The presence of a connection does not establish quota, subscription or source health.

Account cards show subscription, route, quotas, known dates, API-equivalent usage,
estimated weekly remainder and cost recovery. Quota refresh stays disabled until
Hosted API integration. Missing dates are hidden; unknown quota is not zero, and an
expired window does not become a fresh full quota. Economics are API equivalents,
not customer charges or provider costs. Cost recovery requires a purchase price.
Totals include accounts in the current saved inventory, not just search results:
known usage is summed, shared quotas count once per ledger, and cost recovery uses
paired usage and purchase-price data. Partial values are marked as approximate;
fully unknown values use a dash. The `$` control toggles both summary economics and
card metrics.

`accountPresentation.ts` reuses pure Desktop functions from `accountEconomics.ts`,
`poolFormatting.ts` and `quotaFormatting.ts`, without the store or IPC. Telemetry for
two demo accounts is defined separately in `preview/ownerAccounts.ts`, based on
synthetic Desktop tests and anchored to the snapshot time. Local accounts, screenshot
data and secrets are not sources for these values.

Owner Overview separately shows posted customer API charges, requests, errors and
current pool membership. Charges include only `posted` amounts for the selected
period/model, excluding top-ups, holds, unsettled requests and provider costs.
Unknown total charges remain unknown. This validates a synthetic snapshot, not live
statistics across customers.

Pool admission requires separate confirmation. Selection survives filtering and
navigation to API connection creation. A saved provider joins the selection but
enters the pool only after explicit confirmation. Bulk admission neither resumes
paused connections nor restores trashed ones. Only Hosted inventory is used; there
is no Desktop IPC or transfer of local accounts.

Usage preserves its filters. Customer and owner Overview periods cover the last
24 hours, 7 days or 30 days before `snapshot.asOf`, not calendar days from midnight.
All 24/7/30 intervals are complete. Exact times remain in tooltips and the accessible
table, without interval labels below charts. The development snapshot is fixed and
does not simulate live refresh before API integration.
`src/features/overview/timeline.ts` defines the periods; the owner also has an all-time
option. In Usage, Today starts at midnight UTC, while other windows roll relative
to the snapshot (`src/features/usage/period.ts`).

## Catalog and connections

Unknown routes show Page not found. Customer and owner dashboards share the catalog,
without a local tariff editor. The tier/context menu shows published combinations;
each row supports copying the model ID.

`src/features/models/model.ts` owns the presentation contract: `rates` are microUSD
per million tokens, `comparison.officialRates` are comparable official rates, and
`comparison.savingsPercent` contains server-calculated percentages for input, output,
cache reads and cache writes. The frontend does not calculate discounts, select
official prices by model name or reconstruct current prices from request history.
The server qualifies model, tier, context-band and cache-rule comparability. Missing
percentages have no badge; missing prices display a dash, not zero. Invalid percentages
and official prices at or below the offered rate do not become advertised discounts.
The default order follows the server catalog, without invented popularity rankings.

Pool model rules group companies as Desktop Relay does. `catalogProvider` identifies
the group, with `publisher` metadata also used in the preview, rather than model name
or connection type. Prices and enabled state are editable inline. The compact arrow
before a company name collapses or expands its rows only in the current owner panel;
it neither disables models nor hides them from the customer catalog. Groups start
expanded, and collapsing them preserves prices and order.

The leading drag handle moves an entire company or a model within its company.
Touch, arrow keys, Home/End and Escape cancellation are supported. Shared
`modelDisplayOrder` retains complete blocks, including disabled models; new models
join their company. One projection supplies this order, state and pricing to both
owner and customer catalogs.

In the development-only preview, catalog edits synchronize between open tabs through
`BroadcastChannel`, in memory only. A new tab receives edits from an existing tab;
closing all tabs loses them. Without `BroadcastChannel`, edits stay in the current
tab. This is not server persistence or tariff publication: revision-checked writes,
authorization and public-sale projections remain H07/H13–H15 roadmap work.
Reordering does not change pool rotation, route availability or charge calculation.

`pricingVariants` projects server-authorized customer offers rather than implementing
a second pricing engine. Standard/Flex/Priority/Ultrafast identifiers, context bands
`base`/`above100k`/`above128k`/`above200k`/`above272k`/`above512k` and range formatting reuse
pure Desktop/core types and helpers (`api/types/pricing.ts`,
`components/sourcePriceEditorModel.ts`). A price threshold is not inferred from a
model's maximum context. Each variant carries its own rates and comparisons; missing
values are not inherited from another tier. Five-minute and one-hour cache-write
rates remain separate components. A provider's rate is not the retail price, and a
price record does not prove route availability. Core-overlay resolution, availability,
retail markup and official-price comparability remain server responsibilities.

`src/preview/models.ts` supplies synthetic development/preview data, not actual OpenAI,
Anthropic or Google offers. These fixtures are excluded from production. The Hosted
API adapter and server tariff publication are not connected yet.

API cards in Connections and Pool follow Desktop's structure: name and address,
provider balance/spend, model count and four actions. `SourceSummary`, `SourceStats`
and pure currency/protocol helpers are reused from Desktop/core. Unknown amounts use
a dash; customer charges do not substitute for provider costs. The model count opens
a list with permitted core protocol bindings. Adding or editing addresses, protocols
and manual model lists affects only the demo store. A changed address clears old
observations, and new models receive no invented capabilities. Keys are not accepted,
provider requests are not sent, and refresh stays disabled until Hosted API integration.
Hosted does not launch local applications; its final card action is pause/resume.

Removing preview screens does not remove future Hosted accounting, authorization or
audit requirements. Publication, refunds, sign-in and server management do not simulate
successful server operations.

The web app has no local mode, application launching, local profiles, real account
import, Tauri IPC or Desktop runtime access. Desktop state is not transferred. Full
demo keys appear once and never enter URLs or storage; create, reissue and revoke
affect preview data only.

Hosted theme and language controls are in the header, with an SVG flag beside the
theme control as on the landing page. These are optional local preferences; English
is the default. Neither dashboard has a Settings section. `/app/settings`,
`/admin/settings` and `/preview.html?page=settings` show Page not found. On phones,
navigation uses a modal drawer with focus handling, Escape and focus return to the
menu button.

In the dashboard, the language control expands into a strip replacing the flag,
notification bell and theme control. The center is highlighted; wheel scrolling and
arrow keys smoothly move flags without committing a choice. Click or Enter confirms;
Escape restores the previous choice. Opening and closing do not shift the header.
Rapid repeated actions interrupt animation without waiting; reduced motion uses only
a short fade.

Data and tariffs are synthetic; there are no real keys, authentication or network API
calls. Demo top-ups change only the current window's state. Amount input, shortcuts and
payment-method selection sit in the balance block without entry or confirmation
dialogs. Methods come from the demo snapshot's `paymentMethods`; the selected method
is retained in transaction details without calling payment providers. Amounts remain
whole USD from 1 to 1000. After a top-up, the input clears and the result appears on
the page. The demo balance resets on reload and is explicitly marked as simulated.
Balance shows top-up transactions; request history stays in Usage. Charts and tables
share fixtures, while Overview and Usage preserve their filters independently.

## Design and localization

[DESIGN.md](DESIGN.md) describes implemented styles, components, mobile layout, focus
and reduced motion. This README owns startup and data behavior rather than repeating
pixel dimensions or layout-change history. Local Golos Text fonts and their SIL OFL
license are in `public/fonts`.

Development-only fixtures and in-memory operations remain demonstrations even when
the interface does not repeat a demo label. Error and disconnected-backend warnings
remain visible. Both entry points share the theme; localStorage failures do not block
the interface.

The landing page, FAQ, customer dashboard, owner panel and disconnected Hosted screen
support English, Russian and Simplified Chinese. English is the primary language and
fallback; browser language does not override it. The preference persists locally,
independently of the theme, and synchronizes between tabs. Language changes preserve
drafts, filters and data. The landing page and service entry use `LanguageSwitcher`:
a desktop popover and a native selector on narrow/touch screens. The dashboard uses
the separate `HeaderLanguageSwitcher` strip. Both retain accessible names and keyboard
controls.

Keep source documentation in English and translations in separate locale-specific
files. UI translations live in `src/i18n/messages/en.json`, `ru.json` and `zh-CN.json`.
Components use stable semantic keys such as `t("common.save")`, not English sentences
or language-specific branches. Shared actions reuse a key; distinct meanings such as
the top-up action and transaction type have separate keys. The English JSON defines
`MessageKey`, so TypeScript catches unknown keys. Interface text, tooltips,
accessibility labels and notifications belong in these resources.

To add a language, create `src/i18n/messages/<locale>.json` with matching keys and
`{name}`/`{count}` placeholders. Add its code, name, short label, text direction,
week start and dynamic dictionary import to `src/i18n/registry.ts`. Components do not
need copies. An SVG flag may be added to `LanguageFlag.tsx`; otherwise the short label
is used. Check the calendar, long labels, mobile screens and text direction before
enabling the locale. Dictionary checks automatically iterate over the registry.

English is immediately available. Other resources load only when selected or when
restoring a saved language. Dictionaries and in-flight loads are cached separately;
a late response cannot override a newer choice. On failure, the current language
remains active and loading can be retried. Missing or empty messages fall back to
English. New resources do not add every dictionary to the initial download. URL-based
language selection is not implemented yet.

Numbers, amounts, durations and dates use the selected locale. The calendar stores
ISO dates regardless of input format; money arithmetic remains integer microUSD.
Key/account names, identifiers, model names and arbitrary provider data are not
translated.

## History, keys and navigation

Payment history is separate from requests: top-ups and refunds filter by period,
type and status, sort newest first and paginate by 10/25/50 rows. Spending appears
only in Usage, without a duplicate balance-page block. Unknown amounts remain unknown;
pending settlement is not free usage. Requests and model/API-key groups paginate by
8/16/32 rows; totals cover the selected dataset, not the current page. Phone rows show
essential information, with request details in their dialog. This pagination operates
on the loaded frontend snapshot and does not replace future server pagination.

Charts and summaries share one snapshot range. Speed is weighted by the duration of
measured requests; empty intervals remain gaps.

Only sign-out appears at the bottom of the sidebar; theme and language live in the
header. The logo collapses or expands navigation without changing the current section;
on phones it closes the drawer. There is no separate collapse button, preview/backend
caption or profile menu. The Connection and help page, its dialog and links were
removed; `/app/help` and `/preview.html?page=help` show Page not found. Owner Connections
remains available. The welcome dialog was removed entirely.

Sign-out requires confirmation; Cancel and Escape leave the dashboard open. Confirming
shows the preview-ended screen; returning to the dashboard reloads the initial
fixtures. Sign-out changes only local state. API keys have their own section; real
authentication, key issuance and session management remain future work.

Keys use a compact table with a shared header and centered columns: name, API key,
status, models, daily limit, expiry and actions. Headers sort by name, status, models,
limit and expiry. Narrow layouts replace them with labelled fields and a sort selector.
Creation opens above the table; editing and reissue stay below the key row, without
dialogs. Revocation requires the explicit second action Yes, revoke. Cancel, Escape
and outside click disarm confirmation. Reissue does not automatically revoke the old
key. A new key appears once in an inline copy region; Done or leaving the page hides
the full value. While that region remains open, further creation and reissue are
disabled to protect the unacknowledged key.

The bell opens a compact notification panel beside the button, without a backdrop
or page lock. Notifications can be marked read or opened in request detail. Clicking
the bell again, Escape or outside click closes the panel.

## Separate launcher window

1. Run `bun run web:dev` at the repository root, or `bun run dev` inside `web/`.
2. In another terminal, start the development launcher with `bun run app:dev`.
3. Select Hosted, then Open Hosted demo.

The `hosted-preview` window opens only a fixed loopback address, uses InPrivate/incognito
and has no Desktop capabilities. Arbitrary navigation and new windows are blocked.
It does not read launcher accounts, tokens or runtime state. Opening it again focuses
the existing window. If the web development server is unavailable, the launcher shows
startup instructions. Closing the demo leaves the API running. Tests may use a Tauri
mock; real WebView2 acceptance is a separate local check. Release builds hide the button
and reject the native command.

`index.html` is the independent app entry; `/` shows the public landing page. FAQ and
feature links use `/#faq` or `/#features`, not separate routes. All dashboard links
open `/app/overview` without an intermediate dialog. Until integration, production
dashboards show unavailable data, not zero balances or successful authentication.
`/login` and `/admin/login` remain disconnected; routes alone grant no permissions.
Unknown sections show a 404 screen.

The production build contains `index.html` and lazy-loaded dashboard code;
`preview.html` and fixtures remain development-only. Static hosting must return
`index.html` for `/app/*` and `/admin/*`. API integration, sessions, access checks and
server acceptance belong to H13/H14/H15, not this UI migration.

## Structure

- `src/features/`: Desktop-independent pages/panels, view models and formatting.
- `src/components/`: dialogs and safe copying with a manual fallback.
- `src/i18n/`: locale registry, loading and dictionaries.
- `src/preview/`: separate development-only entry and synthetic fixtures.
- `tests/unit/`: token/cost arithmetic, unknown values and support summaries.
- `tests/e2e/`: details, filters, keyboard interaction, themes and mobile layout.

In the live dashboard, billing components and final charges must come from an
authorized Hosted API customer projection. The current pure calculation function
supports synthetic-fixture consistency and presentation checks.

Component costs appear in token rows. Cached tokens are part of input; reasoning
tokens are part of output. `null` means unknown. Execution and accounting states are
independent. Hosted shows the final charge, not an API equivalent, and an API key
replaces the local connection. Token counts and costs form one list, input before
output. Protocol, transport, tier and reasoning settings live in collapsed technical
details beside request and model IDs.

Diagnostics downloads an allowed summary through the browser, without Desktop IPC,
local accounts, prompts, responses or secrets. The final charge remains in the top
summary and component costs remain in token rows. Pending-settlement and discrepancy
warnings are preserved.

## Checks

Build, unit/browser test and Chromium setup commands are in the
[web preview checks](../CONTRIBUTING.md#web-preview). CI checks this package separately.
Hosted backend and real WebView integration are checked after they are connected.

Contracts: [Hosted architecture](../docs/hosted/HOSTED_RELAY_ARCHITECTURE.md),
[web design](../docs/hosted/HOSTED_RELAY_WEB_DESIGN.md) and
[H13/H14 roadmap](../docs/hosted/HOSTED_RELAY_ROADMAP.md#h13--default-web-application-and-typed-transport).
[DESIGN.md](DESIGN.md) describes current styles and localization. Real sessions, keys,
payments, the server catalog, API compatibility and full H13/H14/H15 acceptance are
not implemented yet; the presence of screens does not complete those milestones.
