# Hosted Relay Web Design

**Design specification for future implementation.**
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

1. **Original design.** Author composition, typography, assets, wording,
   chart treatment and interactions. Reference projects contribute functional
   lessons only. Do not copy their screenshots, page components, CSS or branding.
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

- Use the platform UI sans-serif stack with Cyrillic support; no remote font
  requests. A later custom font needs local assets, licensing and loading review.
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
Keep necessary warnings visible; supporting detail belongs in tooltips or Help.
Ship complete Russian/English variants. Browser
locale selects the initial language; an explicit user choice wins. Changing locale
does not restart a payment/login operation or translate machine model IDs/codes.
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

Use a centered content column, maximum 1200 pixels, 32-pixel desktop gutters and
16-pixel mobile gutters. Header is 72 pixels on wide screens, 64 on mobile, with
product mark/name left, section navigation center and language/theme/sign-in right.
At narrow widths, keep brand and sign-in visible; put section links in a labelled
menu. The header may be sticky but must not cover anchor headings or focused fields.
Footer groups product/help and operator-provided legal/support links compactly.
No fixed deployment domain or unverifiable trust logo.

Use a restrained Relay wordmark and an original small route/connection motif.
Review final vector artwork for appearance and provenance. Keep Desktop's existing
brand files. Favicon remains recognizable at small size without text.

### Customer shell

At 1200 pixels and above, use a 240-pixel left sidebar, 64-pixel header and main
content with 32-pixel padding. Main content has a maximum width of 1440 pixels;
forms remain at most 720 pixels. Sidebar contains brand, the seven customer
destinations in the canonical order, then Help/support context near the bottom.
No duplicate Help navigation item is required if it already occupies its main row.

Header: page context left; current personal-workspace label, balance shortcut,
notifications and account menu right. Workspace is a label, not an unsupported
organization switcher. Balance links to Billing. Account menu has account name,
Settings, theme/language and Sign out; it does not contain an owner-role toggle.
Place the page title, one-sentence description and main page action in the content
header below it. A service-condition banner appears above page content when needed.

At 768–1199 pixels, use a 72-pixel labelled-on-demand navigation rail with accessible
names, visible current-page title and 24-pixel content gutters. At less than 768,
replace the rail with a menu drawer: 56-pixel header, 16-pixel gutters, full-width
content and no icon-only bottom navigation competing with seven sections. Keep
menu, title, notifications and account control visible; put wallet summary in page
content or menu. A drawer traps focus,
closes on explicit action/Escape and returns focus to its trigger.

### Owner shell

Reuse the geometry/primitives, with a persistent textual `Owner / Владелец` context
and service mode in the header. Group navigation into Overview; Capacity; Customers
and access; Activity; Settings. No CSS class or selected role grants permissions.
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
| Page header | One H1, concise explanation, one primary task and quiet secondary actions; buttons wrap below title on mobile. |
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

Screen IDs are design references, not new implementation packages. Architecture
owns canonical routes; this inventory binds them to presentation passports below.
Tabs, sheets and dialogs do not create a new top-level app for each operation.

| ID | Audience and destination | Main task |
| --- | --- | --- |
| WS-01 | Public `/` and section anchors | Understand service, inspect published models/prices, start sign-in |
| WS-02 | Customer `/login` and configured identity handoff | Sign in or create own account safely |
| WS-03 | Customer `/app/overview` | Understand current funds/access and period usage |
| WS-04 | Customer `/app/usage` | Filter requests and explain a specific charge/error |
| WS-05 | Customer `/app/models` | Inspect authorized models, prices and connection requirements |
| WS-06 | Customer `/app/keys` | Issue, restrict, replace and revoke an inference key |
| WS-07 | Customer `/app/billing` | Fund access and reconcile payments, holds and journal |
| WS-08 | Customer `/app/settings` | Manage identity, sessions, preferences and account requests |
| WS-09 | Customer `/app/help` | Connect a client and get safe support information |
| WS-10 | Customer header notice sheet | Read own notifications and follow permitted next actions |
| WS-11 | Owner `/admin/login` | Authenticate an existing owner with a passkey |
| WS-12 | Owner `/admin/operations/setup` | Complete explicit setup/readiness steps |
| WS-13 | Owner `/admin/overview` | See present capacity, activity and action-worthy incidents |
| WS-14 | Owner `/admin/connections` | Manage saved accounts/API providers and their states |
| WS-15 | Owner `/admin/pools` | Manage admitted capacity and shared pool policies |
| WS-16 | Owner `/admin/models`, `/admin/tariffs` | Qualify availability and publish revisioned retail prices |
| WS-17 | Owner `/admin/access` | Manage client access, scoped keys and paired devices |
| WS-18 | Owner `/admin/customers` | Inspect customer access/status and authorized actions |
| WS-19 | Owner `/admin/payments` | Reconcile payments, refunds and audited adjustments |
| WS-20 | Owner `/admin/usage` and request-detail route | Investigate logical requests, attempts and accounting |
| WS-21 | Owner `/admin/operations` | Manage readiness, incidents, diagnostics and service lifecycle |
| WS-22 | Owner `/admin/settings` | Configure security, identity, integration and presentation policy |
| WS-23 | Owner `/admin/audit` | Inspect attributable administrative changes |
| WS-24 | Shared exceptional views | Recover from denied/unavailable/expired/incompatible states safely |

