# Hosted workspaces

Target: `src/features/app/HostedApp.tsx`
Mode: Operate. Customer `/app/*`, owner `/admin/*`, dialogs, onboarding, empty and
unavailable states. Hosted frontend surfaces share the landing's design system;
this contract covers their presentation and interactions.

## Direction contract

THESIS: The landing's woven identity becomes a quiet, usable workspace. Keep
the familiar navigation, table/chart structure and existing operations.

OWN-WORLD: Locally served Golos Text; pine navigation with cream
selection, warm-neutral work canvas, charcoal dark surfaces; the same geometric Relay mark. Soft controls replace
hard keycaps. Information panels use thin seams, not stacked decorative shadows.
Functional icons share the authored 20-unit grid in `Icons.tsx`, a 1.5px
non-scaling outline and 14/16/18/20px role-based sizes. Models use one semantic
symbol in navigation and statistics. Logo, flags and chart artwork stay separate.

STORY: Move between models, keys, requests and balance without losing
information or existing state. The owner workspace uses the same vocabulary.
Preserve all actual customer/owner capabilities, accounting warnings and detailed
request information. Omit demo/fictional-data scaffold captions throughout the UI;
keep dev-only fixture isolation and actual unavailable states. No new backend or
account behavior.

FORM: Inherit the woven graphic system from the landing. The operating
surface uses fixed readable type sizes, compact hierarchy, responsive navigation
and normal controls rather than a marketing hero or decorative moving traffic.

Page bodies use a wide primary chart with related chart pairs, readable
key/resource rows, brand-field account balance beside history,
consistent data tables and fully integrated overlay bodies. Preserve content,
operations and semantic states, not the old Desktop composition.

Navigation uses pine with cream selection in both themes. The logo toggles
the desktop rail without navigation and closes the phone drawer. Logout is the
only footer action; theme and language are in the header. There is no separate
settings/help page, footer preview label or collapse button. Owner connections
stay separate from customer pages.
Overview combines its title, model selector and period controls in one compact
header, with an open inline summary below and no four-cell card or duplicate
analytics heading. Wrap controls below the title when the content narrows; on
phones the model sits beside the title, periods span the next row and summary
items use two columns. Keep utilities separate and the exact UTC interval hint
on the title. Usage retains its joined summary surface with token and speed
details in the lower row. Preserve exact amounts, uncertainty hints and all
existing filters. Request-token groups remain unboxed.

Request details follow a receipt-like reading order: model and status, key and
UTC in a compact icon/value row below, diagnostics as a trailing icon; exact
posted charge first, then quieter timing. The 760px receipt has one aligned token
ledger, input followed by output, with shared quantity/cost headings. Protocol,
transport, service tier and reasoning settings sit behind the existing technical
disclosure, not in a separate strip. Preserve unknown amounts, failed/pending
notices, download and copy. At phone widths, metadata wraps, metrics use two columns;
export and copy controls keep 44px touch targets. No changes to accounting data.

Notifications use a compact anchored surface: unread count and read action in
the header, bare semantic event icon, model context and an internal request link.
No redundant read-state captions, disabled footer or nested icon tile. Preserve
pending-accounting guidance, read state, dismissal and focus return.

Overview charts have their own Hosted renderer: a taller token plot and paired
supporting figures, restrained data colours, quiet dashed guides and readable
display-scale ceilings. Show no duplicate right axis; percentage keeps its own
0–100% scale. Legend buttons toggle series, while one shared interval tooltip
supports pointer, touch and keyboard navigation. Keep raw counters, posted charges,
UTC boundaries, missing-data gaps and the accessible table unchanged. Known zero
is not missing. Preserve isolated points and complete snapshot-relative rolling
intervals; the Overview does not invent a partial calendar-day bucket.
Do not smooth across gaps or add decorative fills, outer focus rings or animation.

Shared dropdowns use the same pine field and cream selection as the launcher.
The open trigger matches its list; active pointer/keyboard feedback stays distinct
from the selected value. Apply across customer and owner filters,
without changing the sidebar or the landing's attached language picker.

At a content width of 1120px or more, Usage places all seven selectors and search
in one row, adding reset only when filters differ from their defaults. At narrower
widths, retain the four main selectors and the extra-filter disclosure. Share one
set of controls and values across layouts; do not duplicate inputs or drop filters
when the sidebar or viewport changes. Menu focus remains fill-based, without rings.

Long histories stay bounded: payments use 10/25/50 rows with period/type/status
filters; requests and model/key groups use 8/16/32 rows. Totals cover the whole
filter, never just the page. Keep payment operations separate from request charges.
Balance contains the top-up form and payment history, without a spending block.
Spending and rolling periods remain in Usage, with unknown charges and pending
counts explicit. On phones, use compact two-column rows with reachable details and
single-row pager controls, not squeezed desktop columns or tall repeated labels.

FIRST VIEWPORT: Pine sidebar with a clear cream selected row; utility controls separate
from page actions; a calm page heading followed by the actual work. On phones,
utilities stay reachable and the sidebar becomes the existing accessible drawer.

FINISH: Inspect representative pages and all reachable controls in light/dark,
wide/narrow viewports; retain safe empty states and desktop separation. Run
functional regression checks, the detector, fresh finish review, then merge the
approved extension into DESIGN.md and its sidecar. No commit, push or deployment.
