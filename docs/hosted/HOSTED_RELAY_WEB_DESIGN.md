# Hosted Relay Web Design

**Design specification for future implementation.**
Some screens already exist in the [web preview](../../hosted/web/README.md); its guide
describes what can run locally and which integrations are still pending.
This document owns visuals, screen composition and interaction states.
[Architecture](HOSTED_RELAY_ARCHITECTURE.md) owns routes, permissions and data/money
semantics; [roadmap](HOSTED_RELAY_ROADMAP.md#hosted-implementation-work-packages)
owns delivery and acceptance. Link to their owning sections.

Navigation: [principles](#design-principles) · [visual system](#visual-system) ·
[layout](#layout-and-responsive-behavior) · [components](#shared-components) ·
[screen inventory](#screen-inventory) · [public entry](#public-entry-and-sign-in) ·
[customer cabinet](#customer-cabinet) · [owner panel](#owner-panel) ·
[shared states](#shared-states-and-interaction-rules) ·
[handoff](#design-handoff-and-acceptance).

## Design principles

1. **Owned implementation.** The public landing uses a green field,
   large locally hosted type and an original
   woven connection diagram. Keep Relay's palette and factual product copy.
   Do not invent traffic, availability, pricing or customer data.
   Customer and owner screens extend the approved identity in an operating mode:
   pine navigation, cream selection, Golos Text and quiet work surfaces. Preserve
   their data, accounting meaning and actions, but recompose page bodies for
   Hosted rather than preserving the Desktop arrangement. Desktop is outside
   this visual change.
2. **Clarity before decoration.** The customer can answer: what is available, how
   to connect, what was used, what was charged and what action is safe now.
3. **Money and uncertainty are explicit.** Available, held, charged and pending
   are different labels, not interchangeable numbers. An error is not proof that
   a request was free. Missing data never looks like a successful zero.
4. **Three distinct environments.** Public product explanation, customer cabinet
   and passkey-protected owner panel share primitives, not data or authority.
5. **One obvious next action.** Each screen has a clear primary task. Secondary
   navigation is quiet; destructive and chargeable actions are never defaults.
6. **Reuse proven components.** Use suitable licensed accessible primitives,
   chart libraries and Relay utilities. Keep dependency and asset provenance.

The visual direction follows Relay: warm neutral light surfaces, charcoal dark
surfaces, a muted green action accent, clear typography and restrained grouping.
Use solid surfaces, compact metrics and activity backed by request data. The web
scope covers public information, the customer cabinet and owner controls; retain
Relay's existing Desktop design, light/dark themes and single-workspace model.

The public landing uses a deliberately short, separate visual composition: a plain
explanation of API access, a large green opening and a full-width woven connection
field. Four first questions follow on the same page, with all answers visible.
Native scrolling and the Questions anchor move to them; scrolling is not intercepted.
There is no helper note under the
hero actions, product-data showcase, pricing fixtures or placeholder
connection settings. Start and Open cabinet enter Overview directly; Models and
pricing opens the catalog. Login may guard a target, but no welcome dialog precedes it.
The connection field is illustrative, not live traffic: three SVG thread bundles
connect an application through Relay to a model. Request strokes travel at constant
speed with staggered 4.8–6.4 second cycles. Hover and theme changes do not reset the
phase; reduced motion, a hidden tab or offscreen content stop the animation.
Golos Text is served locally for the landing, customer/owner workspaces and entry
screens with Cyrillic coverage and its SIL OFL license; no font-service requests
are made. The shared mark, palette and type connect the surfaces; decorative
traffic remains exclusive to the landing. Shared and surface-specific tokens live in
`hosted/web/DESIGN.md` and the development-only Impeccable surface brief.
The public landing omits demo/development labels; synthetic
money and account data are confined to development fixtures. The frontend
now exposes the customer `/app/*` shell and the separate owner `/admin/*` shell;
the landing enters the customer shell. Production builds omit fixtures and show
unavailable data/actions until backend integration, without inventing a session,
balance or role. Sign-in remains disconnected. One shared theme preference carries
between landing and cabinet. Local mode, application launching, profiles and Desktop
runtime/credentials are not copied into the web application.
The cabinet reuses base components with a Hosted-only composition layer:
232-pixel sidebar (56-pixel collapsed rail), 32-pixel content gutters (24 below
1100px; 16 on phones) and a full-width token chart. At a
workspace-container width of 960px, related charts form pairs below the primary
series; narrower layouts stack them. Overview starts with charts, without
a second row of headline totals. Usage joins headline totals,
token counters and generation measurements in one panel before its data table;
secondary metrics share its lower section rather than a separate speed card.
Exact charges and uncertainty hints remain available. Navigation retains pine
green in both themes, pale supporting text and a cream selected row. The pine balance
panel sits beside payment history at wide widths. Its demo top-up form is inline:
amount input, quick amounts and a payment-method selector sourced from the demo
snapshot. Errors and success stay on the page; no payment provider is called.
Keys, connections and pool members use full-width resource rows. The logo toggles
the rail without navigation or closes the phone drawer. Customer balance and keys
sit above the footer divider; sign-out is the footer action. No Help/Settings
sidebar entry. Security and setup actions appear in context when their APIs exist.
These compositions collapse to one column when their container is narrow.
Request details place model/status above key and UTC context, with diagnostics
at the trailing edge. Exact posted charge leads the left-aligned timing row;
the token total anchors two open token columns rather than nested cards.
Parameters stay quiet and left-aligned; technical details use the existing
disclosure. Phone layouts stack token groups and retain 44px export/copy targets.
Missing charges, token counts and accounting warnings keep their existing meaning.
Forms, notifications,
onboarding, empty results and signed-out/not-found screens share the same materials.
Headings are 28px on desktop and 24px on mobile; controls are 40/44px high with
8px corners and soft shadows. The cream balance action is flat on pine. Information
panels use 16px corners; tables and settings control groups use 12px.
Keyboard focus uses a two-pixel accent outline with a three-pixel gap, cream in
the pine sidebar, and system Highlight in forced-colors mode. Mobile utilities are
sticky in normal flow; the navigation drawer retains focus trapping and Escape.
Only presentation crosses the boundary; native state, IPC, credentials and local
controls do not. Hosted calculations, role separation and disconnected state remain
unchanged. The current shell dimensions supersede earlier layout proposals below.

## Visual system

### Color tokens

Components use these shared semantic tokens. Theme
configuration may change brand tokens only after the same contrast checks pass.

| Semantic token | Light | Dark | Usage |
| --- | --- | --- | --- |
| `canvas` | `#F4F3EE` | `#141414` | Page background |
| `surface` | `#FFFEFA` | `#1B1B1B` | Cards and dialogs |
| `surface-subtle` | `#F7F8F2` | `#171717` | Sidebar, header, table header and secondary blocks |
| `border-subtle` | `#DCE1D8` | `#333332` | Nonessential panel separators; not a lone control boundary |
| `border-control` | `#BACBBD` | `#555550` | Input/secondary-control boundary; retain visible focus |
| `text` | `#202B29` | `#F2F0EB` | Main content |
| `text-muted` | `#65716C` | `#AAA9A2` | Supporting text, units and timestamps |
| `accent` | `#15644D` | `#80B49A` | Primary actions and selected navigation |
| `on-accent` | `#FFFEFA` | `#102018` | Text/icons inside a filled primary action |
| `accent-hover` | `#104C3B` | `#A0CCB3` | Hover/pressed primary action background |
| `focus` | `#15644D` | `#80B49A` | Two-pixel focus outline with two-pixel surface gap |
| `success` | `#15644D` | `#80B49A` | Confirmed successful state, always with text/icon |
| `warning` | `#916013` | `#E2AA52` | Pending/limited state requiring attention |
| `danger` | `#A52D2A` | `#FFA8A4` | Confirmed failure or destructive action |
| `info` | `#246DA8` | `#67B7EE` | Informational state or explanation |

Status text sits on `surface`/`surface-subtle`.
A filled destructive button uses light-theme `danger` with white text in both
themes; verify hover/focus combinations separately. Disabled controls retain a
readable label and an adjacent reason.
Do not reuse the error color merely because a tariff or balance is expensive.
Project acceptance requires at least 4.5:1 for ordinary text and 3:1 for necessary
control boundaries/indicators against their adjacent background. This token table
is not a certification of every rendered combination.

### Typography, spacing and surfaces

- Use locally served Golos Text with Cyrillic support and its bundled OFL license;
  no remote font requests. Keep the system fallback for Chinese glyphs.
  Monospace is reserved for IDs, API paths and configuration examples.
- Type sizes/line heights: hero 40/48 desktop, 30/38 mobile; page title 28/36;
  section 20/28; body 16/24; dense data 14/20; secondary caption 12/16. Critical
  amounts, form instructions and action labels never use caption size.
- Use sentence-case navigation with normal/medium/semibold emphasis. Monetary values,
  tokens and durations use tabular numerals, right alignment and explicit units.
- Spacing scale: 4, 8, 12, 16, 24, 32, 48 and 64 CSS pixels. Use 24 between desktop
  page blocks, 16 on mobile, 8 between a label and its field, 4 to field feedback.
- Controls are 44 pixels high by default; primary touch targets are at least
  44 by 44. Radius: controls 8, panels 12, dialogs 16. Badges may be pill-shaped;
  use borders and backgrounds to group related content.
- Panels have a subtle border without a default shadow. Menus/dialogs alone get
  restrained elevation; modal backdrop obscures content without blur dependence.
- Use one coherent icon style with a 20-pixel visual box inside the target area.
  Label unfamiliar icons. Icons do not replace status text or provider names.
  Use licensed generic icons; only use provider assets under applicable permission.
- Default theme follows the OS, with explicit Light/Dark/System choices. Keep
  theme/locale in non-sensitive preferences; never persist authenticated data there.

### Language, numbers and time

Use concise descriptions of the current function, state and next action.
Keep necessary warnings visible; supporting detail belongs with the relevant action.
Use the [locale registry](../../hosted/web/src/i18n/registry.ts): English is the
initial language and fallback unless a supported preference was saved; Russian
and Simplified Chinese are also shipped. Browser language does not override it. Changing locale
does not restart a payment/login operation or translate machine model IDs/codes.
Keep source documentation and its example copy in English. Additional languages
belong in separate locale files or explicitly localized documents.
Allow longer Russian labels to wrap; never solve overflow by shrinking everything.

Follow architecture's exact money and UTC report definitions. Wallet headlines
may be compact, but an adjacent accessible disclosure exposes the exact value;
small nonzero request charges display six decimals. Never
recalculate a debit in the browser. Display USD explicitly at first occurrence
and in every export/confirmation; label token counts with their own units.

Reporting controls and chart axes say UTC. Row/detail timestamps are explicit UTC
in the first release. Local-time equivalents may be supplementary in detail and
announcements, always with timezone/UTC available. Use an em dash with explanation
for unavailable values, not `0`, `NaN` or a blank cell. A timestamp says whether it
is request start, journal posting, observation or scheduled effective time.

## Layout and responsive behavior

Pixel dimensions describe the nominal baseline. Use
scalable typography and let headers/cards/forms grow for zoom, localization and
wrapped content. A breakpoint cannot override reading order or reachable actions.

### Public shell

Align hero, FAQ and footer with the existing landing column in
[the web design system](../../hosted/web/DESIGN.md). Keep the mark on the left,
language, theme and cabinet entry on the right; no extra section-navigation menu.
The sticky header must not cover anchor headings or focused fields.
Footer groups product/help and operator-provided legal/support links compactly.
No fixed deployment domain or unverifiable trust logo.

Use a restrained Relay wordmark and an original small route/connection motif.
Review final vector artwork for appearance and provenance. Keep Desktop's existing
brand files. Favicon remains recognizable at small size without text.

### Customer shell

Keep the existing rail, responsive drawer and six [primary destinations](../../hosted/web/src/features/app/routes.ts)
in order: Overview, Usage, Models, Status, Keys, Balance. The header holds page
context, notifications, language and theme; account security is a contextual task,
not a role switch or another sidebar item. Show a service-condition banner only
when relevant. The mobile drawer traps focus, closes on explicit action/Escape
and returns focus to its trigger.

### Owner shell

Authenticated owner pages use the same sidebar and content geometry as the
preview, with `Owner` in the header and service access mode in Overview.
Production links to Overview, Connections, Pool, Payment methods, Usage and
Notifications. The preview's Models screen has no production sidebar entry;
published model configuration remains in Pool. Sign-in stays outside the
workspace. Link guarded setup, security, refund and audit tasks from their
owning context without adding sidebar entries.
No CSS class or selected role grants permissions.
The customer cabinet is a separate authenticated destination, not an admin preview
that can display another customer's session. Do not show an impersonation button.

### Reflow rules

- Summary cards: four columns when they fit, two on medium widths, one on narrow
  widths; current wallet groups remain visually separate from period summaries.
- Two-column detail: main area 2/3 and supporting facts 1/3; stack in reading order
  on narrow screens. Forms do not become multi-column below 768 pixels.
- Filter rows wrap without losing labels. On mobile show period and active-filter
  count, with remaining filters in a full-width sheet and Apply/Reset actions.
- Tables keep keyboard-accessible headers/actions. Mobile shows priority fields
  and expandable detail; optional horizontal scrolling is only inside a labelled
  table region, never the whole page. No required action is hidden offscreen.
- Request detail: up to 860 pixels wide on desktop, using Relay's overview,
  parameters, token groups and technical disclosures. On mobile fit the viewport
  with a visible sticky heading/close control. Long diagnostics use a full page.
- Dialog: maximum 480 pixels for confirmation, 640 for a short form. Complex tasks
  use a page. On small screens dialogs fit the visual viewport and software keyboard.
  Never stack modal dialogs; complete step-up and return to the pending action.
- Charts start around 280 pixels high on desktop and 220 on mobile. An accessible
  table remains available; pinching, dragging and hover are not required to read data.

## Shared components

| Component | Content and behavior contract |
| --- | --- |
| Page header | One H1 and the controls needed for this task. A read-only page need not have a primary button or a subtitle. |
| Action button | Verb plus object; default, focus, hover, submitting, disabled-reason and result. Submission locks the same operation, not the entire page indefinitely. |
| Field group | Persistent label, optional hint, input, inline error; required marker explained once. Never use a placeholder as the only label. |
| Amount display | Exact server value/currency/basis; distinguish current, held, posted and pending. Copy exposes the safe exact value. |
| Status badge | Icon, text and semantic tone. Execution and accounting get separate badges; unknown is not success or automatic error. |
| Summary card | Metric name, value/unit, population/period, freshness/coverage and optional drill-down. No zero or percentage until evidence exists. |
| Filter bar | Labelled UTC period, supported dimensions, active filters, Reset and bounded refresh. Unsupported combinations are explained, not silently ignored. |
| Data table | Fixed meaning/order, server sort/filter/cursor, pending/empty/error states and explicit row actions. Sorting never changes the query's authorization scope. |
| Chart panel | Title/unit, population, legend, coverage/as-of, tooltip by pointer/focus and Show table. Gaps remain gaps; legend choices cannot change financial totals. |
| Detail sheet | Stable title/ID, state, facts, relevant actions and Close; preserve parent filters/scroll on return. Authorized fresh read required for deep links. |
| Notice/banner | Severity, concise fact, next action and timestamp when relevant. Persistent service/financial warnings are not dismissible toasts. |
| Toast | Short nonessential confirmation, not the only proof of a financial operation. Never auto-copy a secret or read sensitive detail aloud. |
| Confirm/step-up flow | Action, target, consequences, current revision and Confirm/Cancel. Require fresh proof only per server policy; no action is executed by completing proof alone. |
| Code/copy block | Protocol-specific safe example with placeholder secret; explicit Copy, success/failure feedback, no token in the URL, telemetry or saved preferences. |
| Pagination/export | Bounded cursor navigation and authorized asynchronous export. Preserve filters/snapshot; show expiry, partial coverage or bound instead of silently truncating. |

## Screen inventory

The two sidebars follow the [route registry](../../hosted/web/src/features/app/routes.ts)
in the order below. WS IDs describe tasks, not additional navigation. Follow the
[customer walkthrough](HOSTED_RELAY_ARCHITECTURE.md#first-customer-request-walkthrough)
or the [owner import walkthrough](HOSTED_RELAY_ARCHITECTURE.md#from-connection-import-to-first-offer)
to see how the tabs connect.

| Customer tab | What the person can do | What they get |
| --- | --- | --- |
| Overview `/app/overview` (WS-03) | Choose period/model; open a chart interval in Usage. | Requests, tokens, spend and recorded speeds in that window. One contextual next step for a new or blocked account. |
| Usage `/app/usage` (WS-04) | Filter Requests/Models/Keys/Errors, open a group then a request, copy its safe reference or export the filter. | Own execution result and charge status separately, with coverage and exact historical charge. No resend. |
| Models `/app/models` (WS-05) | Inspect granted offers, compare published context bands, copy a model ID. | Model, protocol and applicable prices. Changing the displayed band does not configure the client. |
| Status `/app/status` (WS-05) | Choose 24h/7d, find a model and inspect an interval by pointer, keyboard or touch. | Observed availability and scoped request/latency samples, or no evidence. No check/probe button. |
| Keys `/app/keys` (WS-06) | Create, copy once, edit limits/expiry, reissue or revoke with inline confirmation; retrieve nonsecret client setup. | Masked list and effective policy. Reissue first, migrate client, revoke old key separately. |
| Balance `/app/billing` (WS-07) | Top up, return to the same payment, inspect receipt/posting or request an eligible refund. | Available funds, outstanding holds when relevant, and authoritative payment/journal results. No duplicate cost graph. |

| Owner tab | What the owner can do | What changes |
| --- | --- | --- |
| Overview `/admin/overview` (WS-13) | Choose 24h/7d/30d/All time; drill into Usage, readiness or an outstanding case. | Platform charts, including **posted customer charges**. Reading never restarts, tests or opens the service. |
| Connections `/admin/connections` (WS-14) | Add/reauthenticate Accounts; edit an API's purchase rates, default markup and per-model selling prices; import/bind Proxies; park, trash or restore. Open the membership action or pair Desktop when needed. | Protected inventory and pricing drafts. Saving an API does not admit it to the pool or publish an offer. |
| Pool `/admin/pools` (WS-15) | Participants: add from inventory, edit member rules, pause/resume, remove, configure rotation, see active model/member. Model rules: edit state/prices inline, reorder and publish. | Acknowledged membership/policy or tariff revision. A live highlight is observed work, never the predicted next member. |
| Models `/admin/models` (WS-16) | Inspect the effective storefront, compare bands, copy the ID; open its Pool rule to edit. | What is actually published, without impersonating a customer or displaying drafts as prices. No second editor. |
| Usage `/admin/usage` (WS-20) | Filter platform history/errors, inspect the request and its attempts, or follow an exact support reference to a guarded case. | What executed, what was charged, what remains unresolved. No replay, prompt viewer or editable charge. |
| Notifications `/admin/notifications` (WS-10) | Create/edit/discard a draft; review audience, confirm publication, then check delivery. | One in-app notice per eligible recipient/revision. Saving a draft or reopening the tab sends nothing. |

**Outside the sidebars:** WS-01/02 handle public entry and customer sign-in;
WS-08 is signed-in account/security, WS-10 the customer's bell. WS-09 remains
removed. Owner sign-in (WS-11) and setup (WS-12) lead to the same owner workspace.
Reach pairing, support/refunds, operations, security and audit (WS-17–19/21–23)
from the [owning context](HOSTED_RELAY_ARCHITECTURE.md#owner-browser-navigation).
WS-24 applies everywhere, not as a destination.

**What stays out of the cabinet:** no onboarding dashboard, playground or paid “test” button;
no provider accounts, proxy/rotation settings or customer directory in the
cabinet; no permanent admin menus for exceptional cases; no duplicate price,
wallet or notification editor. Do not add a control just because an API exists:
omit unsupported actions; explain a temporary blocker beside a real action.
Keep a way to recover a payment, key issue or import after the original tab closes.

## Public entry and sign-in

### WS-01 — Landing page

**Entry:** anonymous `/`; signed-in visitors may still read it. The header action
becomes Open cabinet. Never
render personalized balances into public/cacheable HTML.

**Composition:** keep the hero and four visible FAQ answers (`/#faq` or `/#features`).
Explain how to use a Relay key in an external client. If this site offers Image
Studio, link it as a separate way to use the same balance, not as an API test.
Start/Open cabinet targets Overview, via `/login` only when
needed; no first-use dialog. Theme and language require no account. Keep
configured terms, privacy, support and independent service status in the footer.

**Before paying:** one quiet Models and pricing link opens the planned public
`/models` directly. Reuse the catalog table with the public projection only;
visitors can compare prices/bands without registration. The preview currently
links to `/app/models`, which in the service is the **authenticated** catalog.
Do not stretch the landing with a second price table, invented discounts,
performance counters or a seven-section sales funnel.

**States/actions:** public configuration failure shows neutral branding and an
unavailable message, not another installation's defaults. If prices cannot load,
retain the explanation but replace the table with an explicit unavailable block.
If there are no published models, say so and do not advertise functioning paid
access. Visible login methods and whether registration is open come from policy.
A maintenance/closed banner is above the hero; do not hide outage/support links.

**Mobile:** hero becomes text then diagram; diagram is decorative to assistive
technology when its text is already stated. Pricing rows become model cards with
labelled component prices and Details.
No invented customer counters, reviews, endorsements or product screenshots.

**Linked site pages:** put **API guide** (`/docs`) and **Policies** (`/legal`) in
the footer. Link to the same guide from the key setup example and to the operator's
terms/refund policy before checkout. No sign-in is needed to read either page.
When offered, **Image Studio** (`/studio`) follows the
[sign-in and key selection flow](HOSTED_RELAY_ARCHITECTURE.md#entering-studio-with-a-relay-key).
After sign-in, show the key picker before the model controls. When there are no
keys, put **Create key** in the picker's place; it opens **Keys** in the cabinet,
not an inline form. Once a key is selected, show its eligible models and rates.
If the selected key or offer becomes unavailable, do not switch it silently.
Keep these entries out of both sidebars. Use the content and branding of the
[current installation](HOSTED_RELAY_ARCHITECTURE.md#public-source-and-operator-installations).

### WS-02 — Customer sign-in and return

**Entry:** `/login`, or a protected customer route without a valid customer
session. Store only a validated same-origin destination, not arbitrary redirects.
Returning to a protected detail still requires a fresh authorized lookup.

**Composition:** centered surface up to 440 pixels wide: brand/back-to-site;
`Sign in to Relay`; short statement that a new account may be
created if registration is open; vertical configured provider buttons; required
policy/terms disclosure; support and theme/language controls. Do not render empty
email/password fields, an unsupported password reset or a separate duplicate
registration form. Customer Google/Telegram/OIDC sign-in is not owner sign-in.

**Flow:** choose an enabled provider -> show Leaving for sign-in -> use the
server-created provider flow -> return through the configured callback -> verify
session/workspace -> open Overview or the validated destination. First registration
shows only the policy acceptance required by the reviewed server contract; no
preselected optional marketing consent. Same-email identities are not silently
merged. Identity linking starts in the signed-in account context instead.

**States:** user cancelled, provider temporarily unavailable, attempt expired,
identity needs explicit linking, registration closed and session confirmation
unavailable each get a safe explanation/action. A failed callback does not dump
provider content, authorization codes or tokens. A duplicate return cannot create
another account. Once session verification is unknown, offer a safe session check,
not repeated side-effectful account creation. No automatic switch to a different
provider that might sign the user into a different account.

**Mobile:** one-column form, no scrolling background illustration. Browser-native
provider controls remain usable with the software keyboard, zoom and back gesture.

## Customer cabinet

Each customer page uses the customer API only. The displayed customer is the
authenticated personal workspace, never a chosen owner-panel filter. Each passport
inherits WS-24's loading, denied, unavailable and session-expiry behavior.
After session verification, use the shared sidebar and mobile drawer. Keep keys
and balance above sign-out. The header links to Balance and Account security,
then shows the bell, language and theme controls.

### WS-03 — Overview

Start with the period/model selector and the charts: normalized tokens, posted
usage charges, logical requests and observed speeds. Default to rolling 24h;
7d/30d also end at the server snapshot, not midnight. Selecting a bucket opens
Usage with the same model, exact UTC bounds and report revision. Missing samples
stay missing. Keep wallet and key shortcuts in the rail, notices in the bell and
request lists/breakdowns in Usage; do not repeat them above or below the charts.

**First visit:** show at most one relevant next action in the empty state, from
confirmed account data: no eligible offer → Models or support; unresolved top-up
→ its payment status rather than another checkout; no funds → Balance; funded
but no usable key → Keys; key and funds but no history → the client setup in
Keys. The user can navigate freely and create a key before funding. With history,
remove the hint. A failed read is not a new account; a filter with no matches
offers Reset, not onboarding. On phones keep the same order, stacking the charts.

### WS-04 — Usage and safe request detail

**Header:** `Usage`; Requests, Models, Keys and Errors are views of the same
authorized history. Select a group to drill into Requests; Back restores the
filters. Filter, reset, refresh or export the selection; none sends inference.
Below: query summary/as-of, compact totals, request table and cursor navigation.

**Table order:** started at UTC; public model; masked key label; protocol; execution
outcome; duration/TTFT where available; normalized token summary; customer charge;
accounting status; Details. Preserve the preview's 8/16/32 row choices within
server bounds. Do not invent a total-page count for
cursor pagination. Revoked keys retain safe historical labels.

**Detail sheet:** top request ID with Copy and separate execution/accounting
badges; model/protocol/key/client label/time; usage components and completeness;
timings with named meanings. Show historical unit rates and component charges
inside the corresponding token groups, alongside token counts; no separate
cost-calculation disclosure. Input/cache and output/reasoning subdivisions must
not double-count tokens or charges. Keep the recorded final charge, tariff revision
and charged/held/pending explanation visible. Technical details follow Relay's
grouped identifiers, response/error, client, timing, context-size and tool sections
where customer-safe evidence exists, without the owner's routing/account timeline.
Include safe error guidance and Copy support summary. Keep User-Agent labelled as untrusted
client description, separate from authenticated user attribution. Never include
physical account identity, provider tokens, prompts, answers or the owner timeline.

**Export flow:** Download CSV exports the selected bounded request snapshot,
including rows beyond the current page. Retained aggregate exports show current
filters, coverage and bounds before Create export. A pending job row exposes
status; the completed download remains authorized and expires. Refresh recovers
the same job; no export claims completeness after truncation.

**States/mobile:** missing usage is Unknown, estimated charge has its basis, and
expired detail links to retained aggregate information only where available.
Unknown execution offers status/support, never blind resend. Mobile rows keep
model/time, both states and charge visible; the remaining fields move to detail.
Closing a sheet restores filters/scroll. A direct `request` query opens only the
current customer's permitted record and does not disclose an unauthorized ID.
When opened from a chart snapshot, show the receipt at that watermark or label
a newer correction and offer Refresh. Never silently compare a new charge with
the old chart total.

### WS-05 — Models and prices

**Header:** `Models`; start directly with the read-only grouped catalog, without
a generic filter toolbar, unit banner or key action. Use the authorized publisher groups and relative
model order from the [catalog contract](HOSTED_RELAY_ARCHITECTURE.md#pool-model-controls-and-catalog-order),
without alphabetical/popularity re-sorting or inferring capability from a name.

**Row/card:** readable public name, with exact model ID available for copying/details;
supported protocols/capabilities;
input, cached input and output rates with currency/per-million unit; band/details
indicator and evidence-backed availability. No Get key/Connect action. Desktop uses a comparative table;
mobile uses labelled stacked rows. Never show internal account counts or inventories.

**Detail:** the server chooses the displayed pricing variant. Show published
context bands once: a single band is static, two switch inline, more use the
existing compact selector. Leave missing bounds blank. Units remain unambiguous
in column labels/accessible descriptions; exact component rates and effective
revision are inspectable. This is a comparison, not a runtime tier selector.
Keep model-ID copy separate from key creation. Show unavailable or not-granted information
only where the server deliberately exposes that public descriptor; no hidden-catalog
discovery via client guesses. Scheduled revisions show effective time and scope;
historical request prices stay in Usage.

The restricted catalog supplies the exact model ID, Responses protocol, published
input/output/cache-read rates and nullable publisher in the saved display order.
Group unknown publishers under **Other** when there are known groups. Missing
context and cache-write rates use a dash; omit savings without a reference price.

**States:** no authorized models points to support/account policy; temporary
capacity is different from absent grant, disabled route or missing tariff. A model
without a complete qualified price cannot look ready for paid use. Do not present
roadmap connectors as available options.

**Status (`/app/status`):** keep 24h/7d beside the heading, the summary above
the model table, and one legend beside search. Read only the
[authorized history](HOSTED_RELAY_ARCHITECTURE.md#model-status-history).
Missing samples stay neutral with accessible text; fewer observations are not
an outage, 100% uptime or a measured zero latency. Latency uses measured
time to completion, with sample coverage in the detail.

### WS-06 — API keys

**Header:** `API keys`, primary Create key. Use the existing inline editor and
row actions, not a key-settings screen.
List columns: name/prefix, status, allowed models/operations, effective limits,
expiry, last use and Actions. Never fetch a stored full secret for this list.

**Create form:** name, optional daily spend limit and expiry; inherit the
workspace's server policy. Do not ask a user to reselect every allowed model.
Display narrower effective scope/limits when relevant; offer a restriction only
where supported. Units, reset time, inheritance and upper bounds are explicit;
empty is not Unlimited. Server field errors stay beside their fields.

**One-time result:** replace the form with a transient secret panel, explicit Copy,
warning that the secret cannot be shown again, and Done. Done closes the panel in
place; arriving from Studio adds no return button. Provide the configured
inference base URL, exact model/protocol and a copyable placeholder configuration
here and from the masked key row later; the secret never returns with them. Do not copy
automatically, put it in a URL or announce it through a live region. Leaving warns
that the display will be lost, not that the key will be undone. If issuance committed
but the response was lost, show the resulting masked key/status and the replacement
path; never silently issue another key or claim the key is absent.

**Edit/replacement/revoke:** edit only server-supported mutable fields with revision
checks. For a replacement, explain creation of a new key, client migration and
explicit revocation of the old key; do not promise revocation until acknowledged.
The first Reissue or Revoke click arms only that key's row. Show the target and
Confirm/Cancel inline; Escape, another row or navigation disarms it. A deliberate
second activation, not a double click or key repeat, submits once. Explain
new-use blocking versus in-flight work at confirmation. A successful revoke is visible
in the list; repeated confirmation addresses the same operation.

**States/mobile:** empty list has Create key; expired/revoked entries are
readable, not deleted. Policy changes/conflicts require reload and review. Full
secret display is private even in responsive screenshots/test fixtures; use synthetic
values. The mobile key card keeps name/status/scope and an accessible action menu.

### WS-07 — Billing, payments and refunds

**Header:** `Balance`; available amount and inline Top up beside payment history,
stacked on phones. Show actual holds when they explain unavailable funds; avoid
a permanent reserve tutorial. Journal detail is reachable from the balance or
a linked request/payment. Pending payments are history rows, not spendable funds.
Use bounded filters/pagination, not a second spending dashboard.

**Top-up steps:**

1. Show the selected method's quick amounts as USD wallet credits in the order
   returned by the server. Show **Custom amount** and its limits only when allowed;
   fixed-only checkout has buttons and no amount field. Respect the server's
   method order: show one method by name, several in one selector. Switching
   methods clears an ineligible selection rather than changing the amount.
   With no eligible method, show `Top-ups unavailable` and leave balance and
   history visible. Each top-up is a one-time payment, not an automatic refill.
2. Review inline: amount paid, wallet credit in USD, fees/conversion if applicable,
   quote expiry, method and applicable policy. Change means requote, not local math.
   If the customer chooses another method while an invoice is open, keep that
   invoice available; only confirming the new quote creates a second invoice.
3. Create payment: one durable operation; redirect to the approved checkout.
   If the open-invoice limit is reached, show the count beside **Top up** and
   link to the customer's open payments in the existing history. Do not cancel
   the oldest invoice or submit the form automatically when a slot opens.
   Relay does not collect raw card details in its own generic form.
4. Return/status: stable payment reference, status and What happens next. Query the
   same payment; provider redirect text is never proof of credited money.
5. Confirmed credit: server acknowledgement, receipt where available, fresh wallet
   snapshot and Return to overview. Never show a success animation while pending.

**Payments table:** one row per invoice, even when the amounts match. Show creation
time (UTC), reference, method, amount/currency, wallet credit, status and Details/receipt.
Detail includes current state/history, quote facts, confirmed credit/refund and
permitted next action; no merchant secrets
or raw webhook. Show **Continue payment** on the history row only when the server
offers it; `Pending` alone is not enough. On click, open the same invoice. If it
was paid or expired in the meantime, refresh the row instead. When the outcome is
unknown, offer **Check status**, not a second checkout. Expired, cancelled and
failed payments remain distinct in history.

**Wallet journal:** posting time, operation/reason, reference, signed posted amount
and resulting balance only if authoritative. Inference debit links to safe Usage;
refund/credit links to payment. Explain why journal posting dates differ from request
start dates in usage graphs. Holds are shown separately and cannot be counted twice.

**Refund flow:** permitted payment -> Request refund -> eligible amount and policy
from server -> reason when required -> review -> Submit request -> pending/approved/
rejected/paid outcome from the server. It is a request, not an instant credit button.
Show protected refund funds separately; no promise that account closure or a failed
generation guarantees a refund. Chargeback/review states give safe next actions.

**Mobile/error:** funds precede tabs; payment review is a readable full-width step,
not compressed modal columns. Critical confirmations do not auto-dismiss. If any
money mutation acknowledgement is lost, preserve the operation reference and offer
Check status; never display guessed balances or invite an unqualified duplicate.

### WS-08 — Settings and account security

**Layout:** open account/security from the signed-in account context rather than a
primary Settings destination. Group sign-in methods, sessions, notice preferences
and account closure; keep appearance controls in the shell.

- Account shows server-provided identity/workspace context; no editable field can
  reassign the authenticated user. Only supported profile edits are offered.
- Sign-in methods list verified links with provider and safe label. Add method
  begins a server-bound authenticated linking flow; unlink explains the remaining
  access method and fresh-proof requirements. The last usable method is protected.
  Same email is not automatic account merge; failures preserve existing access.
- **Browser sessions** is available from the signed-in header at
  `/app/overview?panel=account`. Show the current session first, then other active
  sessions by last activity, with sign-in and activity times. No device names are
  inferred. End a session with inline confirmation; ending the current session
  returns to sign-in. If the result is unknown, refresh the list before retrying.
- Notifications show supported event preferences and threshold semantics. External
  delivery appears only for a qualified channel with verified recipient/consent;
  in-app read status is not acceptance of a tariff or resolution of an incident.
- Only non-sensitive presentation preferences may persist in the browser.
- With a configured support link, **Close account** shows the workspace
  reference and how to save receipts and request a refund before closure.
  Contacting support does not change account state.

Each editable section has its own Save/Cancel state, revision conflict and result.
Navigating away warns about ordinary unsaved edits; credentials/proof are not saved
in a browser draft. Failed save never marks unrelated sections as successfully saved.

### WS-09 — Removed standalone help

There is no cabinet Help tab, modal or `/app/help` route; its legacy preview URL
also uses the normal not-found state. Connection instructions live beside key
issuance and link to the public `/docs` guide.

### WS-10 — Notices

**Entry:** header bell indicates unread state without a second badge; include
the state in its accessible label and respect reduced motion. Desktop
uses a right sheet; mobile uses a full-screen sheet. Header has Notifications and
Close, then a bounded newest-first list, pagination and Notification settings.

Each notice has type/icon, concise headline, absolute timestamp, read state and
one domain-approved action. Read/unread is independent of payment, incident or
price acceptance. Open rechecks target permission/current state; a deleted/expired
target gives a safe explanation. Notices are not HTML supplied by a provider.

Low funds opens Billing; payment opens its status; delayed settlement opens own
Usage; security change opens account controls; scheduled change opens the published safe
detail. Persistent service restrictions remain in the page banner even after the
notice is read. Empty means no retained notices; failed load is not an empty list.

**Owner (`/admin/notifications`):** keep Draft, Publish and delivery progress
distinct. Confirm content and audience before the first send. Refresh or a lost
acknowledgement looks up the same publication; draft saving never rings a
customer bell. The customer's view contains no editor or delivery administration.

## Owner panel

Owner pages preserve local-pool capabilities and shared validation, not its exact
visual layout. They never infer owner authority from a customer login, client-side
role flag or direct URL. Browser routes and their API calls are separate namespaces.
Use the architecture's [owner navigation](HOSTED_RELAY_ARCHITECTURE.md#owner-browser-navigation)
and [permission boundaries](HOSTED_RELAY_ARCHITECTURE.md#owner-and-customer-permissions).

**First installation:** provision the owner on the host → sign in with a passkey
→ follow Overview's readiness link → configure customer entry, funding and recovery
→ add a connection → admit the member → review and publish a qualified offer
→ inspect the effective catalog. Return to readiness for the separately authorized
funded qualification and explicit opening. Saving an offer does not open the
service. On later visits Overview opens normally; show unresolved tasks only.

### WS-11 — Owner sign-in

**Entry:** `/admin/login`; `/admin` redirects here without a valid owner session.
Show a compact centered card: Relay, Owner sign-in, explanation of existing-owner
access, Sign in with passkey, recovery guidance and Back to public site. Do not
offer Google/Telegram customer sign-in, a password box or public Create owner.

Sign in opens the platform's passkey ceremony. Distinguish cancellation, unsupported
context, expired attempt, denied owner authority and unavailable verification with
safe wording. Never describe an unknown account as an existing owner. Successful
proof still requires server role verification before navigating to Overview/setup.
Recovery explains the audited server-local process without creating an online
reset bypass. The browser owns device/passkey selection; no fake QR or credential
chooser that implies unsupported authentication. Mobile uses the same native flow.

### WS-12 — Setup and launch readiness

**Layout:** title, current service mode and blocking-summary banner; left nine-step
checklist, right selected step; bottom Back/Next navigation plus a separate named
action for the current step. On mobile use a vertical step list and selected detail.
The checklist remains accessible after initial launch.

Use the canonical steps: Installation; Ownership/recovery; Public entry/sign-in;
Capacity; Money; Protection; Recovery/signals; Qualification; Public opening.
Every step shows requirement, current status, why it matters, observed time,
configuration/artifact revision, evidence expiry and allowed next action. Mark
not configured, pending, passed, failed, expired and not applicable distinctly.
Do not let an owner manually tick required technical/commercial proof as passed.

Opening a step reads stored status only. An active check has its own labelled
confirmation showing affected scope, external effect, duration/cost bound where
applicable and operation reference. Back/Next does not repeat a check. Refresh
recovers the same operation. Correcting configuration invalidates affected proof
and explains which steps must be repeated; it does not silently keep green badges.

Public opening shows the exact target mode, checked revisions, unresolved blockers,
customer impact and fresh-proof requirement. Enable only when the backend permits
the transition; refusal stays explicit. Separate qualification entry prerequisites
from successful qualification results, so the wizard cannot demand a test result
before authorizing its test. A written plan or synthetic fixture is not real launch
evidence. Do not display production-ready from a percentage-complete progress bar.

### WS-13 — Owner overview

**Header:** Overview, current service mode plus observed health as separate labels,
last refresh and Review operations action. Setup blockers appear before metrics.

**Order:** show a setup blocker or actionable incident only when applicable, then
the rolling-period charts, including **posted customer charges**. No permanent
top panel duplicating capacity data. Put current active members and their models
in Pool; queued work and physical attempts are not extra sales. Historical
filters never change current activity. Keep separately evidenced provider expense
in the relevant detail, not a guessed profit curve.

Click model/source opens the corresponding permitted detail; an incident opens
Operations or the filtered Errors view. A card has no implicit Enable all, probe,
reauthenticate or retry action. On mobile show critical incidents before charts;
do not put essential warnings
below a long analytics dashboard.

### WS-14 — Connections and credential inventory

**Tabs:** Accounts → Add account/reauthenticate; API providers → Add/edit provider;
Proxies → import, rename and assign; Trash → restore or guarded removal, no Add
action. Open the appropriate form from the current tab without asking for the
type twice. Search, filter or select visible rows without changing their state.

**Table:** safe label/family, credential/auth state, pool membership, enabled models
summary, observed quota with known renewal timing, last refresh/action and row
menu. Show freshness and partial data; additional resets are separate and appear
only where supported. API provider rows show safe endpoint/label and masked
credential metadata, never query credentials or full secrets. Endpoint display is
sanitized.

**Row detail:** show authentication, observed models/quota, relevant settings
and recent events for this source; no empty tabs for unsupported features.
Private owner metadata never appears in customer Usage or public APIs.
Refresh displayed status is a read; Check source/Refresh credentials is a separate
explicit domain action with scope and any external effect disclosed.

**API purchase control:** in **Edit → Connection**, show **Relay purchase limit**:
**No Relay limit** (default) or **Monthly limit** with one USD amount. In capped
mode show this month's accounted spend, the amount reserved for open attempts
and when the allowance renews (UTC). Keep **Provider balance** separate, in the
units the connector reports, with its freshness. The seller tops up directly
with the provider; there is no second wallet or checkout here. If new sends are
blocked, identify whether provider credit or the Relay limit needs attention.
This control is owner-only; model prices still belong in **Edit → Models**.

**Import:** follow [SC-09: choose source, preview, confirm, inspect each result](HOSTED_RELAY_ARCHITECTURE.md#from-connection-import-to-first-offer).
For a qualified provider login use its consent flow; for a supported file show
only safe selectable rows. For an API source use the compact Connection/Models
editor: endpoint/protocol, write-only credential, proxy, then declared versus
observed models. **Edit → Models** shows the API-wide default markup once above
the model list. On a model row the seller edits **Purchase** and **Sale**
together using the [shared price editor](#ws-16--models-and-tariffs). Saving
the connection does not publish the prices.
In the static API inventory, open this editor from the connection card's model
count. It edits rates for admitted models; declared IDs and verified models
outside the pool remain read-only.
For a Desktop transfer, the browser opens pairing/status only;
Desktop selects the local data. The result's next action is **Review in Pool**,
not an automatic admission. An interrupted operation can be reopened by reference.

On HTTPS installations with web intake enabled, **API providers → Add API
connection** uses the Connection/Models editor with the supported Responses
protocol, masked key and proxy fields, and optional declared IDs. Save confirms
with a passkey. A lost reply offers **Check connection save**, without resending
the key. The card separates declared models from current verified observations.
In local HTTPS qualification, the same **Models** dialog offers **Check model**
with explicit billing bounds and passkey confirmation. The short check may
incur upstream charges. Reopening the dialog reads the last receipt and does
not start a check. A passed check updates verified observations only.

**Proxies:** preview valid/error rows without echoing authentication. Choose the
connection's effective binding (specific, inherit, explicitly direct) and save
with its revision. Before removing a used proxy, show affected connections;
require explicit reassignment or leave them blocked, never silently send direct.
An authorized connection check is distinct from refreshing the list. No passive
speed tests or promise that a proxy reachable from Desktop works on the server.

**Action semantics:**

| Action | Confirmation and result |
| --- | --- |
| Reauthenticate/update credential | Name source and affected owner; complete approved flow, show saved/active revision separately, preserve old safe state on failure. |
| Add to pool | Review target pool, eligible models and implications; only authoritative admission success changes membership. |
| Park | Remove membership but keep connection/credentials; explain dispatched work and drain policy. |
| Disable / drain | Distinguish immediate block on new work from bounded completion of existing work; display real transition state. |
| Move to trash / restore | Explain retention and membership consequences; restore cannot silently reactivate or resurrect obsolete credentials. |
| Permanently remove | Explicit target and irreversibility; fresh proof/fencing per policy; never promise removal erases financial/audit obligations. |
| Export credentials | Separate deliberate secret-export action with fresh proof, scope and protected delivery; no generic export from a filtered table. |

In the static API inventory, keep pause/resume and a compact **Actions**
disclosure on the source row. Park and trash confirm inline; a trashed row
offers restore in place. Show membership separately from pause and draining.
Pool Participants reads those states without treating a retained publishing
anchor as a member.

Bulk operations begin with current-page selection. All-matching selection, if
supported, names the bounded snapshot/count and allowed action; never assume every
filtered source is eligible. Review individual outcomes, revision conflicts and
partial failure; do not mark the whole batch successful after one acknowledgement.
Mobile cards keep label/auth/membership/quota freshness and Actions, with remaining
settings in detail. No hover-only credential or destructive controls.

### WS-15 — Pool

**Layout:** title and explicit pool selector only when more than one supported
pool exists; active configuration revision and pending-activation banner; tabs
Participants and Model rules. Rotation settings open from Participants, not a
third tab. Highlight observed active work only, never a predicted next member.
Add participant selects saved eligible
inventory.

Participant rows show order, source label, enabled/draining state, model eligibility,
observed quota and active work. Reorder with accessible Move up/down controls;
dragging may be additional, never required. Park/remove membership is not Delete
connection. Explain stale state and revision conflicts before changing a live pool.

**Participants:** the pencil opens the member's supported model switches, pause
state, note and applicable source settings. The rotation control opens *on this
tab*: only show supported modes and their relevant inputs (e.g. member order
or shares); cancel discards the draft. Read current member/model and simultaneous
work from the server. No animation from a click or preview speed, and no “next”.
Advanced route/quota/tool options appear only on a member/policy that supports
them, not as a permanent control strip.

The restricted API pool's pencil opens the Desktop-style member editor.
**Models** shows only admitted exact model IDs and their switches. One
**Save** confirms the whole draft with a passkey. A conflict reloads the current
flags for review. **Settings** holds whole-connection pause, resume and removal.
Neither tab becomes a second price or publication editor.

Keep participant rotation separate from model display order. The rotation editor
shows only backend-qualified strategies and connector-supported controls. The
restricted API pool supports **Automatic** and **Primary & reserve**. Show member
order controls only in the latter; **Save** confirms the change with one passkey.
After a conflict show the current policy and discard the stale draft. Show the
saved mode beside observed dispatches without marking a predicted member. Quota
uses shared terminology with provider-specific units and windows; unknown data
is not unlimited capacity. Additional reset controls appear only when supported,
independently of quota display. Authentication methods vary by provider. A
provider account, an authorization method and a model publisher are distinct.

**Model rules** owns catalog state, ordering and publication. Its rows expose
price components/bands inline using the same editor as Connections. Collapse a company
locally to navigate, or deliberately move a company/model to change display
order. Draft/save, publish and effective state are distinct. See WS-16 below
for the editor's price review and the storefront readback.

On a subscription offer with a qualified API alternative, show one owner control:
**Use paid API when subscription quota runs out** (off by default). In the
existing change review, name the exact offer and let the seller confirm which
API connections may be used. Show their purchase rates and monthly limit, or
**No Relay limit**; link to Connections to change them. Saving the rule does
not send a request or publish a new price. Keep a saved rule visible and
revocable even if its backup becomes unavailable. No extra rotation mode,
customer prompt or storefront badge.

Review shows changed fields, affected models/sources, validation and impact on new
versus in-flight requests. Save with expected revision, then display durable saved
revision and runtime activation independently. An activation failure is not success;
rollback is offered only through a qualified server action, not inverse browser
writes. Mobile uses stacked policy sections and full-width review.

### WS-16 — Models and tariffs

**Navigation:** `/admin/models` answers “What offer is published now?” It is a
read-only storefront view, not a simulated customer session or proof of any
particular key's grant. Use the same catalog presentation as WS-05. An owner
edit link opens **Pool → Model rules** on that model; preserve its identity and
return context. Connections and Pool use the same pricing draft and publish review.

**Editing a rule:** readable model name with exact ID available for copying/details,
supported operations/protocols, qualification/grant
evidence, effective availability, public-publication state, tariff completeness
and Actions. Qualification, publication and paid enablement are separate decisions.
No switch creates capability or upstream permission. Explain why a route is blocked
and which permitted setup action can resolve it without exposing data to customers.

**Grouped workspace:** use the existing compact publisher-group pattern, such as
OpenAI with its model rows, under the [canonical model-order contract](HOSTED_RELAY_ARCHITECTURE.md#pool-model-controls-and-catalog-order).
Group headers collapse/expand and expose accessible reorder controls; model rows
move only within their publisher group. Dragging has keyboard/button equivalents.
Collapse changes presentation only, not enablement. Keep disabled rows editable
for the owner and retain their saved positions.

Align model name, configured state and currency/per-million price components in
one readable surface rather than nested model cards. Preserve the Hosted design
system; on narrow screens use labelled rows with reachable state/order controls.
Show the selected context/service-tier variant and distinguish input, cache and
output prices without guessed zeroes. Customer views project the authorized order
and published tariffs, not private routes, draft prices or management switches.
Conflicting saves retain the user's draft and offer a fresh server revision; never
silently overwrite newer order, model settings or prices.

**Shared price editor:** one row per supported component for the selected
variant, with **Purchase · [API label]** and **Sale · [public model]** side by
side in USD per million tokens. In Connections the source is already selected;
in Pool show a source choice only when there is more than one. Switching the
source changes the purchase column, not the model's selling rule. For
subscription-only supply, show fixed selling prices without inventing a token
purchase cost.

On the model row choose **Use API default**, **Model markup** or **Fixed prices**.
For a percentage show the server's proposed selling rates; for fixed prices
edit those same cells directly. Show the inherited percentage and let the seller
return an override to the default without retyping. If another API sets a proposed
selling rate, identify it in review. Changing an API default previews only the
offers it affects. Mark missing required fields on their rows;
warn when a sale component is below known cost without blocking a deliberate price.
Review compares the proposed prices with the published ones and shows effective
time and reason. The [pricing rules](HOSTED_RELAY_ARCHITECTURE.md#api-source-rates-and-the-public-tariff)
stay on the server. Customers see no purchase costs, markups or source picker.

**Publish flow:** draft -> validate -> review old/new revision and affected scope
-> required notice/effective-time check -> fresh verification -> explicit publish
-> confirmed schedule/current revision. Display pending/failed activation honestly.
Ordinary increases/removals use the reviewed server notice policy; the form cannot
silently shorten it. Emergency restriction has a separate reasoned action and
does not reprice existing holds. Historical tariffs remain readable, never editable.
On mobile stack component/band rows, keep units visible and use a full-page review.

### WS-17 — Access and paired devices

Open **Paired devices** from Connections when authorizing or revoking a Desktop.
For key abuse, open only the affected customer's masked key from its authorized
case; show scope, expiry and last use, with guarded restriction/emergency revoke.
No browse-all key inventory, owner-issued customer keys or Applications launcher.
Customer instructions for connecting a client belong in Keys, not here.

Paired devices show label, granted scope, proof/expiry, last use and revocation.
Pairing has explicit begin/review/confirm/expiry steps with narrowly scoped authority;
it is not a persistent full-access token displayed on page load. Revocation explains
which future management actions stop and which ongoing domain operations remain
accountable. Show concurrent revision conflicts without silently recreating pairing.
Mobile keeps authority/scope/status visible before expandable technical details.

The pairing review grants only a subset of the
[Desktop Hosted scope](HOSTED_RELAY_ARCHITECTURE.md#desktop-hosted-scope): Overview,
Usage, and Connections and pool. Do not offer an all-admin scope. Customer
administration, balances/keys, payments, tariffs and installation/security settings
remain in the web owner panel; pairing must not grant these operations merely
because they are hidden in Desktop navigation. Display denied, expired and revoked
authority without silently upgrading or recreating the device grant.

**Explicit Desktop transfer:** show this as a separate paired-device operation,
not automatic synchronization or a website credential scrape. Desktop initiates
local inspection/selection; the owner sees scoped source/destination, sanitized
snapshot, differences, authority and the proposed credential/proxy bindings before confirming.
Show stage/drain/activation/verification and recovery states from the canonical
[transfer contract](HOSTED_RELAY_ARCHITECTURE.md#local-and-hosted-pool-transfer).
Saving a connection is not proof of safe ownership transfer. Failed acknowledgement
queries the existing operation; do not overwrite newer logins with an old snapshot
or silently retain two dispatch owners. No transfer is implied by opening Access.

### WS-18 — Customers

**Entry:** from request detail, a refund case or an exact workspace filter in
owner Usage. Open one authorized workspace, not a customer directory, Add user form
or empty global dashboard. Standard shared access follows the approved offer
policy; no one adds each new customer as a participant in the physical pool.

**Detail:** show the verified workspace reference and restrictions; expose only
the keys/limits, payment, request or audit evidence needed for the case. Link to
its authoritative record. No impersonation, raw session view or provider secrets.

Show the current access state and latest receipt. Suspend, reinstate and close
use inline review and passkey confirmation. **Check result** recovers the same
receipt after a lost acknowledgement. Limit/grant changes show affected keys,
not a silent policy overwrite. Money adjustments link
to WS-19's audited journal flow; no editable Balance input on the customer profile.
Deletion/closure follows reviewed retained-record policy, not hard-delete shortcuts.
On mobile keep customer context visible above case actions so an action cannot appear
to target the previous customer after a delayed response.

### WS-19 — Owner payments and money actions

**Entry:** Overview links to the bounded list of refunds/payments needing action,
including when no inference request exists. An exact payment reference or a
customer case opens its detail directly. No permanent Payments sidebar or
all-customer banking dashboard. The case shows normalized
reference, customer, provider/method label, paid amount/currency, USD credit/hold,
current state and observed/posted time. Never add mixed currencies into one total.

**Detail:** authoritative intent/quote, external-event verification state, journal
links, refund encumbrance and retained reconciliation history. Display safe provider
correlation metadata, not raw webhooks, merchant secrets or customer card details.
Read refresh cannot trigger another provider payment or post a credit.

**Guarded actions:** approve/reject an eligible refund, perform a supported status
reconciliation, or propose an authorized journal adjustment. Each review shows
target, exact amount/currency, reason/reference, current policy/state, operation ID
and required fresh proof. A status recheck is not labelled Retry payment. Approval
does not claim money was returned before the domain records provider/journal proof.
Unknown external outcome remains pending review, not a green manual-success toggle.

Corrections append authorized compensating entries; no UI edits/deletes existing
postings or enters arbitrary SQL. Insufficient eligible funds/concurrent holds and
duplicate actions have specific outcomes. Use full-width mobile review; keep exact
amount and target visible above confirmation rather than only in a tooltip.

### WS-20 — Usage, errors and request timeline

**List:** `/admin/usage` with Requests, model/key groupings and Errors. Seven-day UTC default;
time/model/source/customer/key/origin/category/stage/severity/execution/accounting
filters as supported; explicit request-ID lookup; visible as-of/coverage. Secondary
filters collapse into a labelled panel, never silently persist across customer scope.

The Errors tab has one row per logical request needing attention. Completed
execution with unresolved accounting remains visible with both states. Recovered
attempt failures and confirmed cancellation are optional filters, not automatically
red incidents. Keep logical, attempt, rejection, cancellation and accounting counts
distinct; an empty known-outcome denominator is unavailable, not 100% success.

**Full detail page:** `/admin/usage/requests/{request_id}` with back-to-filtered-list,
safe summary/code/ID and badges. Main column: observed admission/queue/dispatch/
delivery/checkpoint/settlement timeline with nested attempts. Side column: customer/
key/model/protocol context, evidence completeness and current money projection.
Below: safe next actions and support summary. Mobile puts summary and money before
the timeline; stages can expand without hiding unresolved execution/accounting.

The page answers what happened, where/when, whether substantive output was forwarded,
what happened to money and what is safe next. Missing/expired stages are labelled;
clock gaps and unknown cause are not invented. Socket writes are not proof a human
read an answer. Show why any alternate attempt was permitted under Relay's contract.
There is no prompt/answer preview, generic replay button or probe on page open.
Status lookup/runbook navigation are distinct from privileged diagnostic actions.

### WS-21 — Operations and service lifecycle

**Top:** intended mode, observed public condition and evidence freshness as separate
facts; guarded Change mode action; link to readiness/setup. Below: current incidents,
component/job health, backup/restore evidence, alerts, temporary diagnostics and
scheduled announcements. Use tabs/sections, not embedded third-party dashboards.

- Incidents show domain, safe code, first/last observed, current state, affected
  scope and permitted runbook action. Acknowledged is not Resolved. Empty retained
  events do not prove health; last successful observation is explicit.
- Backup shows completion/delivery/retention evidence; Restore qualification shows
  its separate tested revision/date/result. A scheduled or uploaded backup is not
  a proven successful restore. No restore-to-production shortcut is introduced.
- Mode change review shows source/target, reason, new admission/payment impact,
  drain behavior, late obligations and required evidence/proof. Closing/Closed is
  not a pause toggle; no one-click Reopen. Database outage cannot produce a claimed
  committed mode change. Failed acknowledgement recovers the same transition.
- Temporary diagnostics uses a bounded form: allowlisted scope, reason, expiry
  and event/size budgets, safe data categories and fresh verification. Active state
  shows remaining lifetime/budget and Stop. Expiry, revocation and server enforcement
  do not depend on the tab staying open. No raw payload toggle or endless debug mode.
- A scheduled tariff/model/service change links to its authoritative revision
  and required notice. For a general broadcast, open Notifications, not a second
  announcement composer in Operations. A notice cannot enable a model or set a price.

Mobile stacks critical incidents, mode, readiness and then secondary operational
sections. Unreachable monitoring/status/support destinations have clear fallback
text; public status links must be reachable independently of this backend.

### WS-22 — Owner settings

Enter a named configuration task from its setup/readiness step or Owner security
from the signed-in owner context. Provide a form only for an operator change that
belongs to the service; quote host-managed settings read-only with a runbook link.
No general settings hub or web controls to rewrite deployment files/certificates.

General covers configured product name, own logo/theme, language defaults and
reviewed legal/support/status links. A labelled local preview is not published
configuration. No arbitrary HTML/JavaScript field or remote theme installer.
Customer sign-in shows configured providers and safe callback requirements; secrets
are write-only replacement fields with configured-state indicators, never reveal
buttons. Testing an integration is explicit and distinct from reading its status.

Owner security lists passkeys/recovery guidance and authorized pairing context.
Adding/removing privileged access needs fresh proof and the server's remaining-
access guards; customer login linking does not grant owner rights. Runtime policy
values show units, bounds and effects; unknown mandatory launch values remain blockers.

**Setup → Money → Payment methods** opens the [checkout builder](HOSTED_RELAY_ARCHITECTURE.md#payment-method-builder),
not a new sidebar tab or the refund queue. An empty installation shows one action:
**Add connection**. Pick an installed provider, fill only its fields, then choose
the methods to offer. For each method with a fee, show **Who pays the fee?**
with **Merchant** and **Customer** as the choices. If the channel allows only
one, show the fixed value instead of a switch. Show the callback address with
Copy when needed; a saved secret shows `Configured`, not its value. A compact
preview shows customer labels in order without starting payment.
**Save draft**, **Check** and **Enable** have
distinct effects. Show a failed check beside the field or method that needs work.
Existing connections are editable rows with their enabled methods and status;
support reordering without drag and disabling a method without deleting its history.

Under **Top-up amounts**, set **Minimum (USD)** and **Quick amounts (USD)**.
**Allow custom amount** starts on. Turning it off previews just the quick buttons;
if no valid quick amount is configured, show the error beside the list before publishing.
Keep **Open invoices per customer** in the same form; a lower limit affects new
payments only.

Each section has reviewable changes, expected revision, Save/Cancel and invalidation
impact on readiness. Sensitive updates require fresh proof. Lost acknowledgement
checks the saved revision, not repeated secret updates. A disconnect cannot expose
submitted secrets through recovery drafts. Mobile preserves labels and save/result
context. A restart/update requirement points to the documented operations flow;
it is not an automatic deployment triggered by saving branding.

### WS-23 — Audit

**Composition:** open History on a changed object, operation or incident, already
scoped to that target. Use bounded filters and a detail view, not a permanent
Audit sidebar or log console. Columns: time, actor/device, action, target safe ID,
result, change/operation reference and reason where authorized.

Detail shows allowlisted changed fields, before/after revisions, reason and links
to domain outcome. Secret fields show that a protected value changed, never its
old/new contents. Raw request bodies, credentials and unrestricted personal data
are excluded. Audit of an attempted action is not proof that money/access changed.
Rows are not editable or deletable through the panel; retention follows policy.
Export exists only if a bounded authorized contract is specified, not because the
table component happens to have an Export button. Mobile prioritizes actor/action/
target/result, with exact time/revision in detail.

## Shared states and interaction rules

### WS-24 — Loading, missing data and recovery

These states apply to every screen and each independently loaded panel. A working
header must not conceal a failed wallet panel, and one failed chart need not erase
safe unrelated content. The server's status/code, authority and evidence decide
the state; a loading timer never proves failure or success.

| State | Appearance and explanation | Allowed next action |
| --- | --- | --- |
| Initial load | Stable page headings and structural skeleton; no fake numbers or shimmering financial amounts | Cancel navigation or wait; do not expose previous-user content |
| Background refresh | Existing safe current-scope data remains readable with refreshing/as-of label | One bounded refresh; no duplicate in-flight fetch or focus jump |
| Confirmed empty | Specific message: no keys, no retained requests or no configured sources; show the appropriate first action | Create/setup only where authorized |
| No filter match | Preserve filters and say no records match; do not imply there has never been usage | Reset/change filters |
| Partial coverage | Mark missing period/components and what can/cannot be concluded; chart gaps and unavailable totals remain explicit | Narrow range or inspect supported detail |
| Stale snapshot | Last observed time and explicit stale label; unknown current capacity/funds is not a current guaranteed value | Refresh/status lookup; guarded actions still revalidate server-side |
| Backend/network unavailable | Safe explanation and reference when available; retain only policy-permitted current-scope stale data | Read retry and independent support/status; no automatic money/inference resend |
| Session expired | Clear protected page/cache/secret display and return to the correct customer/owner sign-in with a safe destination | Reauthenticate; never use the other session type as fallback |
| Forbidden or absent object | Safe unavailable/not-found explanation without confirming another tenant's object | Back to authorized list or support |
| Revision conflict | Explain that the object changed; show safe changed fields/current revision where permitted | Reload and review; no silent overwrite or replay |
| Outcome unknown after mutation | Visible operation/reference and statement that the effect may have committed | Check the same operation's status; no new effect until domain policy permits |
| API/client incompatible | Stable shell with actionable version message and support/build context | Supported update/reload path; disable incompatible mutations, never coerce data |
| Maintenance/closure | Persistent server-projected condition, time and restrictions; keep permitted history/status/support | Follow the actual domain policy, not a blanket hide-everything screen |
| Deliberate navigation cancellation | No alarming global failure toast for an intentionally aborted obsolete read | New page/filter owns its current request |

Successful confirmations must say what is confirmed: key issued, payment created,
credit posted, refund requested, configuration saved or runtime activated. Avoid
one generic Done message for every intermediate state. Use `role=status`-style
polite feedback for ordinary updates; reserve urgent alerts for actionable errors.
Do not read secrets or every chart polling update into assistive technology.

**Unknown route and system page:** use the current safe shell, a concise not-found/
unavailable title, Back to overview/site and support. A boundary fallback contains
safe route/build/reference data only. No stack trace, automatic refresh loop or
blank screen that looks like a deleted account. An unauthorized object and absent
object do not disclose different cross-tenant existence information.

**Legal/support destinations:** the default site links to reviewed operator-configured
destinations; this specification supplies no invented legal text or ticket system.
If those documents are hosted within the chosen site, use a readable maximum-760-pixel
text column, H1, document version/effective date, section headings, accessible links
and print-friendly content; do not hide material terms in a modal. Link language
and availability are explicit. The public footer remains reachable without login
or a healthy customer API. New local legal routes require an explicit route entry,
not a catch-all page serving whichever external URL a visitor supplies.

### Navigation, filters and browser history

Canonical customer and owner route tables live in architecture. The browser query
allowlist below specifies presentation state, not a new API or authorization input:

| Scope | Allowed browser state | Boundary |
| --- | --- | --- |
| Public landing | Named section anchors; configured locale/theme preferences | No personalized IDs, key or identity callback material in landing links |
| Customer reports | `from`, `to`, `model`, `key`, `outcome`; `request` for own detail | UTC bounded range, public model ID and opaque own key/request IDs only; `key` is never a bearer secret |
| Customer billing | Supported `tab`, `payment` | Own opaque payment reference; amount/success in a URL is not trusted evidence |
| Account controls | `panel=account` on Overview; supported section only | No linked identity secrets or editable private fields in the URL |
| Owner reports | Supported time/model/source/customer/key/origin/category/stage/severity/state selectors | Bounded opaque authorized IDs/enums; no freeform provider body or unbounded query |
| Owner connection/pool detail | Supported tab and opaque object reference | Fresh authority/revision lookup; URL does not grant mutation scope |

Unknown/invalid parameters are rejected or safely reset with explanation, not
passed as executable filters. URL changes preserve only nonsensitive allowlisted
state. Owner request detail uses its canonical path. Restoring browser history
revalidates session/object access; back/forward never repeats a submitted mutation.
Use one history entry for a detail open and restore parent filters/scroll on close.
Do not trap the Back button in a cycle of modal and query-state updates.

Selecting filters cancels obsolete reads. Only the newest current-session/scope
response may render. Changing a filter resets its cursor; chart/table/export use
matching normalized filters and compatible snapshot evidence. Do not combine
wallet values from one account with graph data from another pending request.
Private page titles, query values and action payloads never go to public analytics.

Sign out targets the current customer or owner authority explicitly. Do not promise
that it revokes the other session class or all inference keys. Switching surfaces
clears protected client state from the previous scope and verifies the appropriate
session; retaining a cookie cannot justify displaying cached data from another role.
Back/forward cache restoration must undergo the same session/scope safety check.

### Forms, confirmations and copy behavior

Use immediate local format feedback for convenience and server validation for
authority. Field errors appear inline plus an accessible summary for multiple
errors. Preserve nonsecret current-form values during recoverable failures; never
persist credentials, authentication proof or payment-sensitive details as drafts.
Decimal money input stays an exact typed value, not a floating-point recalculation.

Submission displays progress and prevents another submission of the same effect.
Server payments, credential updates and inference continue after page navigation.
A slow operation stays pending with a safe reference/status action.
Use server deadlines/retry guidance; no frontend timeout converts pending to failed.
Ordinary unsaved edits get Leave/Stay; loss of a displayed one-time key gets a
different warning. Escape/back closes the view; committed operations stay active.

Fresh-proof prompts show action context before the platform ceremony. After proof,
revalidate target/revision and explicitly confirm the pending action; a successful
passkey gesture alone must not execute an outdated destructive request. Do not
stack a new confirmation on top of another modal; preserve the review in one flow.

Copy buttons name their target, copy only on explicit action and report success
only after the clipboard action succeeds. If blocked, permit manual selection
with guidance. No automatic clipboard reads or clipboard clearing promises.
Support copy is an allowlisted typed summary, not Copy entire page/console.
Money/secret/error explanations are visible in text, not only in hover tooltips.

### Dialog and detail inventory

| Surface | Opened from | Required review/result | Failure or dismissal behavior |
| --- | --- | --- | --- |
| Mobile filters | WS-03/04/20 | Supported fields, active-count, Apply/Reset | Cancel preserves applied query; no fetched data from an unsubmitted filter |
| Model detail | WS-01/05/16 | Appropriate public/customer/owner projection, exact bands and scope | Unauthorized/private fields never fall back into a public sheet |
| Customer request detail | WS-03/04 | Both outcome dimensions, usage/charge basis, safe support | Expired/foreign detail has no raw fallback |
| Key creation / one-time display | WS-06 | Own workspace, effective policy, confirmed issue, explicit copy warning | Lost response checks issuance; leaving cannot reveal it later |
| Key/session revoke | WS-06/08/17 | Exact target and future-use effect | Unknown result checks same action; no promise of refund/cancelled upstream work |
| Top-up review / payment status | WS-07 | Quote, fees/currency, stable reference and server state | Return/cancelled tab does not invent paid/failed state |
| Refund request / approval | WS-07/19 | Eligible amount, policy, reason and affected holds | Request/approval/returned money remain different states |
| Identity unlink / protected change | WS-08/22 | Remaining access, target and fresh-proof context | No last-login loss, silent account merge or secret draft |
| Connection park/drain/trash/remove | WS-14/15 | Distinct action, active work, revision and retained obligations | Partial/stale result stays visible per source |
| Pool revision / tariff publication | WS-15/16 | Exact diff, affected scope, effective time and notices | Saved, scheduled and activated are separate results |
| Customer suspension / journal correction | WS-18/19 | Target, permitted consequences, reason and reference | No hidden balance edit or impersonation |
| Service mode / closure | WS-12/21 | Current/target mode, readiness, drain and late obligations | No fake committed transition on DB outage or one-click reopening |
| Temporary diagnostics | WS-21 | Scope, safe data, budgets, expiry, reason and proof | Stop/expiry enforced server-side; no indefinite capture after tab loss |
| Unsaved changes / secret-display exit | Applicable forms | Explain exactly what will be discarded versus already committed | Default safe action is Stay/Cancel; no accidental resubmission |

### Chart and table grammar

Use line charts for observed continuous measures, bars for counts/posted charges
and stacking only for mutually exclusive normalized components. Do not use a pie
chart as the main billing display or combine counts/currency on an unlabelled axis.
Use stable model/series color assignments within a view; cap the visual series
count with an explicit aggregate remainder, while the accessible table retains
all authorized rows and reconciles. Distinguish series with labels/markers/dashes,
not color alone. Use theme-aware contrast checks for final chart palettes.

Axes show UTC interval and units; partial buckets are marked. Missing evidence
creates a visible gap, not zero, smoothed interpolation or an implied good state.
Tooltips and keyboard detail expose exact interval/value/completeness. Latency
shows population/sample count; no percentile averaging. A zero comparison baseline
or incomplete comparison window produces Not available, not a dramatic percentage.
Do not animate counts from zero or use chart motion to imply incoming live work.

Tables show server-supported sorts only, distinguish selected row from active
operation and preserve selection scope on refresh. Bulk-selection messages name
the bounded set. Avoid row-wide click targets that make Copy/menu buttons open
detail unexpectedly. Numeric columns align; wrapped labels never overlap actions.
Export and Show table use the same metric/filter definitions as the chart.

### Accessibility, motion and privacy

Use semantic headings, landmarks, labelled controls, skip navigation and a logical
keyboard order. Visible focus is never covered by sticky headers or bottom actions.
Tab/menu/dialog behavior follows the chosen accessible primitives; verify it in
the rendered app rather than declaring compliance from a component's name.
After route changes focus the page heading appropriately; background refresh must
not steal focus. Close returns focus to a surviving trigger or a sensible fallback.

Support 320-pixel reflow, text enlargement and 200%/400% zoom without page-wide
horizontal scrolling or unreachable actions. Test actual Russian labels, long
model IDs and six-decimal amounts, not only short English fixture data. Clipboard,
charts, code blocks, tables and native passkey handoff must work without a mouse.
Do not rely on hover, red/green color or animation as the sole explanation.

Keep ordinary transitions subtle, roughly 120–180 ms, and respect reduced motion.
Essential warnings and financial results do not disappear on a timer. Optional
toasts are supplementary; modal deadlines come from the server and remain readable.
No third-party session replay, unsolicited support uploads or page-content tracking.
Private routes are not included in public sitemaps/previews and carry appropriate
no-index intent, which is not a substitute for server authorization/no-store.
Public social metadata contains only reviewed generic product text and own artwork.

### Core English wording

#### Outcome badges

Map the [canonical states](HOSTED_RELAY_ARCHITECTURE.md#canonical-execution-and-accounting-states)
to presentation without changing their transitions. Pre-admission activity remains
separate from admitted request rows; show a phase only when the API exposes it.
Customer `active` is a grouping, not another persisted state. Owner detail may show
finer phases, but it uses the same underlying execution/accounting evidence.

| State dimension | Canonical value | English caption | Visual meaning |
| --- | --- | --- | --- |
| Execution | `received`, `admitted`, `reserved` | Preparing | Neutral; no claim of provider execution or completion |
| Execution | `queued` | Queued | Informational; bounded wait, not a failed request |
| Execution | `dispatched`, `streaming` | In progress | Informational; output/terminal evidence is still independent |
| Execution | `completed` | Completed | Success for execution only, not a settlement result |
| Execution | `incomplete` | Incomplete | Warning with evidenced reason; not automatic refund eligibility |
| Execution | `failed` | Failed | Failure; safe cause explanation, not automatic blame on Relay or an account |
| Execution | `cancelled` | Cancelled | Neutral confirmed cancellation; disconnect alone does not select this badge |
| Execution | `uncertain` | Outcome unknown | Warning; Check status/support, not an inferred retry |
| Accounting | `none` | No accounting record | Neutral; no claim that usage was free |
| Accounting | `held` | Held | Informational hold, not a posted additional charge |
| Accounting | `held_for_reconciliation` | Reconciling | Pending accounting; retain its actual deadline/basis |
| Accounting | `settled` | Settled | Show posted amount and basis; execution may still have failed |
| Accounting | `released` | Hold released | Hold release, not a fabricated payment refund or second credit |

Estimated settlement and partial/missing usage are separate explanatory badges,
not new execution states. Refund/payment statuses use their own domain contracts;
never map a provider callback string directly to these request badges.

#### Shared labels

These are semantic wording baselines, not the final complete translation catalog.
Every screen/action/state still needs reviewed English copy and translations in
the corresponding locale files before implementation.

| Meaning | English |
| --- | --- |
| Overview | Overview |
| Usage | Usage |
| Models/prices | Models and prices |
| Inference keys | API keys |
| Billing | Balance and payments |
| Account settings | Account and security |
| Available money | Available |
| Request hold | Held for requests |
| Refund hold | Held for refunds |
| Posted customer usage charge | Posted usage charges |
| Execution completed | Request completed |
| Execution unknown | Execution outcome unknown |
| Pending accounting | Settlement pending |
| Estimated accounting | Estimated settlement |
| Payment awaiting confirmation | Awaiting payment confirmation |
| Confirmed wallet credit | Funds credited |
| Refund requested | Refund requested |
| Read failed | Unable to load data |
| Status check, not repeat | Check status |
| Data freshness | Data as of |
| Stale snapshot | Data is stale |
| Unknown evidence | No confirmed data |
| Safe support copy | Copy safe support details |
| Saved inventory only | Saved, not in pool |
| Stop new work and finish existing | Draining current requests |
| Privileged context | Owner panel |

Error sentence order: what is known -> what it means -> safe next action -> safe
reference. Example: `The request status is still unknown. Sending it again may
create a new billable request. Check the status or share the request ID with
support.` Translations must carry the same uncertainty, not promise failure or a refund.
No accusatory customer wording, raw stack traces, unexplained HTTP-only errors or
generic Retry labels for actions with different financial consequences.

## Design handoff and acceptance

### Sequence before implementation

1. Confirm canonical routes, public/customer/owner boundaries and WS-01–WS-24 scope.
2. Review tokens, shared components and customer/owner flows with synthetic data.
3. Check the states and viewports below; resolve affected H/HD blockers before coding.
4. Update accepted choices here, not in separate screen passports or handoff reports.
   Written approval does not complete runtime or production acceptance.

### Screen-to-package and evidence matrix

All evidence below is required future work, not tests executed by writing this
document. Use existing roadmap test locations where applicable; add a focused
case when needed.

| Design coverage | Owning work | Required evidence |
| --- | --- | --- |
| WS-01/02: public entry, prices, identity | H06, H07, H13 | Anonymous publication has no private fields; empty/unavailable/configured-provider states and safe return/closed registration |
| WS-03/04: overview, reports, request detail | H10, H12, H14 | Current versus historical separation, chart/table/export reconciliation, coverage/retention, both outcomes and no raw content |
| WS-05/06: models, status and keys | H07, H09, H12, H14 | Exact bands/scopes, grant removal, neutral history without observations, bounded limits, one-time key, lost acknowledgement and revoke semantics |
| WS-07: money | H04, H08, H14 | Quote/intent/credit/refund distinctions, duplicate/lost return, holds and exact monetary display |
| WS-08/10: account, notices | H06, H09, H12, H14, H15 | Last-login/session safety, confirmed owner publication, per-recipient read state and permission |
| WS-11/12: owner auth/setup | H03, H05, H15, H16 | No public ownership claim, fresh proof, real readiness evidence, invalidation and guarded opening |
| WS-13/14/15/16: capacity/pool/prices | H01, H04, H07, H15 | Parity, saved versus admitted, current counts, stale/partial mutations and revisioned publication |
| WS-17/18/19: access/customers/payments | H04, H05, H08, H09, H15 | Separate authorities, no impersonation, audited restrictions/adjustments and money outcome recovery |
| WS-20/21/22/23: diagnosis/operations/config/audit | H03, H05, H12, H15, H16 | Safe timeline, bounded diagnostics, service lifecycle, protected settings and attributable noneditable audit |
| WS-24 and shared presentation | H00, H13, H14, H15, H17 | Responsive/state/copy/focus checks across roles, locales and themes; no guessed outcomes or stale-account leakage |

### Required design review set

- End-to-end customer and owner flows, including uncertain money, one-time secrets,
  service closure, stale revisions and partial actions; preserve role boundaries.
- Widths 1440, 1024, 768, 390 and 320 CSS pixels, zoom and keyboard/software keyboard.
- English, Russian and Simplified Chinese; light/dark, long labels/IDs, fractional/large amounts and every shared state.
- Clear actions, units, errors, confirmations and recovery; no unexplained icons or
  invented backend behavior. Use labelled synthetic fixtures, not assumed prices.
- Retain asset/license provenance with the assets and distinguish proposals from
  accepted choices. Review rendered layout/focus/accessibility; prose and contrast
  checks alone do not satisfy the implementation gates.