## Public entry and sign-in

### WS-01 — Landing page

**Entry:** anonymous `/`; signed-in visitors may still read it. The header action
becomes Open cabinet. Never
render personalized balances into public/cacheable HTML.

**Top-to-bottom composition:**

1. Header: brand, How it works, Models and prices, FAQ, support, locale/theme,
   Sign in. Section links use `/#how-it-works`, `/#models`, `/#faq`, `/#support`.
2. Hero: left, a short product promise and two-line explanation; right, an
   original static diagram `Your client -> Relay API -> Available model`.
   Primary Start action opens `/login`; secondary View models scrolls to prices.
   Example RU headline: `Доступ к моделям. Расходы под контролем.` EN:
   `Model access. Usage you can understand.` Do not promise unlimited capacity,
   fixed availability, free trials or unqualified providers.
3. How it works: three numbered steps, Sign in and inspect prices; Fund access
   and issue a key; Configure a client and track usage. Explain that model execution
   can incur usage charges.
4. Models and prices: short explanation, published-model search, capability filter
   and a table of public model name, supported protocol, input/cached-input/output
   prices per million tokens, relevant band indicator and Details. Public prices
   come from the architecture's read-only public model projection, not build-time
   constants or customer `/models`. Show tariff revision/effective time and any
   required band details. Missing components are not displayed as zero or hidden
   in a misleading single minimum price. Published listing does not grant access.
5. Product explanation: three concise blocks for configurable access, traceable
   usage and explicit controls. Describe actual qualified capabilities only.
6. FAQ: what is sold, compatible clients/protocols, how tokens are charged, why
   funds may be held, what happens on errors and where to get support. Answers
   link to reviewed policy; no invented refund, privacy or retention promises.
7. Final Start action, then footer with operator identity/contact, terms, privacy,
   support and independently reachable service-status destination from configuration.

**States/actions:** public configuration failure shows neutral branding and an
unavailable message, not another installation's defaults. If prices cannot load,
retain the explanation but replace the table with an explicit unavailable block.
If there are no published models, say so and do not advertise functioning paid
access. Visible login methods and whether registration is open come from policy.
A maintenance/closed banner is above the hero; do not hide outage/support links.

**Mobile:** hero becomes text then diagram; diagram is decorative to assistive
technology when its text is already stated. Pricing rows become model cards with
labelled component prices and Details.
No artificial customer counters, reviews, brand logos or product screenshots.

### WS-02 — Customer sign-in and return

**Entry:** `/login`, or a protected customer route without a valid customer
session. Store only a validated same-origin destination, not arbitrary redirects.
Returning to a protected detail still requires a fresh authorized lookup.

**Composition:** centered surface up to 440 pixels wide: brand/back-to-site;
`Войти в Relay / Sign in to Relay`; short statement that a new account may be
created if registration is open; vertical configured provider buttons; required
policy/terms disclosure; support and theme/language controls. Do not render empty
email/password fields, an unsupported password reset or a separate duplicate
registration form. Customer Google/Telegram/OIDC sign-in is not owner sign-in.

**Flow:** choose an enabled provider -> show Leaving for sign-in -> use the
server-created provider flow -> return through the configured callback -> verify
session/workspace -> open Overview or the validated destination. First registration
shows only the policy acceptance required by the reviewed server contract; no
preselected optional marketing consent. Same-email identities are not silently
merged. Identity linking is initiated from authenticated Settings instead.

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

### WS-03 — Overview

**Header:** `Обзор / Overview`, short period context, primary Top up; Create key
is secondary unless the account has no key, when the onboarding block highlights it.

**Placement:**

