# Relay Hosted web

[Russian translation](README.ru.md)

The Hosted customer and owner UI is a **preview, not a working Hosted service**.
Authentication, server persistence, real keys, payments and API integration are
not implemented. Their delivery and acceptance remain in the
[Hosted roadmap](../../docs/hosted/HOSTED_RELAY_ROADMAP.md).

This React/TypeScript/Vite package has its own entry point, dependencies and build.
It reuses pure Desktop presentation helpers and styles from the same checkout,
but has no Desktop IPC, credential-store or runtime access.

## Local development

From `hosted/web/`:

```powershell
bun install --frozen-lockfile
bun run dev
```

Open `http://127.0.0.1:1441/`. After installing dependencies, `bun run web:dev`
from the repository root starts the same server. Development uses synthetic
fixtures; the production build excludes them and simulated authenticated sessions.

## Dashboards and synthetic data

| Entry | Sections |
| --- | --- |
| `/` | Public landing page, models entry, feature and FAQ anchors |
| `/app/overview` | Overview, Usage, Models, Status, Keys, Balance |
| `/admin/overview` | Overview, Connections, Pool, Models, Usage, Notifications |

`src/features/app/routes.ts` owns dashboard routes. Unknown sections show Page
not found. Customer and owner surfaces are separate; the customer has no role
switch. `/preview.html?page=…` is a development-only entry to the customer UI.

Development operations change an in-memory snapshot, not a server. Connections,
pool membership and account economics use synthetic data. Adding a connection
does not admit it to the pool; admission requires confirmation. Restoring a
trashed connection does not rejoin the pool or resume a paused connection.
Reloading resets window-local changes. Catalog edits can also reach other open
preview tabs, as described below.

The production entry shows disconnected or unavailable data. `/login` and
`/admin/login` do not authenticate anyone; opening an owner route grants no
permissions. Static hosting must serve `index.html` for application routes,
including `/app/*` and `/admin/*`. This only serves the UI, not a backend.

## Catalog and connections

- Owner catalog edits synchronize between live preview tabs through
  `BroadcastChannel`. Without it, edits stay in the current tab. Closing all tabs
  loses them. This is not persistence, authorization or tariff publication.
- Both dashboards use the same catalog projection and model order. Reordering
  models does not change rotation, route availability or charges.
- API connection editing affects demo data only. It does not accept provider
  keys or make provider requests. Quota and source refresh remain unavailable
  until API integration. Local accounts are never imported into this preview.
- Account economics are API-equivalent estimates, not customer charges or
  provider costs. Shared quota counts once per ledger; cost recovery requires
  paired usage and purchase-price evidence. Partial totals stay approximate.
- Unknown quota, price, balance and usage stay unknown, not zero. Expired quota
  does not become a fresh full allowance. A catalog entry is not availability
  evidence; service-status history stays neutral without monitoring data.
- `src/features/models/model.ts` defines the displayed offers. Rates are integer
  microUSD per million tokens. Published tiers, context bands and comparisons
  belong to the server contract; the UI does not infer them from model names,
  maximum context or request history. Missing rates are not borrowed from another
  tier, and cache-write durations remain separate.

Fixture changes and local arithmetic test presentation consistency. They do not
implement server-side pricing, settlement, access checks or live provider support.

## Design and localization

[DESIGN.md](DESIGN.md) owns current visual patterns and interactions.
[PRODUCT.md](PRODUCT.md) describes the product scope. Local Golos Text fonts and
their SIL OFL license are in `public/fonts`.

English is the default and fallback; browser language does not override it.
Russian and Simplified Chinese dictionaries load on selection or restoration of
a saved preference. Theme and language can use localStorage; storage failure does
not block the UI. Language changes retain drafts and filters and synchronize
between tabs. Late dictionary responses cannot override a newer selection.

To add a language:

1. Add `src/i18n/messages/<locale>.json` with the English dictionary's keys and
   matching placeholders. Components use semantic `t("common.save")` keys.
2. Register its code, name, short label, direction, week start and dynamic import
   in `src/i18n/registry.ts`. A flag in `LanguageFlag.tsx` is optional; the short
   label is the fallback.
3. Run dictionary/unit checks and inspect long labels, dates, mobile layout,
   keyboard controls and text direction before enabling it.

Numbers and dates follow the selected locale; stored dates remain ISO and money
arithmetic remains integer microUSD. Identifiers and provider data are not
translated. Keep primary documentation in English and translations in separate
locale-specific files. URL-based language selection is not implemented.

## History, keys and navigation

Request history and payment history are separate. Filters and pagination operate
on the loaded snapshot, not server pages. Overview uses rolling periods ending
at `snapshot.asOf`; Usage's Today starts at midnight UTC. Empty chart intervals
and unknown charges are not fabricated observations or free usage.

Cached tokens are part of input; reasoning tokens are part of output. Execution
and accounting states are independent. Hosted usage presents customer charges,
not Desktop API-equivalent estimates. Final live charges must come from the
authorized Hosted API, not browser calculations.

Demo keys are shown in full once and are not stored or put in URLs. Creation,
reissue and revocation affect preview state only; reissue does not automatically
revoke the old key. Demo top-ups change window-local state without calling a
payment provider. Confirmed sign-out ends the local preview, not a server session.

Diagnostics downloads an allowlisted summary without prompts, responses,
credentials or Desktop state. Error, disconnected-backend and settlement warnings
remain visible even where the preview does not repeat a demo label.

## Separate launcher window

1. Run `bun run web:dev` at the repository root.
2. In another terminal, run `bun run app:dev`.
3. Select Hosted, then Open Hosted demo.

The development-only `hosted-preview` window uses a fixed loopback address and
InPrivate/incognito mode, with no Desktop capabilities. Arbitrary navigation and
new windows are blocked. Reopening focuses the existing window; closing it leaves
the local API running. Release builds hide the button and reject the command.
Browser tests with mocks do not establish real WebView2 acceptance.

## Structure

- `src/features/`: pages, view models and presentation logic.
- `src/components/`: shared UI and copying with a manual fallback.
- `src/i18n/`: locale registry, loading and dictionaries.
- `src/preview/`: development entry, fixtures and in-memory operations.
- `tests/unit/`: presentation, arithmetic, unknown values and data boundaries.
- `tests/e2e/`: routes, interactions, themes, localization and responsive layout.

## Checks

From `hosted/web/`, run `bun run test`, `bun run build` and `bun run test:e2e`.
Chromium setup, prerequisites and targeted commands are in the
[web preview checks](../../docs/CONTRIBUTING.md#web-preview). CI checks this
package separately. The production preview command uses port 4174; browser tests
use port 1442 rather than the development server's 1441.

Future service contracts are in the [Hosted architecture](../../docs/hosted/HOSTED_RELAY_ARCHITECTURE.md)
and [web design plan](../../docs/hosted/HOSTED_RELAY_WEB_DESIGN.md). Passing UI
checks does not complete backend, authentication, billing or installed-WebView
acceptance.