```text
Page title                                      Top up   Create key
Service condition / actionable account notice, when present
CURRENT: Available | Held for requests | Held for refunds | Active / queued
FIRST USE: Models -> Funding -> Key -> Client setup (only when useful)
HISTORY: UTC period | model | key | supported outcome | Reset | Refresh
Requests | Normalized tokens | Posted usage charges | Completion rate
Request outcomes over time                     Token components over time
Posted usage charges over time                 Observed latency / sample count
By model                                       By key
Recent requests                                Important notices
```

These groups use the canonical metrics; the layout does not permit adding token
subcomponents twice. Every period panel shares a snapshot/filter contract. Current
funds/activity have their own observation time and are visibly outside the history
filter area. Chart totals are not sums of whatever series happen to be visible.

**Interactions:** period defaults to the last seven UTC calendar days including
today; Today/7 days/30 days/custom use retained coverage. Model/key rows and chart
selection open Usage with supported matching filters. Recent request opens safe
detail; pending payment opens its Billing record; a notice opens its authorized
target. Refresh reads data only. Onboarding completion is based on server evidence;
creating a key and funding may happen in either
order. Successful first usage is never simulated to finish the checklist.

**States/mobile:** a genuinely new account gets useful steps, not fake chart data.
No matching history offers Reset filters; incomplete data explains coverage.
Unavailable wallet remains unknown even if charts loaded. On mobile, keep funds,
first-use/urgent notice, filters, summaries, charts, breakdowns and recent requests
in that order; collapse secondary breakdowns explicitly, not critical warnings.

### WS-04 — Usage and safe request detail

**Header:** `Использование / Usage`; Export is the main action, not Generate again.
Below: UTC period and model/key/outcome filters; query summary/as-of; compact
canonical totals; optional time-series panel; request table; cursor navigation.

**Table order:** started at UTC; public model; masked key label; protocol; execution
outcome; duration/TTFT where available; normalized token summary; customer charge;
accounting status; Details. Start with 25 rows where the server bound permits;
50 is a permitted choice only when supported. Do not invent a total-page count for
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

**Export flow:** choose request versus retained aggregate data, see current filters,
coverage and bounds, then Create export. A pending job row exposes status; completed
download remains authorized and expires. Polling/refresh recovers the same job;
there is no silent second export or claim of completeness after truncation.

**States/mobile:** missing usage is Unknown, estimated charge has its basis, and
expired detail links to retained aggregate information only where available.
Unknown execution offers status/support, never blind resend. Mobile rows keep
model/time, both states and charge visible; the remaining fields move to detail.
Closing a sheet restores filters/scroll. A direct `request` query opens only the
current customer's permitted record and does not disclose an unauthorized ID.

### WS-05 — Models and prices

**Header:** `Модели и цены / Models and prices`; explanatory subtitle says these
are the models/prices applicable to this account. Search and supported capability/
protocol filters precede the list. Group by a clear public model family only where
the authorized projection provides one; do not infer capability from a name.

**Row/card:** public name and exact model ID; supported protocols/capabilities;
input, cached input and output rates with currency/per-million unit; band/details
indicator; availability explanation; Connect. Desktop uses a comparative table;
mobile uses labelled stacked rows. Never show internal account counts or inventories.

**Detail:** supported operations, protocol endpoints, context/service-tier/cache
bands and exact component prices; effective tariff revision; safe availability
explanation; placeholder configuration example. Connect opens Help with model/
protocol preselected. Show unavailable or not-granted information
only where the server deliberately exposes that public descriptor; no hidden-catalog
discovery via client guesses. Scheduled revisions show effective time and scope;
historical request prices stay in Usage.

**States:** no authorized models points to support/account policy; temporary
capacity is different from absent grant, disabled route or missing tariff. A model
without a complete qualified price cannot look ready for paid use. Do not present
roadmap connectors as available options.

### WS-06 — API keys

**Header:** `API-ключи / API keys`, primary Create key. A short explanation separates
the Relay inference key from upstream accounts and from customer/owner sessions.
List columns: name/prefix, status, allowed models/operations, effective limits,
expiry, last use and Actions. Never fetch a stored full secret for this list.

**Create form:** name; permitted models/operations; supported spending/rate/concurrency
limits; expiry; summary of effective inherited workspace policy. Units, reset time,
default versus explicitly configured value and upper bounds are visible. Empty
does not silently mean Unlimited. No user-selected value can widen workspace grants.
Final Create button names the operation; server field errors stay next to fields.

**One-time result:** replace the form with a transient secret panel, explicit Copy,
warning that the secret cannot be shown again, and Continue to setup. Do not copy
automatically, put it in a URL or announce it through a live region. Leaving warns
that the display will be lost, not that the key will be undone. If issuance committed
but the response was lost, show the resulting masked key/status and the replacement
path; never silently issue another key or claim the key is absent.

**Edit/replacement/revoke:** edit only server-supported mutable fields with revision
checks. For a replacement, explain creation of a new key, client migration and
explicit revocation of the old key; do not promise revocation until acknowledged.
Revoke dialog names key/prefix and explains new-use blocking versus in-flight work;
confirm with an explicit action. A successful revoke is visible
in the list; repeated confirmation addresses the same operation.

**States/mobile:** empty list has Create key and Help; expired/revoked entries are
readable, not deleted. Policy changes/conflicts require reload and review. Full
secret display is private even in responsive screenshots/test fixtures; use synthetic
values. The mobile key card keeps name/status/scope and an accessible action menu.

### WS-07 — Billing, payments and refunds

**Header:** `Баланс и платежи / Balance and payments`; primary Top up. First row
shows current available funds, request holds and refund holds, with snapshot time
and explanations. Pending payment amounts are a separate block, not part of funds.
Below use Payments and Wallet journal tabs, with bounded filters and pagination.

**Top-up steps:**

1. Amount and qualified payment method: server-supported min/max/currency, no
   invented preset amounts, hidden fees, recurring subscription or automatic top-up.
2. Review: amount paid, wallet credit in USD, fees/conversion if applicable,
   quote expiry, method and applicable policy. Change means requote, not local math.
3. Create payment: one durable operation; redirect to the returned approved hosted
   payment flow. Relay does not collect raw card details in its own generic form.
4. Return/status: stable payment reference, status and What happens next. Query the
   same payment; provider redirect text is never proof of credited money.
5. Confirmed credit: server acknowledgement, receipt where available, fresh wallet
   snapshot and Return to overview. Never show a success animation while pending.

**Payments table:** created UTC, reference/method, amount/currency, wallet-credit
amount, canonical status and Details/receipt. Detail includes current state/history,
quote facts, confirmed credit/refund and permitted next action; no merchant secrets
or raw webhook. Pending/expired/cancelled/failed are distinct. Do not start a new
payment automatically after a timeout; explain an existing unresolved intent.

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

**Layout:** page header, section navigation, then a maximum-720-pixel form column.
Sections: Account; Sign-in methods; Sessions; Notifications; Appearance; Account
closure/help. On mobile navigation becomes a labelled section selector, not tiny tabs.

- Account shows server-provided identity/workspace context; no editable field can
  reassign the authenticated user. Only supported profile edits are offered.
- Sign-in methods list verified links with provider and safe label. Add method
  begins a server-bound authenticated linking flow; unlink explains the remaining
  access method and fresh-proof requirements. The last usable method is protected.
  Same email is not automatic account merge; failures preserve existing access.
- Sessions show bounded safe device/client description, last activity and current
  session marker. Revoke other/current session has clear consequences; revoking
  customer sessions does not claim to revoke inference keys or owner sessions.
- Notifications show supported event preferences and threshold semantics. External
  delivery appears only for a qualified channel with verified recipient/consent;
  in-app read status is not acceptance of a tariff or resolution of an incident.
- Appearance has RU/EN and System/Light/Dark controls. These are the only broadly
  persistent client preferences. Sensitive page content stays out of this store.
- Closure area explains what access ends, which unsettled obligations remain and
  which records must be retained under reviewed policy. Use the documented request/
  support process; no fake instant-delete button before a supported domain contract.

Each editable section has its own Save/Cancel state, revision conflict and result.
Navigating away warns about ordinary unsaved edits; credentials/proof are not saved
in a browser draft. Failed save never marks unrelated sections as successfully saved.

### WS-09 — Connection and help

**Composition:** connection checklist; model/protocol/client selection; configured
API base URL; model ID; placeholder key; minimal compatible example; troubleshooting;
support/status links. Show only qualified client/protocol combinations. No account
credentials, auto-detected local application launcher or unsupported client badge.

Copy base URL and model separately. Copy example uses `YOUR_RELAY_API_KEY`, unless
the user explicitly includes a just-issued secret still held in transient memory.
State that executing a model example may cost money; visiting Help and copying it
does not. Do not add automatic connection probes or an in-browser playground.

Troubleshooting begins with scope/key, available funds, model availability and
request status, then opens the matching safe Usage record. Support summary includes
safe code/ID, observed time and build/API context, not prompts, responses, full keys,
headers or raw browser exceptions. No promise that support can recover lost keys.
Mobile code scrolls inside its own region with an accessible Copy action outside.

### WS-10 — Notices

**Entry:** header bell with unread count, also accessible by a text label. Desktop
uses a right sheet; mobile uses a full-screen sheet. Header has Notifications and
Close, then a bounded newest-first list, pagination and Notification settings.

Each notice has type/icon, concise headline, absolute timestamp, read state and
one domain-approved action. Read/unread is independent of payment, incident or
price acceptance. Open rechecks target permission/current state; a deleted/expired
target gives a safe explanation. Notices are not HTML supplied by a provider.

Low funds opens Billing; payment opens its status; delayed settlement opens own
Usage; security change opens Settings; scheduled change opens the published safe
detail. Persistent service restrictions remain in the page banner even after the
notice is read. Empty means no retained notices; failed load is not an empty list.

## Owner panel

Owner pages preserve local-pool capabilities and shared validation, not its exact
visual layout. They never infer owner authority from a customer login, client-side
role flag or direct URL. Browser routes and their API calls are separate namespaces.
Use the architecture's [owner navigation](HOSTED_RELAY_ARCHITECTURE.md#owner-browser-navigation)
and [permission boundaries](HOSTED_RELAY_ARCHITECTURE.md#owner-and-customer-permissions).

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

**Order:** current active logical requests/physical attempts/queued work; capacity
summary by public model and saved/admitted/healthy source states; actionable
incidents; historical request/error/latency panels; posted customer charges and
separately evidenced provider expense; recent administrative changes.

Capacity table columns: model, eligibility/availability, active logical requests,
active attempts, bounded queue, quota/reset evidence where known and observed at.
Counts describe current ownership, not requests started in the last minute.
Historical filters do not affect those counts. Expense unavailable is not zero
and does not create a fabricated profit number. Noncustomer operational work is
labelled separately from sold requests and customer wallet charges.

Click model/source opens the corresponding permitted detail; an incident opens
Operations or the filtered Errors view. A card has no implicit Enable all, probe,
reauthenticate or retry action. On mobile show mode/critical incidents first, then
current activity/capacity, then historical charts; do not put essential warnings
below a long analytics dashboard.

### WS-14 — Connections and credential inventory

**Header:** Connections; primary Add connection with explicit Account/API-provider
choice. Tabs Accounts and API providers, status/family/search filters and Trash
view. Saved inventory and pool membership are separate facts everywhere.

**Table:** safe label/family, credential/auth state, pool membership, enabled models
summary, observed quota/reset, last refresh/action and row menu. Show freshness
and partial data. API provider rows show safe endpoint/label and masked credential
metadata, never query credentials or full secrets. Endpoint display is sanitized.

**Detail sheet/extended page:** Summary; Authentication; Models/quota; Connection
settings; Automation; Recent safe events. Show only supported controls for that
source type. Private owner metadata never appears in customer Usage or public APIs.
Refresh displayed status is a read; Check source/Refresh credentials is a separate
explicit domain action with scope and any external effect disclosed.

**Add flow:** choose a qualified connector -> show supported sign-in/import/API-key
method -> obtain explicit authority/consent -> validate/save to protected storage
-> show Saved and whether parked -> separately review pool admission. No automatic
sale/enablement just because login succeeded. Credential entry is transient, never
browser-drafted; imported files use bounded approved formats, not executable content.
Do not offer unqualified future connectors or accept arbitrary callback URLs.

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

Bulk operations begin with current-page selection. All-matching selection, if
supported, names the bounded snapshot/count and allowed action; never assume every
filtered source is eligible. Review individual outcomes, revision conflicts and
partial failure; do not mark the whole batch successful after one acknowledgement.
Mobile cards keep label/auth/membership/quota freshness and Actions, with remaining
settings in detail. No hover-only credential or destructive controls.

### WS-15 — Pool

**Layout:** title and explicit pool selector only when more than one supported
pool exists; active configuration revision and pending-activation banner; tabs
Participants, Routing/policies, Model rules. Add participant selects saved eligible
inventory.

Participant rows show order, source label, enabled/draining state, model eligibility,
observed quota and active work. Reorder with accessible Move up/down controls;
dragging may be additional, never required. Park/remove membership is not Delete
connection. Explain stale state and revision conflicts before changing a live pool.

The policy editor covers existing shared controls: order/rotation, model enablement,
context/reasoning/service-tier settings, quota automation, supported tool rules,
automatic proxy-derived timezones and other qualified parity items. Use labelled sections with units and
effective inherited values. Pricing links to the canonical tariff flow.
Unsupported controls show an explanation only when useful; never appear enabled.

Review shows changed fields, affected models/sources, validation and impact on new
versus in-flight requests. Save with expected revision, then display durable saved
revision and runtime activation independently. An activation failure is not success;
rollback is offered only through a qualified server action, not inverse browser
writes. Mobile uses stacked policy sections and full-width review.

### WS-16 — Models and tariffs

**Navigation:** Models and Tariffs are related tabs linked to `/admin/models` and
`/admin/tariffs`, not two conflicting editors. Model detail connects them by exact
public identity and version. Search/filter state survives the return to a list.

**Models:** columns public ID, supported operations/protocols, qualification/grant
evidence, effective availability, public-publication state, tariff completeness
and Actions. Qualification, publication and paid enablement are separate decisions.
No switch creates capability or upstream permission. Explain why a route is blocked
and which permitted setup action can resolve it without exposing data to customers.

**Tariff editor:** public model; currency and per-million unit; supported input/cache/
output components; applicable context/service-tier bands; exact decimal values;
effective time; change reason. Blank price differs from an explicitly confirmed
zero. Unsupported/overlapping components cannot be billed twice. Show a synthetic,
clearly labelled server-calculated example if supported, not browser-priced quotes.

**Publish flow:** draft -> validate -> review old/new revision and affected scope
-> required notice/effective-time check -> fresh verification -> explicit publish
-> confirmed schedule/current revision. Display pending/failed activation honestly.
Ordinary increases/removals use the reviewed server notice policy; the form cannot
silently shorten it. Emergency restriction has a separate reasoned action and
does not reprice existing holds. Historical tariffs remain readable, never editable.
On mobile stack component/band rows, keep units visible and use a full-page review.

### WS-17 — Access and paired devices

**Layout:** API connection information followed by separate tabs Customer keys and
Paired devices; owner security links to Settings.
Display public protocol endpoints, compatible instructions and safe copy controls.
There is no Desktop Applications launcher in this panel.

Customer-key rows show assigned customer/user, label/prefix, scopes, effective
budgets/rates/concurrency, expiry, last use and status. Server-authorized issuance,
restriction/replacement/revoke actions follow the same domain rules as customer
keys, with owner audit and no disclosure of stored secrets. An owner cannot convert
an inference key into a management credential by changing a UI field.

Paired devices show label, granted scope, proof/expiry, last use and revocation.
Pairing has explicit begin/review/confirm/expiry steps with narrowly scoped authority;
it is not a persistent full-access token displayed on page load. Revocation explains
which future management actions stop and which ongoing domain operations remain
accountable. Show concurrent revision conflicts without silently recreating pairing.
Mobile keeps authority/scope/status visible before expandable technical details.

**Explicit Desktop transfer:** show this as a separate paired-device operation,
not automatic synchronization or a website credential scrape. Desktop initiates
local inspection/selection; the owner sees scoped source/destination, sanitized
snapshot, differences, authority and proposed membership changes before confirming.
Show stage/drain/activation/verification and recovery states from the canonical
[transfer contract](HOSTED_RELAY_ARCHITECTURE.md#local-and-hosted-pool-transfer).
Saving a connection is not proof of safe ownership transfer. Failed acknowledgement
queries the existing operation; do not overwrite newer logins with an old snapshot
or silently retain two dispatch owners. No transfer is implied by opening Access.

### WS-18 — Customers

**Table:** safe customer identifier/label, status, available/held money, key count,
recent activity and Details. Filters are bounded and server-owned; do not expose
identities through public search. Avoid unnecessary personally identifying columns.

**Detail:** summary of verified identity references and current restrictions;
wallet/holds; keys and limits; usage; payments; relevant audit. Tabs query the same
authorized customer context. No impersonation, raw session view or provider-secret
pane. Distinguish label edits from verified identity changes.

Suspend/reinstate explains reason, new admission/funding/access effects and existing
settlement obligations; use a guarded action with expected revision. Limit/grant
changes show affected keys, not a silent policy overwrite. Money adjustments link
to WS-19's audited journal flow; no editable Balance input on the customer profile.
Deletion/closure follows reviewed retained-record policy, not hard-delete shortcuts.
On mobile keep customer context pinned above its tabs so an action cannot appear
to target the previous customer after a delayed response.

### WS-19 — Owner payments and money actions

**Layout:** independent tabs Payments, Refunds and Journal; period/status/customer
filters; reconciliation-needed summary; bounded table. Columns include normalized
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

**List:** `/admin/usage` with Requests and Errors tabs. Seven-day UTC default;
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
- Announcements reference a real planned tariff/model/service change, audience and
  effective time. Preview RU/EN safe text, validate notice policy, then explicitly
  publish/cancel. Publishing a notice does not itself enable a model or mutate price.

Mobile stacks critical incidents, mode, readiness and then secondary operational
sections. Unreachable monitoring/status/support destinations have clear fallback
text; public status links must be reachable independently of this backend.

### WS-22 — Owner settings

Use section navigation and bounded forms: General/presentation; Customer sign-in;
Owner security; Payments; Limits/data/outbound policy; Notifications. Explain which
changes are application settings versus host-managed configuration; the web form
cannot silently rewrite deployment files or replace certificates.

General covers configured product name, own logo/theme, language defaults and
reviewed legal/support/status links. A labelled local preview is not published
configuration. No arbitrary HTML/JavaScript field or remote theme installer.
Customer sign-in shows configured providers and safe callback requirements; secrets
are write-only replacement fields with configured-state indicators, never reveal
buttons. Testing an integration is explicit and distinct from reading its status.

Owner security lists passkeys/recovery guidance and authorized pairing context.
Adding/removing privileged access needs fresh proof and the server's remaining-
access guards; customer login linking does not grant owner rights. Payment settings
show qualified method/merchant readiness and vault references, not retail tariff
editing or raw merchant credentials. Runtime policy values show units, bounds and
effects; unknown mandatory launch values remain blockers, not attractive defaults.

Each section has reviewable changes, expected revision, Save/Cancel and invalidation
impact on readiness. Sensitive updates require fresh proof. Lost acknowledgement
checks the saved revision, not repeated secret updates. A disconnect cannot expose
submitted secrets through recovery drafts. Mobile preserves labels and save/result
context. A restart/update requirement points to the documented operations flow;
it is not an automatic deployment triggered by saving branding.

### WS-23 — Audit

**Composition:** title, scope/privacy explanation, UTC/actor/action/object filters,
bounded table and detail sheet. Columns: time, actor/device, action, target safe ID,
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
| Settings | Supported section/tab only | No linked identity secrets or editable private fields in the URL |
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
Leaving a page does not claim to cancel a server payment, credential update or
inference. A slow operation remains pending with a safe reference/status action.
Use server deadlines/retry guidance; no frontend timeout converts pending to failed.
Ordinary unsaved edits get Leave/Stay; loss of a displayed one-time key gets a
different warning. Escape/back closes a view, not an already committed operation.

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
| Key creation / one-time display | WS-06/17 | Effective policy, confirmed issue, explicit copy warning | Lost response checks issuance; leaving cannot reveal it later |
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

### Core RU and EN wording

#### Outcome badges

Map the [canonical states](HOSTED_RELAY_ARCHITECTURE.md#canonical-execution-and-accounting-states)
to presentation without changing their transitions. Pre-admission activity remains
separate from admitted request rows; show a phase only when the API exposes it.
Customer `active` is a grouping, not another persisted state. Owner detail may show
finer phases, but it uses the same underlying execution/accounting evidence.

| State dimension | Canonical value | RU / EN caption | Visual meaning |
| --- | --- | --- | --- |
| Execution | `received`, `admitted`, `reserved` | Подготовка / Preparing | Neutral; no claim of provider execution or completion |
| Execution | `queued` | В очереди / Queued | Informational; bounded wait, not a failed request |
| Execution | `dispatched`, `streaming` | Выполняется / In progress | Informational; output/terminal evidence is still independent |
| Execution | `completed` | Завершён / Completed | Success for execution only, not a settlement result |
| Execution | `incomplete` | Не завершён полностью / Incomplete | Warning with evidenced reason; not automatic refund eligibility |
| Execution | `failed` | Ошибка выполнения / Failed | Failure; safe cause explanation, not automatic blame on Relay or an account |
| Execution | `cancelled` | Отменён / Cancelled | Neutral confirmed cancellation; disconnect alone does not select this badge |
| Execution | `uncertain` | Результат неизвестен / Outcome unknown | Warning; Check status/support, not an inferred retry |
| Accounting | `none` | Нет записи расчёта / No accounting record | Neutral; no claim that usage was free |
| Accounting | `held` | Зарезервировано / Held | Informational hold, not a posted additional charge |
| Accounting | `held_for_reconciliation` | Проверяется расчёт / Reconciling | Pending accounting; retain its actual deadline/basis |
| Accounting | `settled` | Рассчитано / Settled | Show posted amount and basis; execution may still have failed |
| Accounting | `released` | Резерв снят / Hold released | Hold release, not a fabricated payment refund or second credit |

Estimated settlement and partial/missing usage are separate explanatory badges,
not new execution states. Refund/payment statuses use their own domain contracts;
never map a provider callback string directly to these request badges.

#### Shared labels

These are semantic wording baselines, not the final complete translation catalog.
Every screen/action/state still needs reviewed RU/EN copy before implementation.

| Meaning | Russian | English |
| --- | --- | --- |
| Overview | Обзор | Overview |
| Usage | Использование | Usage |
| Models/prices | Модели и цены | Models and prices |
| Inference keys | API-ключи | API keys |
| Billing | Баланс и платежи | Balance and payments |
| Account settings | Аккаунт и безопасность | Account and security |
| Connection help | Подключение и помощь | Connection and help |
| Available money | Доступно | Available |
| Request hold | Зарезервировано для запросов | Held for requests |
| Refund hold | Зарезервировано для возврата | Held for refunds |
| Posted customer usage charge | Списано за использование | Posted usage charges |
| Execution completed | Запрос завершён | Request completed |
| Execution unknown | Результат выполнения неизвестен | Execution outcome unknown |
| Pending accounting | Расчёт ещё не завершён | Settlement pending |
| Estimated accounting | Расчёт по оценке | Estimated settlement |
| Payment awaiting confirmation | Ожидаем подтверждения платежа | Awaiting payment confirmation |
| Confirmed wallet credit | Средства зачислены | Funds credited |
| Refund requested | Запрос на возврат отправлен | Refund requested |
| Read failed | Не удалось получить данные | Unable to load data |
| Status check, not repeat | Проверить статус | Check status |
| Data freshness | Данные на | Data as of |
| Stale snapshot | Данные устарели | Data is stale |
| Unknown evidence | Нет подтверждённых данных | No confirmed data |
| Safe support copy | Скопировать данные для поддержки | Copy safe support details |
| Saved inventory only | Сохранено, не в пуле | Saved, not in pool |
| Stop new work and finish existing | Завершение текущих запросов | Draining current requests |
| Privileged context | Панель владельца | Owner panel |

Error sentence order: what is known -> what it means -> safe next action -> safe
reference. Example: `Статус запроса пока неизвестен. Повторная отправка может создать
новый платный запрос. Проверьте статус или передайте ID в поддержку.` Its English
counterpart must carry the same uncertainty, not promise failure or a refund.
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
| WS-05/06: models and keys | H07, H09, H14 | Exact bands/scopes, unavailable grants, bounded limits, one-time key, lost acknowledgement and revoke semantics |
| WS-07: money | H04, H08, H14 | Quote/intent/credit/refund distinctions, duplicate/lost return, holds and exact monetary display |
| WS-08/09/10: settings, help, notices | H06, H09, H12, H14 | Last-login/session safety, placeholder examples, safe support, verified consent and permitted notice targets |
| WS-11/12: owner auth/setup | H03, H05, H15, H16 | No public ownership claim, fresh proof, real readiness evidence, invalidation and guarded opening |
| WS-13/14/15/16: capacity/pool/prices | H01, H04, H07, H15 | Parity, saved versus admitted, current counts, stale/partial mutations and revisioned publication |
| WS-17/18/19: access/customers/payments | H04, H05, H08, H09, H15 | Separate authorities, no impersonation, audited restrictions/adjustments and money outcome recovery |
| WS-20/21/22/23: diagnosis/operations/config/audit | H03, H05, H12, H15, H16 | Safe timeline, bounded diagnostics, service lifecycle, protected settings and attributable noneditable audit |
| WS-24 and shared presentation | H00, H13, H14, H15, H17 | Responsive/state/copy/focus checks across both roles/locales/themes; no guessed outcomes or stale-account leakage |

### Required design review set

- End-to-end customer and owner flows, including uncertain money, one-time secrets,
  service closure, stale revisions and partial actions; preserve role boundaries.
- Widths 1440, 1024, 768, 390 and 320 CSS pixels, zoom and keyboard/software keyboard.
- RU/EN, light/dark, long labels/IDs, fractional/large amounts and every shared state.
- Clear actions, units, errors, confirmations and recovery; no unexplained icons or
  invented backend behavior. Use labelled synthetic fixtures, not assumed prices.
- Retain asset/license provenance with the assets and distinguish proposals from
  accepted choices. Review rendered layout/focus/accessibility; prose and contrast
  checks alone do not satisfy the implementation gates.
